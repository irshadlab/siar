//! §14 "Chunk Identity": "allows verifying individual chunks before
//! whole-object completion." [`verify_complete_blob`] is also §76
//! "Full Blob Verification"'s own "reconstruct ordered ciphertext →
//! verify root/blob identity" — but see
//! [`crate::source_trust::verify_manifest_authenticity`]'s own doc
//! comment for a real gap this function alone does NOT close: it
//! checks a manifest for internal self-consistency, never against an
//! externally-authenticated expected [`BlobId`] (§77).

use crate::ids::{BlobId, ChunkHash};
use crate::manifest::{BlobManifest, ChunkDescriptor};

/// Real, byte-level verification — recomputes the hash and compares,
/// not a trust-the-caller check.
pub fn verify_chunk(chunk_bytes: &[u8], descriptor: &ChunkDescriptor) -> bool {
    // `try_from`, not `as u32`: a chunk of `2^32 + size` bytes must not
    // alias to `size`.
    u32::try_from(chunk_bytes.len()) == Ok(descriptor.size)
        && ChunkHash::from_ciphertext_chunk(chunk_bytes) == descriptor.hash
}

/// Verifies a fully-assembled ciphertext buffer against a manifest:
/// every chunk individually (§14) and the whole blob's own
/// content-derived identity (§6/§7) — a peer could in principle
/// tamper with data such that every individual chunk hash still
/// matches its own descriptor while the manifest's `blob_id` itself
/// was forged to a value that doesn't match the real reassembled
/// ciphertext; checking both, not just chunk hashes, closes that gap.
pub fn verify_complete_blob(ciphertext: &[u8], manifest: &BlobManifest) -> bool {
    if ciphertext.len() as u64 != manifest.total_size {
        return false;
    }
    if BlobId::from_ciphertext(ciphertext) != manifest.blob_id {
        return false;
    }
    for chunk_descriptor in &manifest.chunks {
        let start = chunk_descriptor.offset as usize;
        let end = start + chunk_descriptor.size as usize;
        let Some(chunk_bytes) = ciphertext.get(start..end) else {
            return false;
        };
        if !verify_chunk(chunk_bytes, chunk_descriptor) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;
    use crate::manifest::build_manifest;

    #[test]
    fn a_real_chunk_verifies_against_its_own_descriptor() {
        let ciphertext = vec![5u8; 300];
        let manifest = build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        assert!(verify_chunk(&ciphertext[0..100], &manifest.chunks[0]));
    }

    #[test]
    fn a_tampered_chunk_fails_verification() {
        let ciphertext = vec![5u8; 300];
        let manifest = build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let mut tampered = ciphertext[0..100].to_vec();
        tampered[0] ^= 0xFF;
        assert!(!verify_chunk(&tampered, &manifest.chunks[0]));
    }

    #[test]
    fn a_complete_correctly_assembled_blob_verifies() {
        let ciphertext = vec![5u8; 300];
        let manifest = build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        assert!(verify_complete_blob(&ciphertext, &manifest));
    }

    #[test]
    fn tampering_after_manifest_construction_fails_whole_blob_verification() {
        let ciphertext = vec![5u8; 300];
        let manifest = build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let mut tampered = ciphertext.clone();
        tampered[250] ^= 0xFF;
        assert!(!verify_complete_blob(&tampered, &manifest));
    }
}
