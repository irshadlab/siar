//! §186 "Error Model", §187 "Recoverability Classification", §188 "No
//! `anyhow` in Public File APIs" — for the local blob store
//! ([`crate::fs_store`], [`crate::blob_io`]).
//!
//! One closed enum, no `Box<dyn Error>`, no `anyhow`: a caller matches
//! on *what went wrong*, and [`BlobError::recoverability`] says what to
//! *do* about it — the §187 classification as a method, so every call
//! site gets the same answer instead of re-deriving "should I retry
//! this?" from the variant name.
//!
//! ## Errors carry no paths
//!
//! [`BlobError::Io`] holds the `std::io::ErrorKind` and a static
//! operation name, never the `io::Error`'s message. An I/O message can
//! embed an absolute path (`/home/alice/...`), and these errors reach
//! logs, diagnostics and — through callers — bug reports. The kind plus
//! the operation is enough to act on; the path is on the operator's own
//! disk and recoverable from context.

use std::io::ErrorKind;

use thiserror::Error;

use crate::ids::BlobId;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlobError {
    #[error("blob not found")]
    NotFound(BlobId),
    #[error("a write for this blob is already in progress")]
    AlreadyStaging(BlobId),
    #[error("expected {expected} bytes, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("chunk index {index} out of range (blob has {chunk_count} chunks)")]
    ChunkIndexOutOfRange { index: u32, chunk_count: u32 },
    #[error("chunk {index} must be {expected} bytes, got {actual}")]
    ChunkSizeMismatch {
        index: u32,
        expected: u32,
        actual: usize,
    },
    #[error("chunk {index} failed hash verification")]
    ChunkHashMismatch { index: u32 },
    #[error("blob is incomplete: {missing_chunks} chunk(s) missing")]
    Incomplete { missing_chunks: u32 },
    /// The assembled bytes do not hash to the declared [`BlobId`]. The
    /// staged object was moved to quarantine, never exposed.
    #[error("assembled blob does not match its declared identity; quarantined")]
    BlobHashMismatch { declared: BlobId },
    #[error("range {start}+{len} is outside a {size}-byte blob")]
    RangeOutOfBounds { start: u64, len: u64, size: u64 },
    #[error("range of {len} bytes exceeds the {max}-byte read limit; use stream()")]
    RangeTooLarge { len: u64, max: u64 },
    #[error("storage reservation of {requested} bytes denied ({available} available)")]
    ReservationDenied { requested: u64, available: u64 },
    #[error("blob of {size} bytes exceeds the {limit}-byte per-blob limit")]
    BlobTooLarge { size: u64, limit: u64 },
    #[error("too many concurrent writes (limit {limit})")]
    TooManyConcurrentWrites { limit: usize },
    #[error("descriptor and manifest disagree: {0}")]
    InvalidManifest(&'static str),
    #[error("staging data is damaged: {0}")]
    CorruptStaging(&'static str),
    #[error("i/o error during {op}: {kind:?}")]
    Io { kind: ErrorKind, op: &'static str },
    #[error("metadata store error: {0}")]
    Metadata(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),
}

/// §187: what a caller should do about a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recoverability {
    /// Transient: the same call may succeed later (back off, retry).
    Retryable,
    /// The *request* is wrong; repeating it unchanged will fail again.
    CallerError,
    /// The data on the wire/disk was bad. Re-fetching from a source
    /// (a different chunk copy, another peer) can fix it.
    RefetchData,
    /// Local state is damaged; needs recovery/repair, not a retry.
    RequiresRepair,
    /// Policy said no (quota, limits). Retryable only if policy or
    /// usage changes.
    PolicyDenied,
}

impl BlobError {
    pub fn recoverability(&self) -> Recoverability {
        use BlobError::*;
        match self {
            Io { kind, .. } => match kind {
                ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut => {
                    Recoverability::Retryable
                }
                ErrorKind::OutOfMemory | ErrorKind::StorageFull => Recoverability::PolicyDenied,
                _ => Recoverability::RequiresRepair,
            },
            ChunkHashMismatch { .. } | BlobHashMismatch { .. } => Recoverability::RefetchData,
            CorruptStaging(_) | Metadata(_) => Recoverability::RequiresRepair,
            ReservationDenied { .. } | BlobTooLarge { .. } | TooManyConcurrentWrites { .. } => {
                Recoverability::PolicyDenied
            }
            AlreadyStaging(_) => Recoverability::Retryable,
            NotFound(_)
            | SizeMismatch { .. }
            | ChunkIndexOutOfRange { .. }
            | ChunkSizeMismatch { .. }
            | Incomplete { .. }
            | RangeOutOfBounds { .. }
            | RangeTooLarge { .. }
            | InvalidManifest(_)
            | InvalidArgument(_) => Recoverability::CallerError,
        }
    }
}

pub(crate) fn io(op: &'static str) -> impl FnOnce(std::io::Error) -> BlobError {
    move |e| BlobError::Io { kind: e.kind(), op }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_keep_the_kind_and_operation_but_never_a_path() {
        let raw = std::io::Error::new(
            ErrorKind::PermissionDenied,
            "denied: /home/alice/secret/blob",
        );
        let e = io("open staging")(raw);
        assert_eq!(
            e,
            BlobError::Io {
                kind: ErrorKind::PermissionDenied,
                op: "open staging"
            }
        );
        let rendered = format!("{e} {e:?}");
        assert!(!rendered.contains("alice") && !rendered.contains("/home"));
    }

    #[test]
    fn every_variant_has_a_classification() {
        let id = BlobId::from_ciphertext(b"x");
        let samples = [
            (BlobError::NotFound(id), Recoverability::CallerError),
            (BlobError::AlreadyStaging(id), Recoverability::Retryable),
            (
                BlobError::ChunkHashMismatch { index: 1 },
                Recoverability::RefetchData,
            ),
            (
                BlobError::BlobHashMismatch { declared: id },
                Recoverability::RefetchData,
            ),
            (
                BlobError::ReservationDenied {
                    requested: 2,
                    available: 1,
                },
                Recoverability::PolicyDenied,
            ),
            (
                BlobError::CorruptStaging("x"),
                Recoverability::RequiresRepair,
            ),
            (
                BlobError::Io {
                    kind: ErrorKind::Interrupted,
                    op: "w",
                },
                Recoverability::Retryable,
            ),
            (
                BlobError::Io {
                    kind: ErrorKind::StorageFull,
                    op: "w",
                },
                Recoverability::PolicyDenied,
            ),
            (
                BlobError::Io {
                    kind: ErrorKind::InvalidData,
                    op: "w",
                },
                Recoverability::RequiresRepair,
            ),
            (
                BlobError::Incomplete { missing_chunks: 3 },
                Recoverability::CallerError,
            ),
        ];
        for (e, want) in samples {
            assert_eq!(e.recoverability(), want, "{e}");
        }
    }
}
