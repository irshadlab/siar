//! §82 Offline Event Log Integration and §83 Capability Snapshot Persistence.
//!
//! `last_verified_peer_capabilities` is stored **as a cache**, marked with the
//! session it was observed in, when, and the hash of the set. Three rules keep
//! it a cache and not a source of truth:
//!
//! * it is only ever restored *into a [`CapabilityCache`]* keyed by the
//!   original observation time, so after downtime it reads as stale/unknown
//!   instead of "supported" (§51);
//! * it is integrity-checked on load (the stored hash must match the set), and
//!   rejected otherwise, but a matching hash is **not** authenticity: anyone
//!   who can write the file can forge both, so it can never back a security
//!   decision (§84);
//! * [`PersistGate`] writes only *meaningful* changes — a different set, at
//!   most once per interval — not every ephemeral capability fluctuation (§82).

use serde::{Deserialize, Serialize};

use crate::cache::{CapabilityCache, PeerKey};
use crate::descriptor::CapabilityDescriptor;
use crate::epoch::{CapabilityEpoch, PeerCapabilityView};
use crate::hash::CapabilitySetHash;
use crate::set::CapabilitySet;

/// A stored snapshot is a handful of descriptors; anything near this is corrupt or hostile.
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024;
const WIRE_VERSION: u8 = 1;

/// Opaque id of the session the set was observed in.
pub type SessionTag = [u8; 16];

#[derive(Serialize, Deserialize)]
struct Wire {
    version: u8,
    observed_session: SessionTag,
    observed_at_millis: u64,
    epoch: u64,
    hash: [u8; 32],
    descriptors: Vec<CapabilityDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot larger than {MAX_SNAPSHOT_BYTES} bytes")]
    TooLarge,
    #[error("snapshot is not valid")]
    Malformed,
    #[error("unsupported snapshot version")]
    UnsupportedVersion,
    #[error("stored hash does not match the stored capabilities")]
    HashMismatch,
    #[error("stored capabilities do not form a valid set")]
    InvalidSet,
}

#[derive(Debug, Clone)]
pub struct VerifiedPeerSnapshot {
    observed_session: SessionTag,
    observed_at_millis: u64,
    epoch: CapabilityEpoch,
    hash: CapabilitySetHash,
    set: CapabilitySet,
}

impl VerifiedPeerSnapshot {
    /// Captures what a *verified* session just negotiated or received.
    pub fn capture(session: SessionTag, now_millis: u64, view: &PeerCapabilityView) -> Self {
        Self {
            observed_session: session,
            observed_at_millis: now_millis,
            epoch: view.epoch(),
            hash: CapabilitySetHash::of(view.set()),
            set: view.set().clone(),
        }
    }

    pub fn hash(&self) -> &CapabilitySetHash {
        &self.hash
    }

    pub fn observed_session(&self) -> SessionTag {
        self.observed_session
    }

    pub fn observed_at_millis(&self) -> u64 {
        self.observed_at_millis
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, SnapshotError> {
        let wire = Wire {
            version: WIRE_VERSION,
            observed_session: self.observed_session,
            observed_at_millis: self.observed_at_millis,
            epoch: self.epoch.0,
            hash: *self.hash.as_bytes(),
            descriptors: self.set.iter().cloned().collect(),
        };
        let bytes = postcard::to_allocvec(&wire).map_err(|_| SnapshotError::Malformed)?;
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::TooLarge);
        }
        Ok(bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SnapshotError> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::TooLarge);
        }
        let (wire, rest) =
            postcard::take_from_bytes::<Wire>(bytes).map_err(|_| SnapshotError::Malformed)?;
        if !rest.is_empty() {
            return Err(SnapshotError::Malformed);
        }
        if wire.version != WIRE_VERSION {
            return Err(SnapshotError::UnsupportedVersion);
        }
        let mut set = CapabilitySet::new();
        for d in wire.descriptors {
            set.insert(d).map_err(|_| SnapshotError::InvalidSet)?;
        }
        let hash = CapabilitySetHash::of(&set);
        if hash.as_bytes() != &wire.hash {
            return Err(SnapshotError::HashMismatch);
        }
        Ok(Self {
            observed_session: wire.observed_session,
            observed_at_millis: wire.observed_at_millis,
            epoch: CapabilityEpoch(wire.epoch),
            hash,
            set,
        })
    }

    /// Restores into the cache stamped with the *original* observation time, so
    /// staleness is computed from when it was true, not from when it was loaded.
    pub fn restore_into(self, cache: &mut CapabilityCache, peer: PeerKey) {
        cache.put(
            peer,
            PeerCapabilityView::from_snapshot(self.epoch, self.set),
            self.observed_at_millis,
        );
    }
}

/// §82: decides when a snapshot is worth writing. Call [`Self::should_persist`],
/// write, and only on success call [`Self::mark_persisted`] — a failed write must
/// not suppress the retry.
#[derive(Debug, Clone)]
pub struct PersistGate {
    min_interval_millis: u64,
    last: Option<(CapabilitySetHash, u64)>,
}

impl PersistGate {
    pub fn new(min_interval_millis: u64) -> Self {
        Self {
            min_interval_millis,
            last: None,
        }
    }

