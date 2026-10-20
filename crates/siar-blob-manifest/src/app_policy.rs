//! §142-147: what the subsystem tells an application, and the policy
//! shapes an application hands back. Nothing here draws, notifies or
//! reads the network itself — the core "emits state and enforces policy
//! it is given" (§142, §145).
//!
//! * §143 [`TransferNotification`] / [`NotificationSink`]: `completed`,
//!   `failed`, `action required`, through an abstraction. The core never
//!   calls Android/iOS/desktop notification APIs.
//! * §144 [`AcceptancePrompt`]: only *trusted* facts (size from the
//!   verified offer, media type, expiry). A display name is not here:
//!   it is untrusted peer text and goes through [`crate::filename_safety`].
//! * §145 [`AutoDownloadPolicy`]: per media class × network.
//! * §146 [`DataSaver`]: thumbnail first, defer originals, pause large
//!   background files; [`plan_fetch`] uses the derivative→original link
//!   this crate already stores ([`crate::schema_rows::BlobDerivative`]).
//! * §147 [`BlobStorageClass`]: retention, quota and eviction priority
//!   per class.

use serde::{Deserialize, Serialize};
use siar_domain::{DeviceId, MediaType};

use crate::file_offer::FileOffer;
use crate::ids::{BlobId, TransferId};
use crate::scheduling::TransferClass;
use crate::schema_rows::DerivativeKind;
use crate::transfer_state::TransferState;

// ---------------------------------------------------------------------
// §143 notifications
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferNotification {
    Completed,
    Failed,
    /// The user must do something (accept an offer, free space, …).
    ActionRequired,
}

/// Which notification, if any, a transfer reaching `state` warrants.
/// Paused and in-progress transitions are silent (progress is state, not
/// a notification); a user's own cancel is silent too.
pub fn notification_for(state: TransferState) -> Option<TransferNotification> {
    match state {
        TransferState::Completed => Some(TransferNotification::Completed),
        TransferState::Failed => Some(TransferNotification::Failed),
        TransferState::Offered => Some(TransferNotification::ActionRequired),
        TransferState::Accepted
        | TransferState::InProgress
        | TransferState::Paused
        | TransferState::Cancelled => None,
    }
}

/// Implemented by the host (Android channel, notification centre,
/// desktop toast). The core only ever calls this.
pub trait NotificationSink: Send + Sync {
    fn notify(&self, transfer: TransferId, what: TransferNotification);
}

// ---------------------------------------------------------------------
// §144 acceptance prompt
// ---------------------------------------------------------------------

/// What an accept/decline sheet may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptancePrompt {
    pub transfer_id: TransferId,
    pub from: DeviceId,
    pub size_bytes: u64,
    pub media_type: Option<MediaType>,
    pub expiry_millis: Option<u64>,
}

impl AcceptancePrompt {
    pub fn new(from: DeviceId, offer: &FileOffer) -> Self {
        Self {
            transfer_id: offer.transfer_id,
            from,
            size_bytes: offer.blob.size.bytes(),
            media_type: offer.blob.media_type,
            expiry_millis: offer.expiry_millis,
        }
    }
}

// ---------------------------------------------------------------------
// §145 automatic download
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MediaClass {
    Image,
    Voice,
    Video,
    File,
}

