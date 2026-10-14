//! §82 "Derivatives", §83 "Thumbnail Policy", §84 "Voice Note
//! Policy", §85 "Video Attachment vs Video Call".
//!
//! ## §82: relationships live in metadata, never in identity
//!
//! "Represent relationships in metadata, not inside storage identity."
//! A thumbnail is **its own blob** — own [`BlobId`] (the hash of *its*
//! ciphertext), own [`crate::schema_rows::BlobRow`], own references —
//! and a [`BlobDerivative`] row is only the *link*. Two consequences
//! fall out and are tested:
//!
//! - **Linking changes nothing about either blob.** There is no way for
//!   a relationship to alter a `BlobId`, because a `BlobId` is computed
//!   from bytes and this module never touches bytes.
//! - **A derivative can have more than one source.** Content addressing
//!   means two different photos whose thumbnails happen to be
//!   byte-identical produce *one* thumbnail blob. So the link is keyed
//!   `(derivative, source, kind)` — "the same derivative of two
//!   originals" is two links to one blob, and removing one original's
//!   links leaves the other's intact.
//!
//! Lifetime is the reference model's job
//! ([`crate::reference_store`]): a derivative stays alive because
//! something holds a reference to it ([`role_for`] says which role), not
//! because a link row exists. Deleting a link never deletes a blob.
//!
//! ## What derivatives are *for* (§82's purpose)
//!
//! "Faster conversation rendering and mesh transfer." [`choose_representation`]
//! is that decision as a pure function: given what exists and how much a
//! link can carry, which blob should be sent or shown? It prefers the
//! smallest representation that still satisfies the need, and when
//! nothing adequate fits the budget it returns a *degraded* lower
//! fidelity one — flagged, so the UI can say "preview" rather than
//! pretend it is the original — rather than failing to render at all.
//!
//! ## §83 thumbnails, §84 voice notes
//!
//! Thumbnail *generation* is a media-codec concern (decoding untrusted
//! images is its own attack surface — §113-117's safe-rendering rules)
//! and is not here; what this module owns is the policy skeleton that
//! doesn't need a codec: a thumbnail is a derivative, it is *advisory
//! metadata for UI*, and it never substitutes for the original in an
//! integrity decision.
//!
//! Voice notes (§84) are ordinary binary blobs ("Opus frames"); the
//! only extra data is [`VoiceNoteMeta`] — duration and a waveform for
//! the player's scrubber — which is metadata, strictly bounded because
//! it arrives from the network ([`MAX_WAVEFORM_SAMPLES`],
//! [`MAX_VOICE_DURATION_MS`]).
//!
//! ## §85: video attachment is a blob; video *call* is not
//!
//! "Video message / file attachment → blob subsystem. Live video call →
//! realtime media subsystem." [`MediaChannel`] makes that a closed
//! choice, and [`channel_for_attachment`] is a total function over
//! [`siar_domain::MediaType`] that returns
//! [`MediaChannel::BlobTransfer`] for **every** variant — including
//! `VideoMp4`. There is no attachment media type that routes to the
//! realtime path, so a recorded video can never be mistaken for a call
//! stream (or vice versa) by a code path that only has the media type
//! to go on.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use siar_domain::MediaType;
use stoolap::Database;

use crate::ids::BlobId;
use crate::reference_store::BlobRole;
use crate::schema_rows::{BlobDerivative, DerivativeKind};
use crate::store_error::BlobError;

fn kind_code(k: DerivativeKind) -> i64 {
    match k {
        DerivativeKind::Thumbnail => 0,
        DerivativeKind::Transcode => 1,
        DerivativeKind::Preview => 2,
        DerivativeKind::Waveform => 3,
    }
}

fn kind_from(c: i64) -> Option<DerivativeKind> {
    match c {
        0 => Some(DerivativeKind::Thumbnail),
        1 => Some(DerivativeKind::Transcode),
        2 => Some(DerivativeKind::Preview),
        3 => Some(DerivativeKind::Waveform),
        _ => None,
    }
}