    pub fn should_persist(&self, hash: &CapabilitySetHash, now_millis: u64) -> bool {
        match self.last {
            None => true,
            Some((last_hash, at)) => {
                last_hash != *hash && now_millis.saturating_sub(at) >= self.min_interval_millis
            }
        }
    }

    pub fn mark_persisted(&mut self, hash: CapabilitySetHash, now_millis: u64) {
        self.last = Some((hash, now_millis));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{StalenessPolicy, Support};
    use crate::descriptor::{CapabilityParameters, CapabilityRequirement};
    use crate::id::{CapabilityId, CapabilityNamespace};
    use crate::version::CapabilityVersion;

    const FILES: CapabilityId = CapabilityId::new(CapabilityNamespace::Files, 1);
    const DTN: CapabilityId = CapabilityId::new(CapabilityNamespace::Dtn, 1);

    fn view(ids: &[CapabilityId]) -> PeerCapabilityView {
        let mut s = CapabilitySet::new();
        for id in ids {
            s.insert(CapabilityDescriptor::new(
                *id,
                CapabilityVersion::new(1, 0),
                CapabilityRequirement::Optional,
                CapabilityParameters::U32(7),
            ))
            .unwrap();
        }
        PeerCapabilityView::from_snapshot(CapabilityEpoch(3), s)
    }

    fn snap(ids: &[CapabilityId]) -> VerifiedPeerSnapshot {
        VerifiedPeerSnapshot::capture([9; 16], 1_000, &view(ids))
    }

    #[test]
    fn round_trips_with_its_provenance() {
        let s = snap(&[FILES, DTN]);
        let back = VerifiedPeerSnapshot::from_bytes(&s.to_bytes().unwrap()).unwrap();
        assert_eq!(back.hash(), s.hash());
        assert_eq!(back.observed_session(), [9; 16]);
        assert_eq!(back.observed_at_millis(), 1_000);
    }

    #[test]
    fn a_forged_hash_oversize_trailing_bytes_and_garbage_are_rejected() {
        let good = snap(&[FILES]).to_bytes().unwrap();

        // The hash sits right after version(1) + session(16) + millis(varint) + epoch(varint):
        // flip a byte inside it by locating the real hash bytes.
        let h = *snap(&[FILES]).hash().as_bytes();
        let pos = good.windows(32).position(|w| w == h).unwrap();
        let mut forged = good.clone();
        forged[pos] ^= 0xff;
        assert_eq!(
            VerifiedPeerSnapshot::from_bytes(&forged).unwrap_err(),
            SnapshotError::HashMismatch
        );

        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(
            VerifiedPeerSnapshot::from_bytes(&trailing).unwrap_err(),
            SnapshotError::Malformed
        );
        assert_eq!(
            VerifiedPeerSnapshot::from_bytes(&vec![0u8; MAX_SNAPSHOT_BYTES + 1]).unwrap_err(),
            SnapshotError::TooLarge
        );
        assert!(VerifiedPeerSnapshot::from_bytes(&[0xff; 12]).is_err());

        let mut future_version = good;
        future_version[0] = 2;
        assert_eq!(
            VerifiedPeerSnapshot::from_bytes(&future_version).unwrap_err(),
            SnapshotError::UnsupportedVersion
        );
    }

    #[test]
    fn restoring_keeps_it_a_cache_staleness_counts_from_the_original_observation() {
        let policy = StalenessPolicy {
            fresh_for_millis: 5_000,
            stale_for_millis: 60_000,
        };
        let peer = [1u8; 32];
        let load = |now| {
            let mut cache = CapabilityCache::new(8, policy);
            snap(&[FILES]).restore_into(&mut cache, peer);
            (
                cache.supports(&peer, &FILES, now),
                cache.supports(&peer, &DTN, now),
            )
        };
        // Observed at t=1000: still fresh shortly after, "maybe" later, unknown much later.
        assert_eq!(load(2_000), (Support::Yes, Support::No));
        assert_eq!(load(30_000).0, Support::MaybeStale);
        assert_eq!(load(1_000_000).0, Support::Unknown);
    }

    #[test]
    fn only_meaningful_changes_are_persisted_and_not_more_often_than_the_interval() {
        let (a, b) = (*snap(&[FILES]).hash(), *snap(&[FILES, DTN]).hash());
        let mut gate = PersistGate::new(10_000);
        assert!(gate.should_persist(&a, 0), "first observation is written");
        gate.mark_persisted(a, 0);
        assert!(
            !gate.should_persist(&a, 99_999),
            "unchanged set is never rewritten"
        );
        assert!(
            !gate.should_persist(&b, 5_000),
            "a change inside the interval waits"
        );
        assert!(
            gate.should_persist(&b, 10_000),
            "…and is written once the interval has passed"
        );
        // A failed write (no mark_persisted) is retried, not suppressed.
        assert!(gate.should_persist(&b, 10_001));
        gate.mark_persisted(b, 10_001);
        // Flapping back to a previously persisted set is still a change from the *last* one.
        assert!(gate.should_persist(&a, 30_000));
    }
}