impl MediaClass {
    pub fn of(media: Option<MediaType>) -> Self {
        match media {
            Some(MediaType::ImagePng | MediaType::ImageJpeg | MediaType::ImageWebp) => Self::Image,
            Some(MediaType::AudioOpus) => Self::Voice,
            Some(MediaType::VideoMp4) => Self::Video,
            Some(MediaType::Other) | None => Self::File,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkKind {
    Unmetered,
    Metered,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoAction {
    /// Fetch now.
    Download,
    /// Fetch later (a better network, or data saver off).
    Defer,
    /// Wait for the user.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rule {
    Always,
    /// Only on an unmetered network; deferred otherwise.
    UnmeteredOnly,
    Never,
}

/// §145's examples as defaults: images on Wi-Fi, voice always, video
/// never, files manual; emergency thumbnails always (see
/// [`AutoDownloadPolicy::decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoDownloadPolicy {
    pub image: Rule,
    pub voice: Rule,
    pub video: Rule,
    pub file: Rule,
    /// Never auto-fetch anything larger than this (bytes).
    pub max_auto_bytes: Option<u64>,
}

impl Default for AutoDownloadPolicy {
    fn default() -> Self {
        Self {
            image: Rule::UnmeteredOnly,
            voice: Rule::Always,
            video: Rule::Never,
            file: Rule::Never,
            max_auto_bytes: Some(32 * 1024 * 1024),
        }
    }
}

impl AutoDownloadPolicy {
    pub fn decide(
        &self,
        media: Option<MediaType>,
        size_bytes: u64,
        network: NetworkKind,
        emergency_thumbnail: bool,
    ) -> AutoAction {
        if network == NetworkKind::Offline {
            return AutoAction::Defer;
        }
        // §145 "emergency thumbnail automatically": tiny, and the point
        // of the emergency channel; metered or not.
        if emergency_thumbnail {
            return AutoAction::Download;
        }
        if self.max_auto_bytes.is_some_and(|m| size_bytes > m) {
            return AutoAction::Manual;
        }
        let rule = match MediaClass::of(media) {
            MediaClass::Image => self.image,
            MediaClass::Voice => self.voice,
            MediaClass::Video => self.video,
            MediaClass::File => self.file,
        };
        match (rule, network) {
            (Rule::Never, _) => AutoAction::Manual,
            (Rule::Always, _) | (Rule::UnmeteredOnly, NetworkKind::Unmetered) => {
                AutoAction::Download
            }
            (Rule::UnmeteredOnly, _) => AutoAction::Defer,
        }
    }
}

// ---------------------------------------------------------------------
// §146 data saver
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DataSaver {
    pub enabled: bool,
    /// "pause background large files": above this, background and bulk
    /// transfers wait while the saver is on.
    pub pause_background_above_bytes: u64,
}

/// What to fetch for one message attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStep {
    Fetch(BlobId),
    /// Left for the user to ask for.
    DeferOriginal(BlobId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivativeLink {
    pub derivative: BlobId,
    pub kind: DerivativeKind,
}

/// §146: with the saver on and a thumbnail available, fetch the
/// thumbnail and defer the original; otherwise fetch the original.
pub fn plan_fetch(
    original: BlobId,
    thumbnail: Option<DerivativeLink>,
    saver: &DataSaver,
) -> Vec<FetchStep> {
    match thumbnail {
        Some(t) if saver.enabled && t.kind == DerivativeKind::Thumbnail => vec![
            FetchStep::Fetch(t.derivative),
            FetchStep::DeferOriginal(original),
        ],
        Some(t) if t.kind == DerivativeKind::Thumbnail => {
            // Thumbnail first so the UI has something at once, then the original.
            vec![FetchStep::Fetch(t.derivative), FetchStep::Fetch(original)]
        }
        _ => vec![FetchStep::Fetch(original)],
    }
}

impl DataSaver {
    /// Whether a transfer of this class and size should wait.
    pub fn should_pause(&self, class: TransferClass, size_bytes: u64) -> bool {
        self.enabled
            && matches!(
                class,
                TransferClass::BackgroundSync | TransferClass::Bulk | TransferClass::Relay
            )
            && size_bytes > self.pause_background_above_bytes
    }
}

// ---------------------------------------------------------------------
// §147 storage classes
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlobStorageClass {
    Durable,
    Cached,
    Temporary,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageClassPolicy {
    /// `None`: kept until its references are gone.
    pub retention_millis: Option<u64>,
    /// Share of the store this class may occupy, in permille.
    pub quota_permille: u16,
    /// Lower is evicted first.
    pub eviction_rank: u8,
}

const HOUR: u64 = 60 * 60 * 1000;
const DAY: u64 = 24 * HOUR;

impl BlobStorageClass {
    pub const fn default_policy(self) -> StorageClassPolicy {
        match self {
            Self::Durable => StorageClassPolicy {
                retention_millis: None,
                quota_permille: 1000,
                eviction_rank: u8::MAX,
            },
            Self::Cached => StorageClassPolicy {
                retention_millis: Some(30 * DAY),
                quota_permille: 400,
                eviction_rank: 20,
            },
            Self::Temporary => StorageClassPolicy {
                retention_millis: Some(DAY),
                quota_permille: 100,
                eviction_rank: 5,
            },
            Self::Relay => StorageClassPolicy {
                retention_millis: Some(7 * DAY),
                quota_permille: 200,
                eviction_rank: 10,
            },
        }
    }

    /// Whether data of this class is still within retention.
    pub fn within_retention(self, age_millis: u64) -> bool {
        self.default_policy()
            .retention_millis
            .is_none_or(|r| age_millis < r)
    }

    /// Every class, in the order space is reclaimed.
    pub fn eviction_order() -> [Self; 4] {
        let mut all = [Self::Durable, Self::Cached, Self::Temporary, Self::Relay];
        all.sort_by_key(|c| c.default_policy().eviction_rank);
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }

    #[test]
    fn only_finished_failed_and_waiting_offers_notify() {
        use TransferState::*;
        assert_eq!(
            notification_for(Completed),
            Some(TransferNotification::Completed)
        );
        assert_eq!(notification_for(Failed), Some(TransferNotification::Failed));
        assert_eq!(
            notification_for(Offered),
            Some(TransferNotification::ActionRequired)
        );
        for s in [Accepted, InProgress, Paused, Cancelled] {
            assert_eq!(notification_for(s), None, "{s:?}");
        }
    }

    #[test]
    fn default_auto_download_follows_the_spec_examples() {
        let p = AutoDownloadPolicy::default();
        let d = |m, size, net| p.decide(m, size, net, false);
        // images on Wi-Fi
        assert_eq!(
            d(Some(MediaType::ImageJpeg), 1_000, NetworkKind::Unmetered),
            AutoAction::Download
        );
        assert_eq!(
            d(Some(MediaType::ImageJpeg), 1_000, NetworkKind::Metered),
            AutoAction::Defer
        );
        // voice always
        assert_eq!(
            d(Some(MediaType::AudioOpus), 1_000, NetworkKind::Metered),
            AutoAction::Download
        );
        // video never, files manual
        assert_eq!(
            d(Some(MediaType::VideoMp4), 1_000, NetworkKind::Unmetered),
            AutoAction::Manual
        );
        assert_eq!(d(None, 1_000, NetworkKind::Unmetered), AutoAction::Manual);
        // offline defers everything
        assert_eq!(
            d(Some(MediaType::AudioOpus), 10, NetworkKind::Offline),
            AutoAction::Defer
        );
    }

    #[test]
    fn size_cap_beats_a_permissive_rule_and_emergency_thumbnails_beat_everything_but_offline() {
        let p = AutoDownloadPolicy::default();
        assert_eq!(
            p.decide(
                Some(MediaType::AudioOpus),
                64 * 1024 * 1024,
                NetworkKind::Unmetered,
                false
            ),
            AutoAction::Manual
        );
        assert_eq!(
            p.decide(
                Some(MediaType::ImageJpeg),
                1_000,
                NetworkKind::Metered,
                true
            ),
            AutoAction::Download
        );
        assert_eq!(
            p.decide(
                Some(MediaType::ImageJpeg),
                1_000,
                NetworkKind::Offline,
                true
            ),
            AutoAction::Defer
        );
    }

    #[test]
    fn data_saver_fetches_the_thumbnail_and_defers_the_original() {
        let t = DerivativeLink {
            derivative: b(2),
            kind: DerivativeKind::Thumbnail,
        };
        let on = DataSaver {
            enabled: true,
            pause_background_above_bytes: 1 << 20,
        };
        assert_eq!(
            plan_fetch(b(1), Some(t), &on),
            vec![FetchStep::Fetch(b(2)), FetchStep::DeferOriginal(b(1))]
        );
        let off = DataSaver::default();
        assert_eq!(
            plan_fetch(b(1), Some(t), &off),
            vec![FetchStep::Fetch(b(2)), FetchStep::Fetch(b(1))]
        );
        assert_eq!(plan_fetch(b(1), None, &on), vec![FetchStep::Fetch(b(1))]);
        // A non-thumbnail derivative is not a substitute for the original.
        let wave = DerivativeLink {
            derivative: b(3),
            kind: DerivativeKind::Waveform,
        };
        assert_eq!(
            plan_fetch(b(1), Some(wave), &on),
            vec![FetchStep::Fetch(b(1))]
        );
    }

    #[test]
    fn data_saver_pauses_only_large_background_work() {
        let s = DataSaver {
            enabled: true,
            pause_background_above_bytes: 1_000,
        };
        assert!(s.should_pause(TransferClass::Bulk, 5_000));
        assert!(s.should_pause(TransferClass::BackgroundSync, 5_000));
        assert!(!s.should_pause(TransferClass::Bulk, 500));
        assert!(!s.should_pause(TransferClass::InteractiveAttachment, 5_000));
        assert!(!s.should_pause(TransferClass::EmergencyCritical, 5_000));
        assert!(!DataSaver::default().should_pause(TransferClass::Bulk, u64::MAX));
    }

    #[test]
    fn storage_classes_are_evicted_in_the_right_order() {
        use BlobStorageClass::*;
        assert_eq!(
            BlobStorageClass::eviction_order(),
            [Temporary, Relay, Cached, Durable]
        );
        assert!(Durable.within_retention(u64::MAX));
        assert!(Temporary.within_retention(DAY - 1));
        assert!(!Temporary.within_retention(DAY));
        assert!(Cached.default_policy().quota_permille < Durable.default_policy().quota_permille);
    }

    #[test]
    fn the_acceptance_prompt_carries_only_verified_facts() {
        use crate::descriptor::*;
        use siar_domain::BlobSize;
        let offer = FileOffer {
            transfer_id: TransferId::new(),
            blob: BlobDescriptor {
                blob_id: b(9),
                size: BlobSize::parse(12_400_000).unwrap(),
                chunking: ChunkingDescriptor { chunk_size: 1024 },
                encryption: EncryptionDescriptor {
                    algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                    nonce: [0; 12],
                },
                media_type: Some(MediaType::Other),
                manifest_id: None,
            },
            encrypted_metadata: b"Document.pdf".to_vec(),
            expiry_millis: Some(5),
        };
        let from = DeviceId::new();
        let p = AcceptancePrompt::new(from, &offer);
        assert_eq!(p.size_bytes, 12_400_000);
        assert_eq!(p.from, from);
        assert_eq!(p.expiry_millis, Some(5));
    }
}