/// The relationship index. Synchronous, like [`crate::metadata_store`].
pub trait DerivativeIndex: Send + Sync {
    /// `true` if the link is new.
    fn link(&self, d: BlobDerivative) -> Result<bool, BlobError>;
    /// `true` if the link existed.
    fn unlink(&self, d: BlobDerivative) -> Result<bool, BlobError>;
    fn derivatives_of(&self, source: BlobId) -> Result<Vec<BlobDerivative>, BlobError>;
    /// Every `(source, kind)` this blob is a derivative of.
    fn sources_of(&self, derivative: BlobId) -> Result<Vec<BlobDerivative>, BlobError>;
    /// Removes every link whose **source** is `source`; returns how
    /// many. Never touches any blob.
    fn remove_for_source(&self, source: BlobId) -> Result<u64, BlobError>;
}

fn key(d: &BlobDerivative) -> LinkKey {
    (
        *d.derivative_blob_id.as_bytes(),
        *d.source_blob_id.as_bytes(),
        kind_code(d.kind),
    )
}

/// `(derivative, source, kind code)` — the uniqueness key of a link.
type LinkKey = ([u8; 32], [u8; 32], i64);

#[derive(Debug, Default)]
pub struct InMemoryDerivativeIndex {
    links: Mutex<BTreeSet<LinkKey>>,
}

impl InMemoryDerivativeIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

fn from_key(k: LinkKey) -> Option<BlobDerivative> {
    Some(BlobDerivative {
        derivative_blob_id: BlobId::from_raw_digest(k.0),
        source_blob_id: BlobId::from_raw_digest(k.1),
        kind: kind_from(k.2)?,
    })
}

impl DerivativeIndex for InMemoryDerivativeIndex {
    fn link(&self, d: BlobDerivative) -> Result<bool, BlobError> {
        Ok(self.links.lock().expect("index poisoned").insert(key(&d)))
    }
    fn unlink(&self, d: BlobDerivative) -> Result<bool, BlobError> {
        Ok(self.links.lock().expect("index poisoned").remove(&key(&d)))
    }
    fn derivatives_of(&self, source: BlobId) -> Result<Vec<BlobDerivative>, BlobError> {
        let l = self.links.lock().expect("index poisoned");
        Ok(l.iter()
            .filter(|k| &k.1 == source.as_bytes())
            .filter_map(|k| from_key(*k))
            .collect())
    }
    fn sources_of(&self, derivative: BlobId) -> Result<Vec<BlobDerivative>, BlobError> {
        let l = self.links.lock().expect("index poisoned");
        Ok(l.iter()
            .filter(|k| &k.0 == derivative.as_bytes())
            .filter_map(|k| from_key(*k))
            .collect())
    }
    fn remove_for_source(&self, source: BlobId) -> Result<u64, BlobError> {
        let mut l = self.links.lock().expect("index poisoned");
        let before = l.len();
        l.retain(|k| &k.1 != source.as_bytes());
        Ok((before - l.len()) as u64)
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn unhex(s: &str) -> Result<[u8; 32], BlobError> {
    if s.len() != 64 {
        return Err(BlobError::Metadata("stored id has wrong length".into()));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(me)?;
    }
    Ok(out)
}

fn me(e: impl std::fmt::Display) -> BlobError {
    BlobError::Metadata(e.to_string())
}

/// Durable relationships on `stoolap`.
pub struct StoolapDerivativeIndex {
    db: Database,
    lock: Arc<Mutex<()>>,
}

impl StoolapDerivativeIndex {
    pub fn open(path: &str) -> Result<Self, BlobError> {
        Self::from_db(Database::open(&format!("file://{path}")).map_err(me)?)
    }

    pub fn open_in_memory() -> Result<Self, BlobError> {
        Self::from_db(Database::open_in_memory().map_err(me)?)
    }

    fn from_db(db: Database) -> Result<Self, BlobError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS blob_derivative_links (
                derivative TEXT NOT NULL, source TEXT NOT NULL, kind INTEGER NOT NULL)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_bdl_tuple
             ON blob_derivative_links (derivative, source, kind)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_bdl_source ON blob_derivative_links (source)",
            (),
        )
        .map_err(me)?;
        Ok(Self {
            db,
            lock: Arc::new(Mutex::new(())),
        })
    }

    fn collect(
        rows: impl Iterator<Item = Result<stoolap::ResultRow, stoolap::Error>>,
    ) -> Result<Vec<BlobDerivative>, BlobError> {
        let mut out = Vec::new();
        for r in rows {
            let r = r.map_err(me)?;
            let d: String = r.get(0).map_err(me)?;
            let s: String = r.get(1).map_err(me)?;
            let k: i64 = r.get(2).map_err(me)?;
            out.push(BlobDerivative {
                derivative_blob_id: BlobId::from_raw_digest(unhex(&d)?),
                source_blob_id: BlobId::from_raw_digest(unhex(&s)?),
                kind: kind_from(k).ok_or_else(|| me("unknown stored kind"))?,
            });
        }
        Ok(out)
    }
}

