//! Encryption at rest (§50-§51) for session state and prekey secrets.
//!
//! * **XChaCha20-Poly1305** with a random 192-bit nonce per write: a random
//!   nonce is safe at that size, so rewriting the same state thousands of
//!   times never risks reuse (§17).
//! * The key is **derived from the device identity under a per-purpose label**
//!   (key separation, §44), so there is no new secret to provision, and the
//!   prekey store and the session store use different keys.
//! * Every blob is bound by AAD to *what it is and which session it belongs
//!   to*: a state blob cannot be swapped onto another session or into the
//!   prekey store and still open.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand_core::RngCore;
use siar_crypto::DeviceIdentity;
use zeroize::Zeroizing;

const FORMAT: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CipherError {
    #[error("stored data could not be decrypted (wrong key, tampering or corruption)")]
    Open,
    #[error("stored data has an unknown format")]
    Format,
}

pub struct AtRestKey(Zeroizing<[u8; 32]>);

impl AtRestKey {
    /// Derive the key for one storage purpose (`"session-state"`, `"prekeys"`…).
    #[must_use]
    pub fn derive(identity: &DeviceIdentity, purpose: &str) -> Self {
        Self(identity.derive_secret(&format!("siar/at-rest/{purpose}/v1")))
    }

    #[must_use]
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(Zeroizing::new(b))
    }

    /// `[format][nonce(24)][ciphertext+tag]`.
    #[must_use]
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; 24];
        rand_core::OsRng.fill_bytes(&mut nonce);
        let ct = XChaCha20Poly1305::new((&*self.0).into())
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("encryption of an in-memory buffer cannot fail");
        let mut out = Vec::with_capacity(1 + 24 + ct.len());
        out.push(FORMAT);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, CipherError> {
        if blob.len() < 1 + 24 + 16 || blob[0] != FORMAT {
            return Err(CipherError::Format);
        }
        XChaCha20Poly1305::new((&*self.0).into())
            .decrypt(
                XNonce::from_slice(&blob[1..25]),
                Payload {
                    msg: &blob[25..],
                    aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| CipherError::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(b: u8) -> AtRestKey {
        AtRestKey::from_bytes([b; 32])
    }

    #[test]
    fn round_trip_and_fresh_nonce_every_time() {
        let key = k(1);
        let (a, b) = (key.seal(b"aad", b"secret"), key.seal(b"aad", b"secret"));
        assert_ne!(a, b, "identical plaintext never produces identical blobs");
        assert_eq!(&*key.open(b"aad", &a).unwrap(), b"secret");
        assert!(
            !a.windows(6).any(|w| w == b"secret"),
            "plaintext is not visible in the blob"
        );
    }

    #[test]
    fn wrong_key_wrong_aad_and_tampering_all_fail_closed() {
        let blob = k(1).seal(b"session-1", b"state");
        assert_eq!(
            k(2).open(b"session-1", &blob).err(),
            Some(CipherError::Open)
        );
        assert_eq!(
            k(1).open(b"session-2", &blob).err(),
            Some(CipherError::Open),
            "a blob cannot be moved to another session"
        );
        for i in 0..blob.len() {
            let mut bad = blob.clone();
            bad[i] ^= 1;
            assert!(k(1).open(b"session-1", &bad).is_err(), "byte {i}");
        }
        assert_eq!(
            k(1).open(b"x", &blob[..10]).err(),
            Some(CipherError::Format)
        );
    }

    #[test]
    fn purposes_derive_independent_keys() {
        let id = DeviceIdentity::generate();
        let (a, b) = (
            AtRestKey::derive(&id, "session-state"),
            AtRestKey::derive(&id, "prekeys"),
        );
        let blob = a.seal(b"x", b"y");
        assert!(b.open(b"x", &blob).is_err());
        assert_eq!(
            &*AtRestKey::derive(&id, "session-state")
                .open(b"x", &blob)
                .unwrap(),
            b"y",
            "derivation is deterministic"
        );
    }
}
