//! §57 UI capability adapter, §58 unknown UI capability, §104
//! user-visible mapping.
//!
//! The UI never reads capability ids. It reads [`UiCapabilities`] (five
//! booleans, as the spec writes them), derived from whatever is known
//! about the peer:
//! - **negotiated** state is authoritative: all required capabilities
//!   present at the minimum version → available, else unavailable;
//! - a **cached** view is a hint: a *fresh* absence disables the action,
//!   but a stale or missing entry never does (§58 — do not over-disable
//!   on a stale cache; negotiate on demand);
//! - **unknown** peers keep every action enabled.
//!
//! The richer [`UiAvailability`] keeps "available" and "may work, will be
//! checked on use" distinct for UIs that want a subtle hint; the plain
//! booleans fold the latter into `true`.
//!
//! §104: user-facing text is a closed, fieldless enum with stable l10n
//! keys — there is no place to put "Capability 0x0421 missing". Technical
//! detail is only produced by [`diagnostic_detail`].

use std::collections::BTreeMap;

use crate::cache::{CapabilityCache, PeerKey, Support};
use crate::error::CapabilityNegotiationError;
use crate::failure::{classify, FailureAction, NegotiationStage};
use crate::id::CapabilityId;
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiFeature {
    Message,
    SendFile,
    AudioCall,
    VideoCall,
    Dtn,
}

impl UiFeature {
    pub const ALL: [UiFeature; 5] = [
        Self::Message,
        Self::SendFile,
        Self::AudioCall,
        Self::VideoCall,
        Self::Dtn,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAvailability {
    Available,
    /// Peer state is unknown/stale: leave the action enabled and
    /// negotiate when it is used (§58).
    MayWork,
    Unavailable,
}

/// The spec's §57 struct, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiCapabilities {
    pub can_message: bool,
    pub can_send_file: bool,
    pub can_audio_call: bool,
    pub can_video_call: bool,
    pub can_dtn: bool,
}

/// Which capabilities (and minimum versions) each UI action needs. An
/// action with **no mapping is unavailable** (an unconfigured feature
/// must not appear to work).
#[derive(Debug, Clone, Default)]
pub struct UiFeatureMap {
    needs: BTreeMap<UiFeature, Vec<(CapabilityId, CapabilityVersion)>>,
}

impl UiFeatureMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn require(&mut self, f: UiFeature, id: CapabilityId, min: CapabilityVersion) -> &mut Self {
        self.needs.entry(f).or_default().push((id, min));
        self
    }

