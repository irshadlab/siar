use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::error::IdentityError;

/// §5 "Account Identity", §6 "Root Key Strategy": the account's durable
/// logical principal is anchored by a root signing key that is used
/// rarely — only to sign [`crate::certificate::DeviceCertificate`]s and
/// [`crate::directory::DeviceDirectory`] snapshots, not for every
/// message or session (§6's own explicit "rather than: root key used
/// for every message/session"). Deliberately independent of
/// `siar-crypto::DeviceIdentity` — see this crate's own top doc comment
/// for why a *root* key and a *device* key are kept as separate
/// concepts here, matching §3's "identity layers must not be
/// collapsed."
pub struct RootIdentityKey {
    signing_key: SigningKey,
}

impl RootIdentityKey {
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Rebuild a root key from its 32-byte seed (the inverse of [`seed`](Self::seed)).
    /// For loading a key this device stored earlier.
    #[must_use]
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(seed),
        }
    }

    /// The 32-byte seed, wiped from memory when dropped. Whoever holds it can
    /// sign certificates for the account: store it like a private key.
    #[must_use]
    pub fn seed(&self) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new(self.signing_key.to_bytes())
    }

    pub fn root_public_key(&self) -> RootPublicKey {
        RootPublicKey(self.signing_key.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }
}

impl Drop for RootIdentityKey {
    fn drop(&mut self) {
        // `SigningKey` already zeroizes itself on drop given this
        // crate's own "zeroize" feature on `ed25519-dalek` (matching
        // `siar_crypto::DeviceIdentity`'s exact same dependency
        // feature); this is the same defense-in-depth no-op belt that
        // type's own `Drop` impl documents, kept here for the identical
        // reason — a root key's secret material deserves at least as
        // much care as an ordinary device key's, arguably more (§6:
        // compromising it can forge every device certificate for the
        // account).
        let mut marker = [0u8; 0];
        marker.zeroize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootPublicKey(pub [u8; 32]);

impl RootPublicKey {
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<(), IdentityError> {
        let verifying_key =
            VerifyingKey::from_bytes(&self.0).map_err(|_| IdentityError::MalformedKey)?;
        let signature = Signature::from_bytes(signature);
        verifying_key
            .verify(message, &signature)
            .map_err(|_| IdentityError::InvalidSignature)
    }
}

#[cfg(test)]
mod seed_tests {
    use super::*;

    #[test]
    fn a_stored_seed_rebuilds_the_same_key() {
        let a = RootIdentityKey::generate();
        let b = RootIdentityKey::from_seed(&a.seed());
        assert_eq!(a.root_public_key(), b.root_public_key());
        let sig = a.sign(b"hello");
        assert!(b.root_public_key().verify(b"hello", &sig).is_ok());
    }
}
