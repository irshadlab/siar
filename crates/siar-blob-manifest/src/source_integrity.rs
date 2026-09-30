//! §118 "Source Mutation Detection", §119 "Immutable Source Snapshot".
//!
//! §119 gets no type of its own: "best production approach where
//! feasible: copy/import source into managed blob staging, then
//! transfer immutable bytes" describes an I/O strategy (this crate has
//! no filesystem staging — see `lib.rs`'s own scope note), not a value
//! this crate could represent. When that strategy IS used, its whole
//! point is that [`SourceSnapshot`] below becomes unnecessary — there
//! is no live, mutable source left to detect changes against once the
//! bytes are already staged immutably. [`SourceSnapshot`] is
//! specifically the fallback for when §119's approach ISN'T feasible
//! and a transfer really is reading from a live file handle.

use serde::{Deserialize, Serialize};

use crate::ids::BlobId;

/// §118's own four named facts about a live source, recorded at the
/// moment a transfer starts reading it. `file_id` is `Option<String>`
/// rather than a platform-specific inode type — "if available" in the
/// spec's own wording already concedes this isn't universal (Windows
/// has no POSIX inode), so a caller supplies whatever stable
/// identifier its own platform can give, or `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub size: u64,
    pub mtime_millis: Option<u64>,
    pub file_id: Option<String>,
    pub content_hash_at_read: BlobId,
}

/// §118: "If source changes mid-transfer, abort/restart. Do not
/// silently send mixed versions." Any single field disagreeing is
/// enough — this is deliberately NOT a majority-vote or
/// weighted-confidence check; the whole point is refusing to guess
/// whether a change is meaningful.
pub fn source_mutated(original: &SourceSnapshot, current: &SourceSnapshot) -> bool {
    original != current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(size: u64, hash_seed: &[u8]) -> SourceSnapshot {
        SourceSnapshot {
            size,
            mtime_millis: Some(1_000),
            file_id: Some("inode-42".to_string()),
            content_hash_at_read: BlobId::from_ciphertext(hash_seed),
        }
    }

    #[test]
    fn an_unchanged_source_is_never_reported_as_mutated() {
        let a = snapshot(100, b"same content");
        let b = snapshot(100, b"same content");
        assert!(!source_mutated(&a, &b));
    }

    #[test]
    fn a_changed_size_alone_is_enough_to_detect_mutation() {
        let original = snapshot(100, b"same content");
        let mut current = original.clone();
        current.size = 200;
        assert!(source_mutated(&original, &current));
    }

    #[test]
    fn a_changed_content_hash_alone_is_enough_to_detect_mutation() {
        let original = snapshot(100, b"version one");
        let current = snapshot(100, b"version two");
        assert!(source_mutated(&original, &current));
    }

    #[test]
    fn a_changed_mtime_alone_is_enough_to_detect_mutation() {
        let original = snapshot(100, b"same content");
        let mut current = original.clone();
        current.mtime_millis = Some(2_000);
        assert!(source_mutated(&original, &current));
    }
}