impl DerivativeIndex for StoolapDerivativeIndex {
    fn link(&self, d: BlobDerivative) -> Result<bool, BlobError> {
        let _g = self.lock.lock().expect("index poisoned");
        let p = (
            hex(d.derivative_blob_id.as_bytes()),
            hex(d.source_blob_id.as_bytes()),
            kind_code(d.kind),
        );
        let exists = self
            .db
            .query(
                "SELECT 1 FROM blob_derivative_links WHERE derivative = $1 AND source = $2 AND kind = $3",
                p.clone(),
            )
            .map_err(me)?
            .next()
            .is_some();
        if exists {
            return Ok(false);
        }
        self.db
            .execute(
                "INSERT INTO blob_derivative_links (derivative, source, kind) VALUES ($1, $2, $3)",
                p,
            )
            .map_err(me)?;
        Ok(true)
    }

    fn unlink(&self, d: BlobDerivative) -> Result<bool, BlobError> {
        let _g = self.lock.lock().expect("index poisoned");
        let n = self
            .db
            .execute(
                "DELETE FROM blob_derivative_links WHERE derivative = $1 AND source = $2 AND kind = $3",
                (hex(d.derivative_blob_id.as_bytes()), hex(d.source_blob_id.as_bytes()), kind_code(d.kind)),
            )
            .map_err(me)?;
        Ok(n > 0)
    }

    fn derivatives_of(&self, source: BlobId) -> Result<Vec<BlobDerivative>, BlobError> {
        let _g = self.lock.lock().expect("index poisoned");
        let rows = self
            .db
            .query(
                "SELECT derivative, source, kind FROM blob_derivative_links
                 WHERE source = $1 ORDER BY derivative ASC, kind ASC",
                (hex(source.as_bytes()),),
            )
            .map_err(me)?;
        Self::collect(rows)
    }

    fn sources_of(&self, derivative: BlobId) -> Result<Vec<BlobDerivative>, BlobError> {
        let _g = self.lock.lock().expect("index poisoned");
        let rows = self
            .db
            .query(
                "SELECT derivative, source, kind FROM blob_derivative_links
                 WHERE derivative = $1 ORDER BY source ASC, kind ASC",
                (hex(derivative.as_bytes()),),
            )
            .map_err(me)?;
        Self::collect(rows)
    }

    fn remove_for_source(&self, source: BlobId) -> Result<u64, BlobError> {
        let _g = self.lock.lock().expect("index poisoned");
        let n = self
            .db
            .execute(
                "DELETE FROM blob_derivative_links WHERE source = $1",
                (hex(source.as_bytes()),),
            )
            .map_err(me)?;
        Ok(n as u64)
    }
}

/// Which reference role keeps a derivative of this kind alive.
pub fn role_for(kind: DerivativeKind) -> BlobRole {
    match kind {
        DerivativeKind::Thumbnail | DerivativeKind::Preview | DerivativeKind::Waveform => {
            BlobRole::Thumbnail
        }
        // A transcode is a full alternate rendition, not a UI aid.
        DerivativeKind::Transcode => BlobRole::MessageAttachment,
    }
}

/// What the UI/transfer layer needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderNeed {
    /// A conversation-list thumbnail.
    ListThumbnail,
    /// A larger inline preview.
    Preview,
    /// The real thing.
    Full,
}

impl RenderNeed {
    const fn rank(self) -> u8 {
        match self {
            Self::ListThumbnail => 1,
            Self::Preview => 2,
            Self::Full => 4,
        }
    }
}

/// One thing that could be sent/shown. `kind == None` is the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub blob_id: BlobId,
    pub kind: Option<DerivativeKind>,
    pub size: u64,
}

