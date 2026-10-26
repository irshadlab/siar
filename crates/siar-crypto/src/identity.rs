//! Device identity (plan.md §6, §8).
//!
//! Never expose the raw secret keys outside this crate (plan.md §6) — the
//! only way callers get key material out is `verifying_key()` /
//! `x25519_public()`, both public keys.

use crate::CryptoError;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::Zeroize;

/// One device's key material: an Ed25519 signing key for authentication,
/// and a separate X25519 key for ECDH key agreement (plan.md §7–8).
///
/// Deliberately two different keys on two different curves rather than
/// reusing one Curve25519 key for both roles — that reuse is a known
/// footgun (signing and DH key reuse can leak key material across
/// protocols) and the extra key costs nothing at this scale.
pub struct DeviceIdentity {
    signing_key: SigningKey,
    x25519_secret: StaticSecret,
}

impl DeviceIdentity {
    /// Generate a fresh identity (first-run device provisioning,
    /// plan.md §8).
    pub fn generate() -> Self {
        Self {
            // clippy's `needless_borrows_for_generic_args` flagged only
            // the `StaticSecret::random_from_rng` call below (line 31 in
            // the original report) — `SigningKey::generate` genuinely
            // requires `&mut R` (confirmed against a real compile
            // error when this line was mistakenly changed too: "expected
            // `&mut _`, found `OsRng`"), so it keeps its borrow.
            signing_key: SigningKey::generate(&mut OsRng),
            x25519_secret: StaticSecret::random_from_rng(OsRng),
        }
    }

