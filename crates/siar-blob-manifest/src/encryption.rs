//! §18 "Encryption Model": "Use authenticated encryption." §19 "Per-Blob
//! Key": a fresh random key per blob, never reused. This is the actual
//! encrypt/decrypt this crate's own top doc comment previously listed
//! under "What's explicitly NOT here" — real now, not just a type
//! shape in [`crate::descriptor::EncryptionDescriptor`].
//!
//! Whole-blob AEAD, not per-chunk: one fresh key
//! ([`generate_blob_key`]), one fresh nonce, one `ChaCha20Poly1305`
//! operation over the *entire* plaintext — matching §7's own
//! "encrypt first, then content-address the ciphertext." Chunking
//! ([`crate::chunking`]/[`crate::manifest`]) then splits the resulting
//! ciphertext buffer by byte offset exactly as those modules already
//! did before this file existed; nothing about them needed to change.
//! See [`crate::descriptor::EncryptionDescriptor`]'s own doc comment
//! for the earlier, more complicated per-chunk-nonce design this
//! module's real implementation superseded.
//!
//! This crate stays independent of `siar-crypto` (see `lib.rs`'s own
//! top doc comment) — `chacha20poly1305` is called directly here,
//! mirroring (not importing) `siar_crypto::attachment::encrypt_attachment`'s
//! exact same pattern: fresh key, fresh nonce, nonce-prepended
//! ciphertext, `AttachmentKey`'s equivalent role played here by
//! [`crate::ids::BlobEncryptionKey`].

use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand_core::RngCore;

use crate::descriptor::{EncryptionAlgorithm, EncryptionDescriptor};
use crate::ids::BlobEncryptionKey;

const NONCE_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncryptionError {
    #[error("AEAD encryption failed")]
    EncryptionFailed,
    #[error("AEAD decryption failed — wrong key, corrupted ciphertext, or tampered data")]
    DecryptionFailed,
    #[error("ciphertext is {actual} bytes, too short to contain a {NONCE_LEN}-byte nonce")]
    CiphertextTooShort { actual: usize },
}

/// §19: "never reused across blobs." A fresh key every call — there is
/// no `from_bytes`/deterministic-derivation path in this module, so a
/// caller can't accidentally reuse a key by deriving "the same" one
/// twice; reconstructing a key from bytes carried in a
/// [`crate::descriptor::BlobDescriptor`] (the receiving side's job) is
/// [`BlobEncryptionKey`]'s own plain tuple-struct constructor, not
/// something this module needs to provide separately.
pub fn generate_blob_key() -> BlobEncryptionKey {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    BlobEncryptionKey(bytes)
}

fn cipher(key: &BlobEncryptionKey) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new((&key.0).into())
}

/// Encrypts the entire plaintext under one fresh random nonce. Returns
/// the ciphertext with the nonce prepended (self-contained —
/// [`decrypt_blob`] needs nothing but the key and this buffer) plus an
/// [`EncryptionDescriptor`] recording the same nonce and algorithm for
/// a caller that wants it separately (e.g. to publish alongside
/// [`crate::descriptor::BlobDescriptor`] without re-parsing the
/// ciphertext's first 12 bytes).
pub fn encrypt_blob(
    key: &BlobEncryptionKey,
    plaintext: &[u8],
) -> Result<(Vec<u8>, EncryptionDescriptor), EncryptionError> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let sealed = cipher(key)
        .encrypt(nonce, plaintext)
        .map_err(|_| EncryptionError::EncryptionFailed)?;

    let mut framed = Vec::with_capacity(NONCE_LEN + sealed.len());
    framed.extend_from_slice(&nonce_bytes);
    framed.extend_from_slice(&sealed);

    let descriptor = EncryptionDescriptor {
        algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
        nonce: nonce_bytes,
    };
    Ok((framed, descriptor))
}