impl Candidate {
    /// Fidelity rank; `None` for kinds that are not visual
    /// representations (a waveform is not a smaller picture).
    fn rank(&self) -> Option<u8> {
        match self.kind {
            Some(DerivativeKind::Thumbnail) => Some(1),
            Some(DerivativeKind::Preview) => Some(2),
            Some(DerivativeKind::Transcode) => Some(3),
            Some(DerivativeKind::Waveform) => None,
            None => Some(4),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub candidate: Candidate,
    /// The best that fit was *below* what was needed.
    pub degraded: bool,
}

/// See the module doc. `budget_bytes`: the most the link/screen can
/// take (`None` = unbounded).
pub fn choose_representation(
    candidates: &[Candidate],
    need: RenderNeed,
    budget_bytes: Option<u64>,
) -> Option<Choice> {
    let fits = |c: &&Candidate| budget_bytes.is_none_or(|b| c.size <= b);
    let ranked: Vec<(&Candidate, u8)> = candidates
        .iter()
        .filter(fits)
        .filter_map(|c| c.rank().map(|r| (c, r)))
        .collect();

    // 1. Smallest adequate representation (rank >= need).
    if let Some((c, _)) = ranked
        .iter()
        .filter(|(_, r)| *r >= need.rank())
        .min_by_key(|(c, r)| (*r, c.size))
    {
        return Some(Choice {
            candidate: **c,
            degraded: false,
        });
    }
    // 2. Otherwise the best inadequate one that fits, flagged.
    ranked
        .iter()
        .max_by_key(|(c, r)| (*r, std::cmp::Reverse(c.size)))
        .map(|(c, _)| Choice {
            candidate: **c,
            degraded: true,
        })
}

/// Longest voice note this subsystem will carry metadata for.
pub const MAX_VOICE_DURATION_MS: u32 = 30 * 60 * 1000;
/// Waveform resolution cap: one byte of amplitude per sample.
pub const MAX_WAVEFORM_SAMPLES: usize = 256;
/// Encoded [`VoiceNoteMeta`] is checked against this *before* decoding.
pub const MAX_VOICE_META_BYTES: usize = MAX_WAVEFORM_SAMPLES + 16;

/// §84's only extra data for a voice note; the audio itself is an
/// ordinary blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceNoteMeta {
    pub duration_ms: u32,
    pub waveform: Vec<u8>,
}

impl VoiceNoteMeta {
    pub fn new(duration_ms: u32, waveform: Vec<u8>) -> Result<Self, BlobError> {
        let m = Self {
            duration_ms,
            waveform,
        };
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), BlobError> {
        if self.duration_ms == 0 || self.duration_ms > MAX_VOICE_DURATION_MS {
            return Err(BlobError::InvalidArgument(
                "voice note duration out of range",
            ));
        }
        if self.waveform.len() > MAX_WAVEFORM_SAMPLES {
            return Err(BlobError::InvalidArgument("waveform has too many samples"));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, BlobError> {
        self.validate()?;
        postcard::to_allocvec(self)
            .map_err(|_| BlobError::InvalidArgument("cannot encode voice metadata"))
    }

    /// Untrusted input: length-checked first, then decoded, then
    /// validated — an oversized or out-of-range value never gets past.
    pub fn decode(bytes: &[u8]) -> Result<Self, BlobError> {
        if bytes.len() > MAX_VOICE_META_BYTES {
            return Err(BlobError::InvalidArgument("voice metadata too large"));
        }
        let m: Self = postcard::from_bytes(bytes)
            .map_err(|_| BlobError::InvalidArgument("voice metadata is malformed"))?;
        m.validate()?;
        Ok(m)
    }
}

/// §85's closed choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaChannel {
    /// The blob subsystem: store, chunk, address, transfer, GC.
    BlobTransfer,
    /// The realtime media subsystem: latency-bound, never stored as a
    /// blob.
    RealtimeMedia,
}

/// Total over [`MediaType`], and `BlobTransfer` for all of it. A video
/// *file or message* is an attachment, whatever its codec.
pub fn channel_for_attachment(media: MediaType) -> MediaChannel {
    match media {
        MediaType::ImagePng
        | MediaType::ImageJpeg
        | MediaType::ImageWebp
        | MediaType::AudioOpus
        | MediaType::VideoMp4
        | MediaType::Other => MediaChannel::BlobTransfer,
    }
}

/// A live call's media. Distinct entry point so the two paths cannot be
/// confused by passing a media type around.
pub const fn channel_for_live_call() -> MediaChannel {
    MediaChannel::RealtimeMedia
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }
    fn link(d: u8, s: u8, k: DerivativeKind) -> BlobDerivative {
        BlobDerivative {
            derivative_blob_id: b(d),
            source_blob_id: b(s),
            kind: k,
        }
    }