    /// A 32-byte secret for a DIFFERENT purpose, derived from this
    /// identity: same identity + same `context` always gives the same
    /// bytes; a different `context` (or identity) gives unrelated bytes.
    ///
    /// Why it exists: some keys must be stable across restarts but must
    /// not be the signing or DH key themselves — this crate's own rule
    /// against reusing one key across protocols. The transport (iroh)
    /// endpoint key is the case in point: it used to be generated afresh
    /// at every launch, so a device's address changed on every restart and
    /// every saved ticket for it went stale. Deriving it from the identity
    /// makes it stable with no extra secret file to store or back up.
    ///
    /// `context` must be a hard-coded, globally unique, versioned string
    /// (it is the domain separator); never build it from user input.
    pub fn derive_secret(&self, context: &str) -> zeroize::Zeroizing<[u8; 32]> {
        let mut material = [0u8; 64];
        material[..32].copy_from_slice(&self.signing_key.to_bytes());
        material[32..].copy_from_slice(&self.x25519_secret.to_bytes());
        let derived = blake3::derive_key(context, &material);
        material.zeroize();
        zeroize::Zeroizing::new(derived)
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn x25519_public(&self) -> X25519PublicKey {
        X25519PublicKey::from(&self.x25519_secret)
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    pub fn verify(
        verifying_key: &VerifyingKey,
        message: &[u8],
        signature: &Signature,
    ) -> Result<(), CryptoError> {
        verifying_key
            .verify(message, signature)
            .map_err(|_| CryptoError::InvalidSignature)
    }

    /// Consumes `self` and `their_public` to derive a shared secret. This
    /// is the raw ECDH output — `Session::establish` is what turns it into
    /// an actual encryption key; nothing outside this crate should use
    /// this directly.
    pub(crate) fn diffie_hellman(&self, their_public: &X25519PublicKey) -> [u8; 32] {
        let shared = self.x25519_secret.diffie_hellman(their_public);
        *shared.as_bytes()
    }

    /// Persists this identity's secret key material directly to a file
    /// at `path` — this doc comment's own top-of-file invariant ("never
    /// expose raw secret keys outside this crate") stays true even with
    /// persistence added: the bytes are written straight to a file
    /// handle inside this method, never returned to the caller as a
    /// `Vec<u8>`/array the caller could log, print, or hold onto longer
    /// than necessary.
    ///
    /// Not encrypted at rest — this writes raw key bytes to the
    /// filesystem, relying entirely on filesystem permissions (the
    /// caller should ensure the containing directory is created
    /// user-only, e.g. mode `0700`) for protection. Encrypting this
    /// file with an OS keychain or a passphrase-derived key is real
    /// additional work this method doesn't attempt — flagged, not
    /// silently skipped.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), CryptoError> {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&self.signing_key.to_bytes());
        bytes[32..].copy_from_slice(&self.x25519_secret.to_bytes());
        let result = std::fs::write(path, bytes).map_err(|err| CryptoError::Io(err.to_string()));
        bytes.zeroize();
        result
    }

    /// Loads an identity previously written by [`Self::save_to_file`].
    /// Zeroizes every intermediate byte buffer it touches on the way
    /// to constructing the final `SigningKey`/`StaticSecret` — those
    /// two types zeroize themselves on drop already (see this file's
    /// `Drop` impl below), but the raw bytes read from disk and the
    /// two 32-byte halves split out of them are plain arrays this
    /// method is responsible for clearing itself.
    pub fn load_from_file(path: &std::path::Path) -> Result<Self, CryptoError> {
        let mut bytes = std::fs::read(path).map_err(|err| CryptoError::Io(err.to_string()))?;
        if bytes.len() != 64 {
            bytes.zeroize();
            return Err(CryptoError::MalformedKey);
        }

        let mut signing_bytes = [0u8; 32];
        signing_bytes.copy_from_slice(&bytes[..32]);
        let mut x25519_bytes = [0u8; 32];
        x25519_bytes.copy_from_slice(&bytes[32..]);
        bytes.zeroize();

        let signing_key = SigningKey::from_bytes(&signing_bytes);
        signing_bytes.zeroize();
        let x25519_secret = StaticSecret::from(x25519_bytes);
        x25519_bytes.zeroize();

        Ok(Self {
            signing_key,
            x25519_secret,
        })
    }

    /// Explicit, deliberate key-material duplication — not a derived
    /// `Clone` impl, on purpose. This file's own top invariant ("the
    /// only way callers get key material out is `verifying_key()`/
    /// `x25519_public()`") stays true: this returns another opaque
    /// `DeviceIdentity`, never the raw bytes themselves, the same way
    /// `save_to_file`/`load_from_file` never hand bytes back to a
    /// caller either. Exists because a single device now legitimately
    /// needs more than one owner of its own identity in the same
    /// process — `siar_messaging::MessageService` and
    /// `siar_messaging::GroupService` each take a `DeviceIdentity` by
    /// value, and both represent the same local device, so an
    /// application wiring both up needs two independent instances with
    /// identical key material, not two different devices. A derived
    /// `Clone` would make that duplication just as easy to do by
    /// accident (e.g. cloning into a log line or a debug dump); naming
    /// this `try_clone` instead keeps duplication an explicit,
    /// grep-able choice at each call site.
    ///
    /// Fallible (`Result`, not infallible `Clone`) even though nothing
    /// about this specific implementation can currently fail —
    /// `SigningKey`/`StaticSecret` reconstruction from their own
    /// previously-valid byte representation doesn't error in practice.
    /// Kept fallible anyway so a future change to how either key type
    /// is reconstructed doesn't force a breaking signature change here
    /// later; returns `CryptoError::MalformedKey` in the (currently
    /// unreachable) case reconstruction ever did fail.
    pub fn try_clone(&self) -> Result<Self, CryptoError> {
        let mut bytes = [0u8; 64];
        bytes[..32].copy_from_slice(&self.signing_key.to_bytes());
        bytes[32..].copy_from_slice(&self.x25519_secret.to_bytes());

        let mut signing_bytes = [0u8; 32];
        signing_bytes.copy_from_slice(&bytes[..32]);
        let mut x25519_bytes = [0u8; 32];
        x25519_bytes.copy_from_slice(&bytes[32..]);
        bytes.zeroize();

        let signing_key = SigningKey::from_bytes(&signing_bytes);
        signing_bytes.zeroize();
        let x25519_secret = StaticSecret::from(x25519_bytes);
        x25519_bytes.zeroize();

        Ok(Self {
            signing_key,
            x25519_secret,
        })
    }
}

impl Drop for DeviceIdentity {
    fn drop(&mut self) {
        // SigningKey/StaticSecret already zeroize themselves on drop given
        // the "zeroize" feature; this is a defense-in-depth no-op belt.
        let mut marker = [0u8; 0];
        marker.zeroize();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn derived_secrets_are_stable_separated_and_not_the_identity_keys() {
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        let ctx = "siar test context v1";

        // stable: same identity, same context — including after a
        // save/load round trip and a clone (what a restart or restore does)
        let first = *a.derive_secret(ctx);
        assert_eq!(first, *a.derive_secret(ctx));
        assert_eq!(first, *a.try_clone().unwrap().derive_secret(ctx));
        let dir = std::env::temp_dir().join(format!("siar-derive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        a.save_to_file(&dir.join("id.bin")).unwrap();
        let reloaded = DeviceIdentity::load_from_file(&dir.join("id.bin")).unwrap();
        assert_eq!(first, *reloaded.derive_secret(ctx));
        let _ = std::fs::remove_dir_all(&dir);

        // separated: another context or another identity is unrelated
        assert_ne!(first, *a.derive_secret("siar test context v2"));
        assert_ne!(first, *b.derive_secret(ctx));

        // never just the signing or DH secret handed back out
        assert_ne!(first, a.signing_key.to_bytes());
        assert_ne!(first, a.x25519_secret.to_bytes());
    }

    use super::*;

    #[test]
    fn sign_and_verify_round_trip() {
        let id = DeviceIdentity::generate();
        let msg = b"hello siar";
        let sig = id.sign(msg);
        assert!(DeviceIdentity::verify(&id.verifying_key(), msg, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let id = DeviceIdentity::generate();
        let sig = id.sign(b"original");
        assert!(DeviceIdentity::verify(&id.verifying_key(), b"tampered", &sig).is_err());
    }

    #[test]
    fn ecdh_agreement_matches_on_both_sides() {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let alice_shared = alice.diffie_hellman(&bob.x25519_public());
        let bob_shared = bob.diffie_hellman(&alice.x25519_public());
        assert_eq!(alice_shared, bob_shared);
    }

    #[test]
    fn save_then_load_round_trips_to_the_same_keys() {
        let path = std::env::temp_dir().join(format!(
            "siar-crypto-test-identity-{}.bin",
            std::process::id()
        ));
        let original = DeviceIdentity::generate();
        original.save_to_file(&path).expect("save should succeed");

        let loaded = DeviceIdentity::load_from_file(&path).expect("load should succeed");
        std::fs::remove_file(&path).ok();

        assert_eq!(original.verifying_key(), loaded.verifying_key());
        assert_eq!(
            original.x25519_public().as_bytes(),
            loaded.x25519_public().as_bytes()
        );

        // The loaded key actually works, not just carries matching
        // public halves — sign with one, verify with the other's public
        // key, same as any real cross-process reuse would.
        let sig = loaded.sign(b"round trip");
        assert!(DeviceIdentity::verify(&original.verifying_key(), b"round trip", &sig).is_ok());
    }

    #[test]
    fn load_rejects_a_file_of_the_wrong_length() {
        let path = std::env::temp_dir().join(format!(
            "siar-crypto-test-bad-identity-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, [0u8; 10]).expect("write should succeed");
        let err = DeviceIdentity::load_from_file(&path);
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, Err(CryptoError::MalformedKey)));
    }

    #[test]
    fn try_clone_produces_an_independent_identity_with_matching_keys() {
        let original = DeviceIdentity::generate();
        let cloned = original.try_clone().expect("try_clone should succeed");

        assert_eq!(original.verifying_key(), cloned.verifying_key());
        assert_eq!(
            original.x25519_public().as_bytes(),
            cloned.x25519_public().as_bytes()
        );

        // Actually usable independently, not just matching public
        // halves — same "sign with one, verify with the other" check
        // save_then_load_round_trips_to_the_same_keys uses above.
        let sig = cloned.sign(b"try_clone round trip");
        assert!(
            DeviceIdentity::verify(&original.verifying_key(), b"try_clone round trip", &sig)
                .is_ok()
        );
    }
}
