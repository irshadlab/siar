//! §92 "Filesystem Permissions", §93 "Database Schema", §94 "Blob
//! Table", §95 "Transfer Chunk Table".
//!
//! §93's own eight conceptual tables, each mapped to what actually
//! covers it in this crate — some already existed under this round's
//! predecessors, some are new here:
//!
//! | §93 table | This crate's own type |
//! |---|---|
//! | `blobs` | [`BlobRow`] (new this round) |
//! | `blob_references` | [`crate::blob_references::ReferenceSet`] |
//! | `blob_derivatives` | [`BlobDerivative`] (new this round) |
//! | `transfers` | [`crate::transfer_record::TransferRecord`] |
//! | `transfer_chunks` | [`TransferChunkRow`] (new this round) |
//! | `transfer_sources` | [`TransferSourceRow`] (new this round) |
//! | `blob_pins` | [`BlobPinRow`] (new this round) |
//! | `storage_quotas` | [`crate::resource_policy::FileResourcePolicy`] |
//!
//! None of these are backed by a real database — this crate has no
//! store at all (see `lib.rs`'s own scope note). They're row SHAPES:
//! real types a future SQL/`stoolap` backend would use as its own
//! table definitions, the same way `siar-event-log`'s own schema
//! started as plain Rust types before `StoolapEventStore` existed.
//! Defining the shape now means a future backend has an actual
//! contract to implement against, not a blank page.

use serde::{Deserialize, Serialize};

use crate::ids::{BlobId, TransferId};
use crate::source_trust::BlobSource;
use crate::storage_lifecycle::BlobClassification;

/// §92: "blob directories should use restrictive permissions." A real
/// policy VALUE, not a syscall — applying it (`chmod`/ACLs) is real
/// OS I/O this crate doesn't perform (see `lib.rs`'s own scope note),
/// but the restrictive numbers themselves are a decision worth naming
/// rather than leaving as prose. Unix mode bits specifically (`0o700`/
/// `0o600`) — "on mobile, app sandbox provides additional isolation"
/// (already true by platform, nothing to configure) and "on desktop,
/// set application-owned paths carefully" is exactly what applying
/// this policy on a real filesystem means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemPermissionPolicy {
    pub directory_mode: u32,
    pub file_mode: u32,
}

impl Default for FilesystemPermissionPolicy {
    /// Owner-only read/write/execute on directories (`0o700`), owner-
    /// only read/write on files (`0o600`) — no group or world access
    /// at all, the most restrictive posture §92's own "restrictive
    /// permissions" could mean.
    fn default() -> Self {
        Self {
            directory_mode: 0o700,
            file_mode: 0o600,
        }
    }
}

/// §94's own lifecycle states for a blob row — not the same thing as
/// [`BlobClassification`] (which governs eviction/durability policy
/// for a blob already known to be fine). This is the narrower
/// question §94 actually asks: has this blob's content even been
/// confirmed safe to treat as its declared identity yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobLifecycleState {
    Pending,
    Verified,
    Quarantined,
}

/// §94's own seven named fields. Deliberately has NO path field of any
/// kind — "do not store content path from untrusted metadata" is
/// enforced by there being nothing here to put one in; a real backend
/// derives any on-disk location it needs from `blob_id` itself
/// (content-addressed, never from anything a peer supplied).
/// `reference_count` is a plain `usize` snapshot, not a live
/// [`crate::blob_references::ReferenceSet`] — a database row holds a
/// value, not a handle to a mutable structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRow {
    pub blob_id: BlobId,
    pub size: u64,
    pub state: BlobLifecycleState,
    pub created_at_millis: u64,
    pub last_accessed_millis: u64,
    pub storage_class: BlobClassification,
    pub reference_count: usize,
}

/// §68's own "thumbnail" concept, generalized to whatever kinds of
/// derived object this crate already has a real name for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DerivativeKind {
    Thumbnail,
    Transcode,
    Preview,
}

/// `blob_derivatives`: a derived blob is a REAL, independent blob (its
/// own [`BlobId`], its own [`BlobRow`]) that also happens to be
/// derived from another — this row is purely the link between the
/// two, not a container for the derivative's own bytes or metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobDerivative {
    pub derivative_blob_id: BlobId,
    pub source_blob_id: BlobId,
    pub kind: DerivativeKind,
}

/// §95's own six named fields, field-for-field. `verified` is its own
/// bool, separate from `state` in [`BlobRow`] — a chunk can be
/// `received` without the WHOLE blob having reached
/// [`BlobLifecycleState::Verified`] yet (whole-blob verification only
/// happens once every chunk is in), so collapsing the two would lose
/// real information a resume/retry decision needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferChunkRow {
    pub transfer_id: TransferId,
    pub chunk_index: u32,
    pub verified: bool,
    pub bytes_received: u64,
    pub retry_count: u32,
}

/// `transfer_sources`: binds a [`BlobSource`] KIND to the specific
/// transfer it applies to — [`BlobSource`] alone is just vocabulary
/// (see that type's own doc comment), never tied to any one transfer
/// until a row like this one says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSourceRow {
    pub transfer_id: TransferId,
    pub source: BlobSource,
}

/// `blob_pins`: binds a [`crate::blob_references::PinReason`] to the
/// specific blob it protects — same relationship as
/// [`TransferSourceRow`] above, for the same reason: a bare
/// `PinReason` says WHY something might be pinned, never WHICH blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobPinRow {
    pub blob_id: BlobId,
    pub reason: crate::blob_references::PinReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_permission_policy_is_owner_only_on_both_directories_and_files() {
        let policy = FilesystemPermissionPolicy::default();
        assert_eq!(policy.directory_mode, 0o700);
        assert_eq!(policy.file_mode, 0o600);
        // No group or world bits set at all.
        assert_eq!(policy.directory_mode & 0o077, 0);
        assert_eq!(policy.file_mode & 0o077, 0);
    }

    #[test]
    fn a_blob_row_round_trips_through_postcard_with_no_path_field_to_leak() {
        let row = BlobRow {
            blob_id: BlobId::from_ciphertext(b"row contents"),
            size: 4_096,
            state: BlobLifecycleState::Verified,
            created_at_millis: 1_000,
            last_accessed_millis: 2_000,
            storage_class: BlobClassification::Durable,
            reference_count: 2,
        };
        let bytes = postcard::to_allocvec(&row).unwrap();
        let decoded: BlobRow = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, row);
    }

    #[test]
    fn a_derivative_links_two_distinct_blob_ids() {
        let derivative = BlobDerivative {
            derivative_blob_id: BlobId::from_ciphertext(b"thumbnail bytes"),
            source_blob_id: BlobId::from_ciphertext(b"full image bytes"),
            kind: DerivativeKind::Thumbnail,
        };
        assert_ne!(derivative.derivative_blob_id, derivative.source_blob_id);
    }

    #[test]
    fn a_transfer_chunk_row_can_be_received_without_the_whole_blob_being_verified() {
        let row = TransferChunkRow {
            transfer_id: TransferId::new(),
            chunk_index: 3,
            verified: false,
            bytes_received: 65_536,
            retry_count: 0,
        };
        assert!(!row.verified);
        assert!(row.bytes_received > 0);
    }
}