    fn contract<I: DerivativeIndex>(i: &I) {
        let t = link(2, 1, DerivativeKind::Thumbnail);
        assert!(i.link(t).unwrap());
        assert!(!i.link(t).unwrap(), "idempotent");
        assert!(i.link(link(3, 1, DerivativeKind::Preview)).unwrap());
        assert_eq!(i.derivatives_of(b(1)).unwrap().len(), 2);
        assert_eq!(i.sources_of(b(2)).unwrap(), vec![t]);

        // The SAME derivative blob (byte-identical thumbnail) of a
        // second original: two links, one blob.
        let shared = link(2, 9, DerivativeKind::Thumbnail);
        assert!(i.link(shared).unwrap());
        assert_eq!(i.sources_of(b(2)).unwrap().len(), 2);

        // Removing one original's links leaves the other's.
        assert_eq!(i.remove_for_source(b(1)).unwrap(), 2);
        assert!(i.derivatives_of(b(1)).unwrap().is_empty());
        assert_eq!(i.sources_of(b(2)).unwrap(), vec![shared]);

        assert!(i.unlink(shared).unwrap());
        assert!(!i.unlink(shared).unwrap());
        assert!(i.sources_of(b(2)).unwrap().is_empty());
    }

    #[test]
    fn in_memory_contract() {
        contract(&InMemoryDerivativeIndex::new());
    }

    #[test]
    fn stoolap_contract() {
        contract(&StoolapDerivativeIndex::open_in_memory().unwrap());
    }

