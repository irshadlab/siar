//! `04-offline-event-log-architecture.md` §43 "Blob References".
//!
//! "Large binary data never belongs directly in the journal. Events
//! should reference `BlobId` and `ContentHash`. Part 05 will define the
//! blob subsystem."
//!
//! Part 05 now exists (`siar-blob-manifest`), and it depends on *this*
//! crate, so this crate cannot name its `BlobId`. [`BlobRef`] therefore
//! carries the same 32-byte values as raw arrays; the blob subsystem
//! converts at its edge. What lives here is what the *journal* needs to
//! hold up its half of §43:
//!
//! - a fixed-size, self-describing, **bounded** reference
//!   ([`BLOB_REF_LEN`] = 77 bytes) that an event payload embeds instead of
//!   data, so "event stays small" is a property of the type, not a
//!   convention;
//! - a strict decoder that rejects anything that isn't *exactly* a
//!   reference (no trailing bytes, no truncation) — it is parsed from
//!   replicated, hostile-capable input;
//! - [`require_blob_for_large`], the check a domain runs *before*
//!   building an event, so a 5 MB attachment fails fast with an error
//!   that says "store a blob" rather than reaching
//!   [`crate::limits`]' ceiling and failing with a size message.
//!
//! `size` is advisory metadata for UI and quota decisions; **integrity**
//! is `content_hash`. A consumer must verify fetched bytes against it.

use thiserror::Error;

const MAGIC: &[u8; 5] = b"BREF1";

/// `BREF1 ‖ blob_id(32) ‖ content_hash(32) ‖ size(8, LE)`.
pub const BLOB_REF_LEN: usize = 5 + 32 + 32 + 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobRef {
    pub blob_id: [u8; 32],
    pub content_hash: [u8; 32],
    pub size: u64,
}

impl BlobRef {
    pub fn encode(&self) -> [u8; BLOB_REF_LEN] {
        let mut out = [0u8; BLOB_REF_LEN];
        out[..5].copy_from_slice(MAGIC);
        out[5..37].copy_from_slice(&self.blob_id);
        out[37..69].copy_from_slice(&self.content_hash);
        out[69..].copy_from_slice(&self.size.to_le_bytes());
        out
    }

    /// `None` unless `bytes` is exactly one reference.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != BLOB_REF_LEN || &bytes[..5] != MAGIC {
            return None;
        }
        Some(Self {
            blob_id: bytes[5..37].try_into().ok()?,
            content_hash: bytes[37..69].try_into().ok()?,
            size: u64::from_le_bytes(bytes[69..].try_into().ok()?),
        })
    }

    /// Does a payload consist of nothing but a blob reference?
    pub fn is_blob_ref(bytes: &[u8]) -> bool {
        Self::decode(bytes).is_some()
    }
}

/// Default inline threshold: anything bigger than this belongs in a
/// blob, not an event. Deliberately far below
/// [`crate::limits::HARD_MAX_EVENT_PAYLOAD_BYTES`].
pub const DEFAULT_INLINE_LIMIT_BYTES: usize = 16 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{len} bytes is too large to journal inline (limit {limit}); store it as a blob and journal a BlobRef (§43)")]
pub struct InlineBlobError {
    pub len: usize,
    pub limit: usize,
}

pub fn require_blob_for_large(len: usize, limit: usize) -> Result<(), InlineBlobError> {
    if len > limit {
        Err(InlineBlobError { len, limit })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> BlobRef {
        BlobRef {
            blob_id: [1; 32],
            content_hash: [2; 32],
            size: 5_000_000,
        }
    }

    #[test]
    fn round_trips_at_exactly_the_documented_length() {
        let enc = sample().encode();
        assert_eq!(enc.len(), BLOB_REF_LEN);
        assert_eq!(BlobRef::decode(&enc), Some(sample()));
        assert!(BlobRef::is_blob_ref(&enc));
    }

    #[test]
    fn a_reference_is_tiny_compared_to_what_it_points_at() {
        const _: () = assert!(BLOB_REF_LEN < 100);
        assert!(sample().size as usize > DEFAULT_INLINE_LIMIT_BYTES * 100);
    }

    #[test]
    fn anything_that_is_not_exactly_a_reference_is_rejected() {
        let enc = sample().encode();
        assert_eq!(BlobRef::decode(&enc[..BLOB_REF_LEN - 1]), None, "truncated");
        let mut long = enc.to_vec();
        long.push(0);
        assert_eq!(BlobRef::decode(&long), None, "trailing byte");
        let mut bad_magic = enc;
        bad_magic[0] = b'X';
        assert_eq!(BlobRef::decode(&bad_magic), None);
        assert_eq!(BlobRef::decode(&[]), None);
    }

    #[test]
    fn large_inline_data_is_refused_with_actionable_advice() {
        assert!(
            require_blob_for_large(DEFAULT_INLINE_LIMIT_BYTES, DEFAULT_INLINE_LIMIT_BYTES).is_ok()
        );
        let err = require_blob_for_large(5_000_000, DEFAULT_INLINE_LIMIT_BYTES).unwrap_err();
        assert!(err.to_string().contains("blob"));
        assert_eq!(err.len, 5_000_000);
    }

    proptest! {
        #[test]
        fn decode_never_panics_and_accepts_only_exact_encodings(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            if let Some(r) = BlobRef::decode(&bytes) {
                let again = r.encode();
                prop_assert_eq!(again.as_slice(), bytes.as_slice());
            }
        }
    }
}
