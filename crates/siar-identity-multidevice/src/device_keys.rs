//! §21 "New Device Key Generation": "The new device generates locally:
//! device signing key, transport key, session/bootstrap material,
//! local database key. Private keys remain local. The existing device
//! signs only the new public identity information."
//!
//! The piece this crate was missing before this round: [`crate::link_key`]
//! generates an *ephemeral* keypair for the linking handshake itself,
//! and [`crate::certificate::DeviceCertificate::issue`] can sign a device's
//! public key into the account — but nothing generated the actual,
//! permanent keys a newly-linked device would go on to use. This
//! module is that step, sitting between the two: a new device runs
//! [`generate_new_device_keys`] once, keeps the result
//! ([`NewDeviceKeys`]) entirely local, and sends only
//! [`NewDeviceKeys::public_keys`]'s output to whoever will call
//! `DeviceCertificate::issue` on it — matching the spec's own "private
//! keys remain local" rule structurally, not just by convention: there
//! is no function anywhere in this module that returns or serializes
//! the private half.

use ed25519_dalek::SigningKey;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::Zeroize;

/// Every key §21 lists, generated fresh, kept together so a caller
/// can't accidentally generate one without the others (a device with
/// a signing key but no transport key, or vice versa, is a broken
/// device — this type makes that combination unrepresentable).
///
/// `local_database_key` exists as a generated value only — this crate
/// has no local database to apply it to (a real `siar-storage`
/// integration would consume it; not attempted here, see this crate's
/// own top doc comment on scope), same "generates the value, doesn't
/// use it" honesty [`crate::link_key::EphemeralLinkKeyPair`] already
/// applies to its own ephemeral key material.
pub struct NewDeviceKeys {
    device_signing_key: SigningKey,
    transport_key: StaticSecret,
    local_database_key: [u8; 32],
}

impl NewDeviceKeys {
    pub fn public_keys(&self) -> NewDevicePublicKeys {
        NewDevicePublicKeys {
            device_signing_public: self.device_signing_key.verifying_key().to_bytes(),
            device_transport_public: X25519PublicKey::from(&self.transport_key).to_bytes(),
        }
    }

    /// The bytes [`crate::certificate::DeviceCertificate::issue`]'s
    /// `device_public_key` parameter expects — that certificate binds
    /// exactly one public key today, and this crate's own established
    /// meaning for it (every existing call site) is the device's
    /// *signing* key, not its transport key. Naming this explicitly
    /// here rather than leaving `public_keys()`'s two fields ambiguous
    /// about which one a caller should hand to `issue`.
    pub fn signing_public_key_bytes(&self) -> [u8; 32] {
        self.device_signing_key.verifying_key().to_bytes()
    }

    /// A real, named gap when this was first written: [`crate::
    /// certificate::DeviceCertificate`] only certifies the signing key
    /// (see [`NewDeviceKeys::signing_public_key_bytes`]'s own doc
    /// comment) — the device's transport public key traveled alongside
    /// a certificate but wasn't itself bound by anything. Closed by
    /// [`NewDeviceKeys::bind_transport_key`], added alongside
    /// [`crate::transport_key_binding::TransportKeyBinding`] — see that
    /// module's own doc comment for why it's a second, device-signed
    /// binding rather than a new field on `DeviceCertificate` itself.
    pub fn transport_public_key_bytes(&self) -> [u8; 32] {
        X25519PublicKey::from(&self.transport_key).to_bytes()
    }

    /// Signs [`Self::transport_public_key_bytes`] with this device's
    /// own signing key, scoped to `certificate`'s generation — see
    /// [`crate::transport_key_binding::TransportKeyBinding`]'s own doc
    /// comment for the full reasoning. Errors with
    /// [`crate::error::IdentityError::CertificateNotForThisDevice`] if
    /// `certificate` wasn't actually issued for this device's own
    /// signing key — signing a binding against someone else's
    /// certificate would produce a value that could never verify, so
    /// this is rejected up front rather than handed back as a
    /// certificate-shaped value that just happens to always fail
    /// [`crate::transport_key_binding::TransportKeyBinding::verify`].
    pub fn bind_transport_key(
        &self,
        certificate: &crate::certificate::DeviceCertificate,
    ) -> Result<crate::transport_key_binding::TransportKeyBinding, crate::error::IdentityError>
    {
        if certificate.device_public_key != self.signing_public_key_bytes() {
            return Err(crate::error::IdentityError::CertificateNotForThisDevice);
        }
        Ok(crate::transport_key_binding::TransportKeyBinding::sign(
            &self.device_signing_key,
            certificate.device_id,
            certificate.generation,
            self.transport_public_key_bytes(),
        ))
    }
}

impl Drop for NewDeviceKeys {
    fn drop(&mut self) {
        // `SigningKey`/`StaticSecret` already zeroize themselves on
        // drop given this crate's own "zeroize" feature on
        // `ed25519-dalek`/`x25519-dalek` — matching
        // `RootIdentityKey`'s own identical `Drop` impl and identical
        // reasoning (defense-in-depth, not the primary mechanism).
        // `local_database_key` is a bare array with no such built-in
        // behavior, so it's zeroized explicitly here instead.
        self.local_database_key.zeroize();
    }
}

/// What actually leaves the device — every field here is, by
/// definition, safe to hand to whoever will call
/// [`crate::certificate::DeviceCertificate::issue`] (or, for
/// `device_transport_public`, whoever a real design eventually has
/// bind it — see [`NewDeviceKeys::transport_public_key_bytes`]'s own
/// doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDevicePublicKeys {
    pub device_signing_public: [u8; 32],
    pub device_transport_public: [u8; 32],
}

pub fn generate_new_device_keys() -> NewDeviceKeys {
    let mut local_database_key = [0u8; 32];
    OsRng.fill_bytes(&mut local_database_key);
    NewDeviceKeys {
        device_signing_key: SigningKey::generate(&mut OsRng),
        transport_key: StaticSecret::random_from_rng(OsRng),
        local_database_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::DeviceCapabilitySet;
    use crate::certificate::DeviceCertificate;
    use crate::root_key::RootIdentityKey;
    use siar_domain::{AccountId, DeviceId};

    #[test]
    fn generated_keys_produce_a_consistent_public_bundle() {
        let keys = generate_new_device_keys();
        let public = keys.public_keys();
        assert_eq!(
            public.device_signing_public,
            keys.signing_public_key_bytes()
        );
        assert_eq!(
            public.device_transport_public,
            keys.transport_public_key_bytes()
        );
    }

    #[test]
    fn two_generations_never_produce_the_same_keys() {
        let a = generate_new_device_keys();
        let b = generate_new_device_keys();
        assert_ne!(a.public_keys(), b.public_keys());
    }

    /// The real, end-to-end coherence check this module exists for:
    /// a new device generates its own keys locally, hands only the
    /// public half to an existing device, which issues a real
    /// certificate against it — the full §21 -> §8 pipeline, not two
    /// modules that merely compile against each other's types.
    #[test]
    fn a_new_devices_generated_signing_key_can_be_certified_end_to_end() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device_id = DeviceId::new();

        let new_device_keys = generate_new_device_keys();
        let public = new_device_keys.public_keys();

        let certificate = DeviceCertificate::issue(
            &root,
            account,
            device_id,
            public.device_signing_public,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        assert!(certificate
            .verify_signature(&root.root_public_key())
            .is_ok());
        assert_eq!(
            certificate.device_public_key,
            new_device_keys.signing_public_key_bytes()
        );
    }
}
