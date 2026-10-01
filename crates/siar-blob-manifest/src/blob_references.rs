//! §59 "Deduplication", §60 "Reference Counting", §61 "Pinning".
//!
//! §59 gets no function of its own — it's already true by
//! construction, not something to check at call time.
//! [`crate::ids::BlobId::from_ciphertext`] is the *only* way to build a
//! [`crate::ids::BlobId`] (see that method's own doc comment), so two
//! manifests built from identical ciphertext bytes necessarily produce
//! the identical `BlobId` — local deduplication falls out of the ID
//! scheme itself. §59's second half ("different encryption keys
//! generally prevent cross-message plaintext deduplication... that is
//! acceptable for privacy") is likewise already true: this crate
//! deliberately has no plaintext-level content-addressing anywhere
//! (see [`crate::ids`]'s own top-level reasoning) to accidentally
//! create that leak.

use serde::{Deserialize, Serialize};

/// §60's own six named reference kinds, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceKind {
    MessageAttachment,
    FileShareHistory,
    Draft,
    CachePin,
    DtnBundle,
    Backup,
}

/// §60: "Track logical references... Blob becomes GC-eligible when no
/// required references remain." A real multiset (`Vec`, not a `HashSet`
/// or per-kind counter), because two *different* messages can each
/// reference the same attachment blob — that's two real
/// `MessageAttachment` references, both of which must be removed
/// before this blob is GC-eligible, not one reference that a second
/// identical add would no-op against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceSet {
    references: Vec<ReferenceKind>,
}

impl ReferenceSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_reference(&mut self, kind: ReferenceKind) {
        self.references.push(kind);
    }

    /// Removes exactly one matching reference, if any exist — the
    /// multiset counterpart to [`ReferenceSet::add_reference`]. Two
    /// adds of the same kind need two removes, not one, to reach zero.
    pub fn remove_reference(&mut self, kind: ReferenceKind) {
        if let Some(pos) = self.references.iter().position(|&k| k == kind) {
            self.references.remove(pos);
        }
    }

    /// §60's own closing sentence, made a real check: GC-eligible iff
    /// there is truly nothing left, of any kind.
    pub fn is_gc_eligible(&self) -> bool {
        self.references.is_empty()
    }

    /// §94's own `reference_count` field: the real total across every
    /// kind, not per-kind — a blob with two `MessageAttachment`
    /// references and one `Backup` reference has `len() == 3`, the
    /// same number [`ReferenceSet::is_gc_eligible`] is really asking
    /// "is this zero?" about.
    pub fn len(&self) -> usize {
        self.references.len()
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }
}

/// §61, field-for-field. A distinct vocabulary from
/// [`ReferenceKind`] above by the spec's own choice (five items here,
/// six there, not a 1:1 mapping) — §60 answers "does this blob still
/// logically need to exist at all," while §61 answers a narrower
/// question over things that already passed that bar: "is this
/// specific thing currently exempt from cache eviction." A blob can
/// have real, non-empty [`ReferenceSet`] entries (so it's not
/// GC-eligible) while still being evictable from a *cache* copy if
/// none of those references happen to be a [`PinReason`] — the two
/// checks are genuinely separate, not a renaming of each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinReason {
    UserSaved,
    ActiveTransfer,
    MessageReference,
    DtnRequired,
    BackupPending,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_reference_set_is_immediately_gc_eligible() {
        assert!(ReferenceSet::new().is_gc_eligible());
    }

    #[test]
    fn adding_any_reference_makes_it_ineligible() {
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::Draft);
        assert!(!refs.is_gc_eligible());
    }

    #[test]
    fn two_references_of_the_same_kind_both_need_removing() {
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::MessageAttachment);
        refs.add_reference(ReferenceKind::MessageAttachment);
        refs.remove_reference(ReferenceKind::MessageAttachment);
        assert!(!refs.is_gc_eligible(), "one reference still remains");
        refs.remove_reference(ReferenceKind::MessageAttachment);
        assert!(refs.is_gc_eligible());
    }

    #[test]
    fn removing_a_reference_kind_that_was_never_added_is_a_harmless_no_op() {
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::Backup);
        refs.remove_reference(ReferenceKind::DtnBundle); // never added
        assert!(
            !refs.is_gc_eligible(),
            "the real Backup reference is untouched"
        );
    }

    #[test]
    fn mixed_reference_kinds_all_count_toward_ineligibility() {
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::CachePin);
        refs.add_reference(ReferenceKind::FileShareHistory);
        refs.remove_reference(ReferenceKind::CachePin);
        assert!(
            !refs.is_gc_eligible(),
            "FileShareHistory reference still remains"
        );
        refs.remove_reference(ReferenceKind::FileShareHistory);
        assert!(refs.is_gc_eligible());
    }

    #[test]
    fn len_counts_every_kind_together_not_per_kind() {
        let mut refs = ReferenceSet::new();
        assert_eq!(refs.len(), 0);
        assert!(refs.is_empty());
        refs.add_reference(ReferenceKind::MessageAttachment);
        refs.add_reference(ReferenceKind::MessageAttachment);
        refs.add_reference(ReferenceKind::Backup);
        assert_eq!(refs.len(), 3);
        assert!(!refs.is_empty());
    }
}
