//! §67 "Relay Blob Storage", §68 "DTN Chunk Strategy".

use serde::{Deserialize, Serialize};

use crate::ids::BlobId;
use crate::manifest::BlobManifest;

/// §67's own five bullet points, made real fields rather than a
/// prose checklist: `quota_bytes` (quota-limited), the struct's own
/// absence of any plaintext/metadata field (opaque — it carries only a
/// [`BlobId`], never a [`crate::descriptor::FileMetadata`] or anything
/// else that would tell a relay what it's holding), `expires_at_millis`
/// (expiring), and this type's own existence as something distinct
/// from a device's own blob ownership records (separately accounted).
/// "Lower trust" and "relay node does not get content keys" are
/// structural, not fields: this type has no
/// [`crate::ids::BlobEncryptionKey`] anywhere in it, and never could —
/// a relay physically cannot decrypt what it stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStorageAccounting {
    pub blob_id: BlobId,
    pub quota_bytes: u64,
    pub expires_at_millis: u64,
}

impl RelayStorageAccounting {
    pub fn is_expired(&self, now_millis: u64) -> bool {
        now_millis >= self.expires_at_millis
    }
}

/// §68's own four named policies. `SelectedChunks` and `Thumbnail` are
/// genuinely different things, not two names for the same idea:
/// selected chunks are real pieces of THIS blob's own chunk sequence,
/// while a thumbnail is a separate, already-generated, much smaller
/// blob with its own [`BlobId`] — sending "the thumbnail" over DTN
/// never means sending some subset of the original file's chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DtnChunkPolicy {
    MetadataOnly,
    Thumbnail(BlobId),
    SelectedChunks(Vec<u32>),
    FullFile,
}

/// Resolves a policy against a real manifest into the actual chunk
/// indices of *this* blob to send — `None` for [`DtnChunkPolicy::Thumbnail`]
/// specifically, since a thumbnail's chunks (if any) live under its own,
/// different [`BlobManifest`], not this one; a caller fetches that
/// separately by the returned [`BlobId`] rather than expecting this
/// function to reach across manifests.
pub fn chunks_to_send(policy: &DtnChunkPolicy, manifest: &BlobManifest) -> Option<Vec<u32>> {
    match policy {
        DtnChunkPolicy::MetadataOnly => Some(Vec::new()),
        DtnChunkPolicy::Thumbnail(_) => None,
        DtnChunkPolicy::SelectedChunks(indices) => Some(indices.clone()),
        DtnChunkPolicy::FullFile => Some((0..manifest.chunks.len() as u32).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;

    fn sample_manifest() -> BlobManifest {
        let ciphertext = vec![0u8; 300];
        crate::manifest::build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap()
    }

    #[test]
    fn relay_accounting_expires_at_and_after_its_own_deadline() {
        let accounting = RelayStorageAccounting {
            blob_id: BlobId::from_ciphertext(b"whatever"),
            quota_bytes: 1_000,
            expires_at_millis: 5_000,
        };
        assert!(!accounting.is_expired(4_999));
        assert!(accounting.is_expired(5_000));
    }

    #[test]
    fn metadata_only_sends_zero_chunks() {
        let manifest = sample_manifest();
        assert_eq!(
            chunks_to_send(&DtnChunkPolicy::MetadataOnly, &manifest),
            Some(Vec::new())
        );
    }

    #[test]
    fn full_file_sends_every_chunk_index() {
        let manifest = sample_manifest();
        assert_eq!(
            chunks_to_send(&DtnChunkPolicy::FullFile, &manifest),
            Some(vec![0, 1, 2])
        );
    }

    #[test]
    fn selected_chunks_passes_through_exactly_what_was_asked_for() {
        let manifest = sample_manifest();
        let policy = DtnChunkPolicy::SelectedChunks(vec![0, 2]);
        assert_eq!(chunks_to_send(&policy, &manifest), Some(vec![0, 2]));
    }

    #[test]
    fn a_thumbnail_policy_never_resolves_to_chunks_of_this_manifest() {
        let manifest = sample_manifest();
        let policy = DtnChunkPolicy::Thumbnail(BlobId::from_ciphertext(b"a different blob"));
        assert_eq!(chunks_to_send(&policy, &manifest), None);
    }
}
