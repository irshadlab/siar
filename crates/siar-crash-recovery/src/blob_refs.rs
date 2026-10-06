//! §25 "Blob Reference Recovery" and §26 "Blob GC Crash Safety".
//!
//! §25: reference counts are *derived*. When they look inconsistent,
//! rebuild from the authoritative references; never trust a cached
//! count blindly. §26: GC must be staged (mark → recheck references →
//! delete object → delete metadata) and recovery reconciles whatever
//! partial state a crash left.
//!
//! Design choice for §26: recovery decides from **observed facts**
//! (authoritative reference count, does the object exist, does the
//! metadata exist), not from the persisted [`GcStage`]. The stage
//! only says where the previous run *believed* it was; a crash can
//! land between a side effect and its stage write in either order, and
//! a stage that claims "rechecked" is stale the moment a new reference
//! arrives. This is the same "do not assume" stance this crate's §20
//! and §21 modules take.

use serde::{Deserialize, Serialize};
use siar_blob_manifest::ids::BlobId;
use std::collections::HashMap;

/// Derived reference counts, keyed by blob.
pub type RefCounts = HashMap<BlobId, u64>;

/// §25: rebuild counts from the authoritative references — one item
/// per reference (a manifest entry, a message attachment, …).
pub fn rebuild_ref_counts<I: IntoIterator<Item = BlobId>>(authoritative: I) -> RefCounts {
    let mut out = RefCounts::new();
    for b in authoritative {
        *out.entry(b).or_insert(0) += 1;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriftKind {
    /// Cache says more references than really exist (leak: blob lives
    /// too long; harmless to safety).
    Overcounted,
    /// Cache says fewer than really exist (dangerous: GC could delete
    /// a live blob).
    Undercounted,
    /// A live blob has no cache entry at all.
    MissingEntry,
    /// Cache has an entry for a blob with zero real references.
    StaleEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefCountDrift {
    pub blob: BlobId,
    pub cached: Option<u64>,
    pub authoritative: u64,
    pub kind: DriftKind,
}

impl RefCountDrift {
    /// Whether trusting the cache here could cause data loss.
    pub fn is_dangerous(&self) -> bool {
        matches!(self.kind, DriftKind::Undercounted | DriftKind::MissingEntry)
    }
}

/// Compares the cache to the rebuilt truth. Consistent entries yield
/// nothing; output order is unspecified.
pub fn audit_ref_counts(cached: &RefCounts, authoritative: &RefCounts) -> Vec<RefCountDrift> {
    let mut out = Vec::new();
    for (&blob, &auth) in authoritative {
        match cached.get(&blob) {
            None => out.push(RefCountDrift {
                blob,
                cached: None,
                authoritative: auth,
                kind: DriftKind::MissingEntry,
            }),
            Some(&c) if c > auth => out.push(RefCountDrift {
                blob,
                cached: Some(c),
                authoritative: auth,
                kind: DriftKind::Overcounted,
            }),
            Some(&c) if c < auth => out.push(RefCountDrift {
                blob,
                cached: Some(c),
                authoritative: auth,
                kind: DriftKind::Undercounted,
            }),
            Some(_) => {}
        }
    }
    for (&blob, &c) in cached {
        if !authoritative.contains_key(&blob) {
            out.push(RefCountDrift {
                blob,
                cached: Some(c),
                authoritative: 0,
                kind: DriftKind::StaleEntry,
            });
        }
    }
    out
}

/// Replaces the cache with the authoritative counts (the §25 repair).
/// Returns how many entries changed, were added or were removed.
pub fn repair_ref_counts(cached: &mut RefCounts, authoritative: &RefCounts) -> usize {
    let drift = audit_ref_counts(cached, authoritative).len();
    if drift != 0 {
        *cached = authoritative.clone();
    }
    drift
}

/// The count GC may act on. Takes only the authoritative table so the
/// cache cannot be passed by mistake.
#[inline]
pub fn authoritative_count(authoritative: &RefCounts, blob: &BlobId) -> u64 {
    authoritative.get(blob).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------
// §26
// ---------------------------------------------------------------------

/// §26's persisted stages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GcStage {
    Marked,
    Rechecked,
    ObjectDeleted,
    MetadataDeleted,
}

pub fn can_transition(from: GcStage, to: GcStage) -> bool {
    use GcStage::*;
    matches!(
        (from, to),
        (Marked, Rechecked) | (Rechecked, ObjectDeleted) | (ObjectDeleted, MetadataDeleted)
    )
}

/// What recovery observed for one GC candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcFacts {
    /// From the *rebuilt* table ([`authoritative_count`]).
    pub authoritative_refs: u64,
    pub object_exists: bool,
    pub metadata_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GcRecoveryAction {
    /// Someone referenced the blob after it was marked: unmark it, do
    /// not delete.
    AbortKeepLive,
    /// Live blob whose metadata was deleted prematurely: rebuild the
    /// metadata from the object, keep both.
    RepairMetadataKeepLive,
    /// A reference exists but the object is gone. This is data loss,
    /// not a GC state — hand to single-blob corruption handling (§53).
    ReferencedObjectMissing,
    /// Still unreferenced with the object present: re-run the
    /// reference recheck (never trust a persisted "rechecked"), then
    /// delete the object, then the metadata.
    RecheckThenDelete,
    /// Object already gone (crash after object delete): finish by
    /// deleting the metadata.
    DeleteMetadata,
    /// Nothing left to do: drop the GC record.
    Complete,
}

/// Exhaustive 2×2×2 decision over [`GcFacts`].
pub fn reconcile_gc(facts: GcFacts) -> GcRecoveryAction {
    use GcRecoveryAction::*;
    match (
        facts.authoritative_refs > 0,
        facts.object_exists,
        facts.metadata_exists,
    ) {
        (true, true, true) => AbortKeepLive,
        (true, true, false) => RepairMetadataKeepLive,
        (true, false, _) => ReferencedObjectMissing,
        (false, true, _) => RecheckThenDelete,
        (false, false, true) => DeleteMetadata,
        (false, false, false) => Complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }

    #[test]
    fn rebuild_counts_each_reference() {
        let t = rebuild_ref_counts([blob(1), blob(1), blob(2)]);
        assert_eq!(t[&blob(1)], 2);
        assert_eq!(t[&blob(2)], 1);
        assert_eq!(authoritative_count(&t, &blob(9)), 0);
    }

    #[test]
    fn audit_classifies_all_four_drift_kinds() {
        let auth = rebuild_ref_counts([blob(1), blob(1), blob(2), blob(3)]);
        let mut cached = RefCounts::new();
        cached.insert(blob(1), 1); // under
        cached.insert(blob(2), 5); // over
        cached.insert(blob(4), 2); // stale; blob 3 missing
        let drift = audit_ref_counts(&cached, &auth);
        let kind = |b| drift.iter().find(|d| d.blob == b).unwrap().kind;
        assert_eq!(kind(blob(1)), DriftKind::Undercounted);
        assert_eq!(kind(blob(2)), DriftKind::Overcounted);
        assert_eq!(kind(blob(3)), DriftKind::MissingEntry);
        assert_eq!(kind(blob(4)), DriftKind::StaleEntry);
        assert_eq!(drift.len(), 4);
        let dangerous: Vec<_> = drift.iter().filter(|d| d.is_dangerous()).collect();
        assert_eq!(dangerous.len(), 2);
    }

    #[test]
    fn consistent_cache_has_no_drift_and_repair_is_a_noop() {
        let auth = rebuild_ref_counts([blob(1), blob(2)]);
        let mut cached = auth.clone();
        assert!(audit_ref_counts(&cached, &auth).is_empty());
        assert_eq!(repair_ref_counts(&mut cached, &auth), 0);
    }

    #[test]
    fn repair_makes_cache_equal_authoritative_and_is_idempotent() {
        let auth = rebuild_ref_counts([blob(1), blob(1)]);
        let mut cached = RefCounts::new();
        cached.insert(blob(1), 0);
        cached.insert(blob(7), 3);
        assert_eq!(repair_ref_counts(&mut cached, &auth), 2);
        assert_eq!(cached, auth);
        assert_eq!(repair_ref_counts(&mut cached, &auth), 0);
    }

    #[test]
    fn gc_stages_are_linear() {
        use GcStage::*;
        assert!(can_transition(Marked, Rechecked));
        assert!(can_transition(Rechecked, ObjectDeleted));
        assert!(can_transition(ObjectDeleted, MetadataDeleted));
        assert!(!can_transition(Marked, ObjectDeleted));
        assert!(!can_transition(MetadataDeleted, Marked));
    }

    #[test]
    fn gc_reconcile_covers_every_fact_combination() {
        use GcRecoveryAction::*;
        let f = |r, o, m| {
            reconcile_gc(GcFacts {
                authoritative_refs: r,
                object_exists: o,
                metadata_exists: m,
            })
        };
        assert_eq!(f(1, true, true), AbortKeepLive);
        assert_eq!(f(1, true, false), RepairMetadataKeepLive);
        assert_eq!(f(1, false, true), ReferencedObjectMissing);
        assert_eq!(f(1, false, false), ReferencedObjectMissing);
        assert_eq!(f(0, true, true), RecheckThenDelete);
        assert_eq!(f(0, true, false), RecheckThenDelete);
        assert_eq!(f(0, false, true), DeleteMetadata);
        assert_eq!(f(0, false, false), Complete);
    }

    /// Simulates §26's pipeline with a crash after every step and
    /// checks recovery always converges to "deleted" or "kept live".
    #[test]
    fn crash_at_every_point_converges() {
        // facts after k completed steps of: object delete, metadata delete
        let after = [(true, true), (false, true), (false, false)];
        for (o, m) in after {
            let mut obj = o;
            let mut meta = m;
            for _ in 0..3 {
                match reconcile_gc(GcFacts {
                    authoritative_refs: 0,
                    object_exists: obj,
                    metadata_exists: meta,
                }) {
                    GcRecoveryAction::RecheckThenDelete => obj = false,
                    GcRecoveryAction::DeleteMetadata => meta = false,
                    GcRecoveryAction::Complete => break,
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert!(!obj && !meta);
        }
    }

    #[test]
    fn reference_arriving_mid_gc_keeps_the_blob() {
        // Marked and rechecked at zero refs, then a new reference
        // lands before the delete: recovery must not delete.
        assert_eq!(
            reconcile_gc(GcFacts {
                authoritative_refs: 1,
                object_exists: true,
                metadata_exists: true
            }),
            GcRecoveryAction::AbortKeepLive
        );
    }
}