    fn needs(&self, f: UiFeature) -> &[(CapabilityId, CapabilityVersion)] {
        self.needs.get(&f).map_or(&[], Vec::as_slice)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PeerKnowledge<'a> {
    Negotiated(&'a CapabilitySet),
    Cached {
        cache: &'a CapabilityCache,
        peer: &'a PeerKey,
        now_millis: u64,
    },
    Unknown,
}

pub fn availability(map: &UiFeatureMap, f: UiFeature, k: PeerKnowledge<'_>) -> UiAvailability {
    let needs = map.needs(f);
    if needs.is_empty() {
        return UiAvailability::Unavailable;
    }
    match k {
        PeerKnowledge::Negotiated(set) => {
            let ok = needs
                .iter()
                .all(|(id, min)| set.get(id).is_some_and(|d| d.version >= *min));
            if ok {
                UiAvailability::Available
            } else {
                UiAvailability::Unavailable
            }
        }
        PeerKnowledge::Cached {
            cache,
            peer,
            now_millis,
        } => {
            let mut all_yes = true;
            for (id, _) in needs {
                match cache.supports(peer, id, now_millis) {
                    Support::No => return UiAvailability::Unavailable, // fresh absence only
                    Support::Yes => {}
                    Support::MaybeStale | Support::Unknown => all_yes = false,
                }
            }
            if all_yes {
                UiAvailability::Available
            } else {
                UiAvailability::MayWork
            }
        }
        PeerKnowledge::Unknown => UiAvailability::MayWork,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiState {
    pub message: UiAvailability,
    pub send_file: UiAvailability,
    pub audio_call: UiAvailability,
    pub video_call: UiAvailability,
    pub dtn: UiAvailability,
}

pub fn ui_state(map: &UiFeatureMap, k: PeerKnowledge<'_>) -> UiState {
    UiState {
        message: availability(map, UiFeature::Message, k),
        send_file: availability(map, UiFeature::SendFile, k),
        audio_call: availability(map, UiFeature::AudioCall, k),
        video_call: availability(map, UiFeature::VideoCall, k),
        dtn: availability(map, UiFeature::Dtn, k),
    }
}

impl UiState {
    pub fn get(&self, f: UiFeature) -> UiAvailability {
        match f {
            UiFeature::Message => self.message,
            UiFeature::SendFile => self.send_file,
            UiFeature::AudioCall => self.audio_call,
            UiFeature::VideoCall => self.video_call,
            UiFeature::Dtn => self.dtn,
        }
    }

    /// `MayWork` → enabled: "action may remain available".
    pub fn to_capabilities(&self) -> UiCapabilities {
        let on = |a: UiAvailability| a != UiAvailability::Unavailable;
        UiCapabilities {
            can_message: on(self.message),
            can_send_file: on(self.send_file),
            can_audio_call: on(self.audio_call),
            can_video_call: on(self.video_call),
            can_dtn: on(self.dtn),
        }
    }
}

// ---------------------------------------------------------------- §104

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UserCapabilityMessage {
    MessagingUnavailable,
    FileTransferUnsupported,
    AudioCallingUnavailable,
    VideoCallingUnavailable,
    NearbyForwardingUnavailable,
    /// Any security-class failure: deliberately generic.
    SecureConnectionNotPossible,
}

impl UserCapabilityMessage {
    pub const fn l10n_key(self) -> &'static str {
        match self {
            Self::MessagingUnavailable => "capability.messaging_unavailable",
            Self::FileTransferUnsupported => "capability.file_transfer_unsupported",
            Self::AudioCallingUnavailable => "capability.audio_calling_unavailable",
            Self::VideoCallingUnavailable => "capability.video_calling_unavailable",
            Self::NearbyForwardingUnavailable => "capability.nearby_forwarding_unavailable",
            Self::SecureConnectionNotPossible => "capability.secure_connection_not_possible",
        }
    }
}

pub fn unavailable_message(f: UiFeature) -> UserCapabilityMessage {
    match f {
        UiFeature::Message => UserCapabilityMessage::MessagingUnavailable,
        UiFeature::SendFile => UserCapabilityMessage::FileTransferUnsupported,
        UiFeature::AudioCall => UserCapabilityMessage::AudioCallingUnavailable,
        UiFeature::VideoCall => UserCapabilityMessage::VideoCallingUnavailable,
        UiFeature::Dtn => UserCapabilityMessage::NearbyForwardingUnavailable,
    }
}

/// What to tell the user when negotiation of `feature` failed. A
/// session-fatal (security) failure never names the feature or the
/// technical cause.
pub fn failure_message(
    err: &CapabilityNegotiationError,
    feature: UiFeature,
) -> Option<UserCapabilityMessage> {
    match classify(err, NegotiationStage::Extension) {
        FailureAction::TerminateSession => Some(UserCapabilityMessage::SecureConnectionNotPossible),
        FailureAction::DisableExtension => Some(unavailable_message(feature)),
        FailureAction::Ignore => None,
    }
}

/// The technical view for the diagnostics screen: *which* capabilities
/// are missing or too old. Never shown in normal UI.
pub fn diagnostic_detail(map: &UiFeatureMap, f: UiFeature, negotiated: &CapabilitySet) -> String {
    let needs = map.needs(f);
    if needs.is_empty() {
        return format!("{f:?}: no capability mapping configured");
    }
    let problems: Vec<String> = needs
        .iter()
        .filter_map(|(id, min)| match negotiated.get(id) {
            None => Some(format!("{id} missing")),
            Some(d) if d.version < *min => {
                Some(format!("{id} is {} but {min} is required", d.version))
            }
            Some(_) => None,
        })
        .collect();
    if problems.is_empty() {
        format!("{f:?}: all requirements met")
    } else {
        format!("{f:?}: {}", problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::StalenessPolicy;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::epoch::{CapabilityEpoch, PeerCapabilityView};
    use crate::id::CapabilityNamespace;

    const V1: CapabilityVersion = CapabilityVersion::new(1, 0);
    const MSG: CapabilityId = CapabilityId::new(CapabilityNamespace::Messaging, 1);
    const FILES: CapabilityId = CapabilityId::new(CapabilityNamespace::Files, 1);
    const DTN: CapabilityId = CapabilityId::new(CapabilityNamespace::Dtn, 1);
    const AUDIO: CapabilityId = CapabilityId::new(CapabilityNamespace::Media, 1);
    const VIDEO: CapabilityId = CapabilityId::new(CapabilityNamespace::Media, 2);

    fn map() -> UiFeatureMap {
        let mut m = UiFeatureMap::new();
        m.require(UiFeature::Message, MSG, V1)
            .require(UiFeature::SendFile, FILES, V1)
            .require(UiFeature::AudioCall, AUDIO, V1)
            .require(UiFeature::VideoCall, AUDIO, V1)
            .require(UiFeature::VideoCall, VIDEO, V1)
            .require(UiFeature::Dtn, DTN, CapabilityVersion::new(1, 2));
        m
    }

    fn set(items: &[(CapabilityId, CapabilityVersion)]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for (id, v) in items {
            s.insert(CapabilityDescriptor::new(
                *id,
                *v,
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        }
        s
    }

    #[test]
    fn negotiated_state_is_authoritative_including_versions() {
        let s = set(&[
            (MSG, V1),
            (FILES, V1),
            (AUDIO, V1),
            (DTN, CapabilityVersion::new(1, 1)),
        ]);
        let st = ui_state(&map(), PeerKnowledge::Negotiated(&s));
        assert_eq!(st.message, UiAvailability::Available);
        assert_eq!(st.send_file, UiAvailability::Available);
        assert_eq!(st.audio_call, UiAvailability::Available);
        assert_eq!(
            st.video_call,
            UiAvailability::Unavailable,
            "needs a capability the peer lacks"
        );
        assert_eq!(
            st.dtn,
            UiAvailability::Unavailable,
            "dtn 1.1 < required 1.2"
        );
        let caps = st.to_capabilities();
        assert_eq!(
            (
                caps.can_message,
                caps.can_send_file,
                caps.can_audio_call,
                caps.can_video_call,
                caps.can_dtn
            ),
            (true, true, true, false, false)
        );
    }

    #[test]
    fn unknown_peer_keeps_everything_enabled_and_says_it_is_unverified() {
        let st = ui_state(&map(), PeerKnowledge::Unknown);
        for f in UiFeature::ALL {
            assert_eq!(st.get(f), UiAvailability::MayWork);
        }
        let c = st.to_capabilities();
        assert!(
            c.can_message && c.can_send_file && c.can_audio_call && c.can_video_call && c.can_dtn
        );
    }

    #[test]
    fn a_stale_cache_never_disables_an_action_but_a_fresh_absence_does() {
        let mut cache = CapabilityCache::new(8, StalenessPolicy::default());
        let peer = [1u8; 32];
        cache.put(
            peer,
            PeerCapabilityView::from_snapshot(CapabilityEpoch(1), set(&[(MSG, V1)])),
            0,
        );
        let fresh = ui_state(
            &map(),
            PeerKnowledge::Cached {
                cache: &cache,
                peer: &peer,
                now_millis: 1_000,
            },
        );
        assert_eq!(fresh.message, UiAvailability::Available);
        assert_eq!(
            fresh.send_file,
            UiAvailability::Unavailable,
            "fresh cache says no files"
        );
        // two hours later the entry is stale: its absence proves nothing
        let stale = ui_state(
            &map(),
            PeerKnowledge::Cached {
                cache: &cache,
                peer: &peer,
                now_millis: 2 * 3_600_000,
            },
        );
        assert_eq!(
            stale.send_file,
            UiAvailability::MayWork,
            "§58: do not over-disable on a stale cache"
        );
        assert_eq!(
            stale.message,
            UiAvailability::MayWork,
            "stale presence is only a maybe"
        );
        // an unknown peer is MayWork too
        let other = [9u8; 32];
        assert_eq!(
            ui_state(
                &map(),
                PeerKnowledge::Cached {
                    cache: &cache,
                    peer: &other,
                    now_millis: 0
                }
            )
            .send_file,
            UiAvailability::MayWork
        );
    }

    #[test]
    fn an_unmapped_feature_is_unavailable_not_silently_enabled() {
        let empty = UiFeatureMap::new();
        for k in [
            PeerKnowledge::Unknown,
            PeerKnowledge::Negotiated(&CapabilitySet::new()),
        ] {
            assert!(!ui_state(&empty, k).to_capabilities().can_message);
        }
    }

    #[test]
    fn user_text_has_no_numbers_and_stable_keys() {
        let all = [
            UserCapabilityMessage::MessagingUnavailable,
            UserCapabilityMessage::FileTransferUnsupported,
            UserCapabilityMessage::AudioCallingUnavailable,
            UserCapabilityMessage::VideoCallingUnavailable,
            UserCapabilityMessage::NearbyForwardingUnavailable,
            UserCapabilityMessage::SecureConnectionNotPossible,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(|m| m.l10n_key()).collect();
        assert_eq!(keys.len(), all.len());
        assert!(keys
            .iter()
            .all(|k| k.starts_with("capability.") && !k.chars().any(|c| c.is_ascii_digit())));
        assert_eq!(
            std::mem::size_of::<UserCapabilityMessage>(),
            1,
            "fieldless: nothing technical can be carried"
        );
        assert_eq!(
            unavailable_message(UiFeature::SendFile),
            UserCapabilityMessage::FileTransferUnsupported
        );
        assert_eq!(
            unavailable_message(UiFeature::Dtn),
            UserCapabilityMessage::NearbyForwardingUnavailable
        );
    }

    #[test]
    fn failure_messages_follow_the_failure_class() {
        let missing = CapabilityNegotiationError::MissingRequired(FILES);
        assert_eq!(
            failure_message(&missing, UiFeature::SendFile),
            Some(UserCapabilityMessage::FileTransferUnsupported)
        );
        // a security failure never reveals which feature or why
        assert_eq!(
            failure_message(
                &CapabilityNegotiationError::DowngradeSuspected,
                UiFeature::SendFile
            ),
            Some(UserCapabilityMessage::SecureConnectionNotPossible)
        );
        assert_eq!(
            failure_message(&CapabilityNegotiationError::Cancelled, UiFeature::SendFile),
            None
        );
    }

    #[test]
    fn diagnostics_carry_the_technical_detail_that_ui_text_hides() {
        let s = set(&[(DTN, CapabilityVersion::new(1, 1))]);
        let d = diagnostic_detail(&map(), UiFeature::SendFile, &s);
        assert!(d.contains("missing") && d.contains("files"), "{d}");
        let d = diagnostic_detail(&map(), UiFeature::Dtn, &s);
        assert!(d.contains("1.1") && d.contains("1.2"), "{d}");
        assert!(
            diagnostic_detail(&map(), UiFeature::Message, &set(&[(MSG, V1)]))
                .contains("all requirements met")
        );
        assert!(
            diagnostic_detail(&UiFeatureMap::new(), UiFeature::Message, &s)
                .contains("no capability mapping")
        );
    }
}