/// The AEAD tag check here is real authentication, not a formality —
/// any single-bit tamper anywhere in `ciphertext` (including the
/// prepended nonce) makes this fail, matching
/// [`crate::verify::verify_complete_blob`]'s own hash-based integrity
/// check but at the cryptographic layer underneath it: a real receive
/// path should run both (hash check for cheap early rejection of
/// corrupted transfers per §14, AEAD decryption for actual
/// authenticated confidentiality) — this function only does the
/// latter.
pub fn decrypt_blob(
    key: &BlobEncryptionKey,
    ciphertext: &[u8],
) -> Result<Vec<u8>, EncryptionError> {
    if ciphertext.len() < NONCE_LEN {
        return Err(EncryptionError::CiphertextTooShort {
            actual: ciphertext.len(),
        });
    }
    let (nonce_bytes, sealed) = ciphertext.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher(key)
        .decrypt(nonce, sealed)
        .map_err(|_| EncryptionError::DecryptionFailed)
}

/// §129: decrypt and hand back bytes that are *known* authentic. Media
/// decoders take [`Verified`] so unverified bytes cannot reach them.
pub fn decrypt_blob_verified(
    key: &BlobEncryptionKey,
    ciphertext: &[u8],
) -> Result<crate::preprocess::Verified<Vec<u8>>, EncryptionError> {
    decrypt_blob(key, ciphertext).map(crate::preprocess::Verified::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let key = generate_blob_key();
        let plaintext = b"a whole file's worth of bytes, pretend";
        let (ciphertext, _descriptor) = encrypt_blob(&key, plaintext).unwrap();
        let recovered = decrypt_blob(&key, &ciphertext).unwrap();
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn same_plaintext_twice_yields_different_ciphertext() {
        let key = generate_blob_key();
        let plaintext = b"identical content";
        let (a, _) = encrypt_blob(&key, plaintext).unwrap();
        let (b, _) = encrypt_blob(&key, plaintext).unwrap();
        assert_ne!(a, b); // fresh random nonce each call
    }

    #[test]
    fn two_generated_keys_are_never_the_same() {
        let a = generate_blob_key();
        let b = generate_blob_key();
        assert_ne!(a.0, b.0); // §19: never reused across blobs
    }

    #[test]
    fn wrong_key_fails_decryption() {
        let key = generate_blob_key();
        let wrong_key = generate_blob_key();
        let (ciphertext, _) = encrypt_blob(&key, b"data").unwrap();
        assert!(decrypt_blob(&wrong_key, &ciphertext).is_err());
    }

    #[test]
    fn tampered_ciphertext_fails_decryption() {
        let key = generate_blob_key();
        let (mut ciphertext, _) = encrypt_blob(&key, b"data").unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        assert!(decrypt_blob(&key, &ciphertext).is_err());
    }

    #[test]
    fn a_tampered_nonce_also_fails_decryption() {
        let key = generate_blob_key();
        let (mut ciphertext, _) = encrypt_blob(&key, b"data").unwrap();
        ciphertext[0] ^= 0xFF; // the prepended nonce, not the sealed payload
        assert!(decrypt_blob(&key, &ciphertext).is_err());
    }

    #[test]
    fn a_ciphertext_shorter_than_one_nonce_is_rejected_cleanly() {
        let key = generate_blob_key();
        let result = decrypt_blob(&key, &[0u8; 5]);
        assert_eq!(
            result,
            Err(EncryptionError::CiphertextTooShort { actual: 5 })
        );
    }

    /// Real integration across this crate's own modules — encrypt,
    /// then chunk+manifest the resulting ciphertext, then verify —
    /// exactly the pipeline a real sender would run end to end, not
    /// three modules tested only in isolation from each other.
    #[test]
    fn a_full_pipeline_encrypts_chunks_and_verifies() {
        use crate::limits::ManifestLimits;
        use crate::manifest::build_manifest;
        use crate::verify::verify_complete_blob;

        let key = generate_blob_key();
        let plaintext = vec![42u8; 10_000];

        let (ciphertext, _descriptor) = encrypt_blob(&key, &plaintext).unwrap();
        let manifest = build_manifest(&ciphertext, 1024, &ManifestLimits::default()).unwrap();
        assert!(verify_complete_blob(&ciphertext, &manifest));

        let recovered = decrypt_blob(&key, &ciphertext).unwrap();
        assert_eq!(recovered, plaintext);
    }
}