    #[test]
    fn links_survive_reopen() {
        let path = std::env::temp_dir().join(format!("siar-deriv-{}", uuid::Uuid::new_v4()));
        let p = path.to_str().unwrap();
        {
            StoolapDerivativeIndex::open(p)
                .unwrap()
                .link(link(2, 1, DerivativeKind::Waveform))
                .unwrap();
        }
        assert_eq!(
            StoolapDerivativeIndex::open(p)
                .unwrap()
                .derivatives_of(b(1))
                .unwrap()
                .len(),
            1
        );
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_relationship_cannot_change_either_blobs_identity() {
        // Identity is a function of bytes alone; the index never sees
        // bytes, so nothing it does can move a BlobId.
        let (orig, thumb) = (
            BlobId::from_ciphertext(b"original"),
            BlobId::from_ciphertext(b"thumb"),
        );
        let idx = InMemoryDerivativeIndex::new();
        idx.link(BlobDerivative {
            derivative_blob_id: thumb,
            source_blob_id: orig,
            kind: DerivativeKind::Thumbnail,
        })
        .unwrap();
        assert_eq!(orig, BlobId::from_ciphertext(b"original"));
        assert_eq!(thumb, BlobId::from_ciphertext(b"thumb"));
        assert_ne!(orig, thumb);
    }

    #[test]
    fn reference_roles_follow_the_kind() {
        assert_eq!(role_for(DerivativeKind::Thumbnail), BlobRole::Thumbnail);
        assert_eq!(role_for(DerivativeKind::Waveform), BlobRole::Thumbnail);
        assert_eq!(
            role_for(DerivativeKind::Transcode),
            BlobRole::MessageAttachment
        );
    }

    fn cand(n: u8, kind: Option<DerivativeKind>, size: u64) -> Candidate {
        Candidate {
            blob_id: b(n),
            kind,
            size,
        }
    }

    #[test]
    fn the_smallest_adequate_representation_wins() {
        let all = [
            cand(1, None, 5_000_000),
            cand(2, Some(DerivativeKind::Thumbnail), 8_000),
            cand(3, Some(DerivativeKind::Preview), 120_000),
        ];
        let pick = |need, budget| {
            choose_representation(&all, need, budget).map(|c| (c.candidate.blob_id, c.degraded))
        };
        assert_eq!(pick(RenderNeed::ListThumbnail, None), Some((b(2), false)));
        assert_eq!(pick(RenderNeed::Preview, None), Some((b(3), false)));
        assert_eq!(pick(RenderNeed::Full, None), Some((b(1), false)));
    }

    #[test]
    fn a_small_budget_degrades_gracefully_and_says_so() {
        let all = [
            cand(1, None, 5_000_000),
            cand(2, Some(DerivativeKind::Thumbnail), 8_000),
            cand(3, Some(DerivativeKind::Preview), 120_000),
        ];
        // Mesh link can carry 200 KB: Full is impossible; Preview is the
        // best that fits — flagged as degraded.
        let c = choose_representation(&all, RenderNeed::Full, Some(200_000)).unwrap();
        assert_eq!((c.candidate.blob_id, c.degraded), (b(3), true));
        // 10 KB: only the thumbnail.
        let c = choose_representation(&all, RenderNeed::Preview, Some(10_000)).unwrap();
        assert_eq!((c.candidate.blob_id, c.degraded), (b(2), true));
        // Nothing fits at all.
        assert_eq!(
            choose_representation(&all, RenderNeed::Full, Some(100)),
            None
        );
    }

    #[test]
    fn missing_derivatives_fall_back_up_to_the_original_when_it_fits() {
        let only_original = [cand(1, None, 40_000)];
        let c = choose_representation(&only_original, RenderNeed::ListThumbnail, Some(100_000))
            .unwrap();
        assert_eq!(
            (c.candidate.kind, c.degraded),
            (None, false),
            "bigger than needed is adequate, not degraded"
        );
        assert_eq!(
            choose_representation(&only_original, RenderNeed::ListThumbnail, Some(1_000)),
            None
        );
        assert_eq!(choose_representation(&[], RenderNeed::Full, None), None);
    }

    #[test]
    fn a_waveform_is_never_chosen_as_a_picture() {
        let all = [
            cand(1, Some(DerivativeKind::Waveform), 100),
            cand(2, None, 50_000),
        ];
        let c = choose_representation(&all, RenderNeed::ListThumbnail, None).unwrap();
        assert_eq!(c.candidate.blob_id, b(2));
        assert_eq!(
            choose_representation(&all[..1], RenderNeed::ListThumbnail, None),
            None
        );
    }

    #[test]
    fn equal_fidelity_prefers_the_smaller() {
        let all = [
            cand(1, Some(DerivativeKind::Thumbnail), 9_000),
            cand(2, Some(DerivativeKind::Thumbnail), 4_000),
        ];
        assert_eq!(
            choose_representation(&all, RenderNeed::ListThumbnail, None)
                .unwrap()
                .candidate
                .blob_id,
            b(2)
        );
    }

    #[test]
    fn voice_metadata_is_bounded_in_every_direction() {
        let ok = VoiceNoteMeta::new(12_500, vec![10; 64]).unwrap();
        assert_eq!(VoiceNoteMeta::decode(&ok.encode().unwrap()).unwrap(), ok);

        assert!(VoiceNoteMeta::new(0, vec![]).is_err());
        assert!(VoiceNoteMeta::new(MAX_VOICE_DURATION_MS + 1, vec![]).is_err());
        assert!(VoiceNoteMeta::new(1_000, vec![0; MAX_WAVEFORM_SAMPLES + 1]).is_err());
        assert!(VoiceNoteMeta::new(MAX_VOICE_DURATION_MS, vec![0; MAX_WAVEFORM_SAMPLES]).is_ok());

        // Hostile wire input: oversized, garbage, and a value that
        // decodes but is out of range.
        assert!(VoiceNoteMeta::decode(&vec![0u8; MAX_VOICE_META_BYTES + 1]).is_err());
        assert!(VoiceNoteMeta::decode(&[0xFF; 8]).is_err());
        let bad = postcard::to_allocvec(&VoiceNoteMeta {
            duration_ms: u32::MAX,
            waveform: vec![],
        })
        .unwrap();
        assert!(VoiceNoteMeta::decode(&bad).is_err());
    }

    #[test]
    fn every_attachment_media_type_is_a_blob_and_a_call_is_not() {
        for m in [
            MediaType::ImagePng,
            MediaType::ImageJpeg,
            MediaType::ImageWebp,
            MediaType::AudioOpus,
            MediaType::VideoMp4,
            MediaType::Other,
        ] {
            assert_eq!(
                channel_for_attachment(m),
                MediaChannel::BlobTransfer,
                "{m:?}"
            );
        }
        assert_eq!(
            channel_for_attachment(MediaType::VideoMp4),
            MediaChannel::BlobTransfer
        );
        assert_eq!(channel_for_live_call(), MediaChannel::RealtimeMedia);
        assert_ne!(
            channel_for_attachment(MediaType::VideoMp4),
            channel_for_live_call()
        );
    }
}
