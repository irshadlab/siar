//! Closes a real, previously-named gap (see
//! [`crate::device_keys::NewDeviceKeys::transport_public_key_bytes`]'s
//! history — its doc comment used to say this wasn't attempted) left
//! by retiring the plan.md-era `siar_crypto::device_cert` model (see
//! `MIGRATION.md`'s device-certificate reconciliation section): that
//! older model's `issue_device_certificate` signed a device's signing
//! key *and* its X25519 transport key together in one certificate, so
//! verifying the certificate authenticated both keys as a pair. §8's
//! own conceptual `DeviceCertificate` struct binds exactly one
//! `device_public_key`, and [`crate::certificate::DeviceCertificate`]
//! follows that literally — so once the old dual-key model was gone,
//! nothing anywhere bound a device's transport key to anything at all.
//!
//! **Deliberately not closed by adding a second field to
//! `DeviceCertificate` itself.** That would mean re-involving the
//! account's root key (§6: "rarely online, not every session")
//! whenever a transport key needs binding — fine for today, where
//! [`crate::device_keys::generate_new_device_keys`] only ever produces
//! both keys once, together, at device-creation time, but a needless
//! future constraint if transport keys are ever rotated independently
//! of a full recertification. Instead, [`TransportKeyBinding`] is
//! signed by the device's *own* signing key — the one
//! [`crate::certificate::DeviceCertificate`] already root-certifies —
//! producing a two-hop chain (root key → device signing key → device
//! transport key) rather than extending the root-signed certificate's
//! own schema. Additive: zero existing `DeviceCertificate` call sites
//! change.

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::certificate::DeviceCertificate;
use crate::error::IdentityError;
use siar_domain::DeviceId;

fn signing_payload(
    device_id: DeviceId,
    certificate_generation: u64,
    transport_public_key: &[u8; 32],
) -> Vec<u8> {
    #[derive(Serialize)]
    struct Payload {
        device_id: DeviceId,
        certificate_generation: u64,
        transport_public_key: [u8; 32],
    }
    postcard::to_allocvec(&Payload {
        device_id,
        certificate_generation,
        transport_public_key: *transport_public_key,
    })
    .expect("postcard encoding of a fixed-shape struct cannot fail")
}

/// A device's own signing key vouching for its transport key, scoped to
/// one specific [`DeviceCertificate`] generation (see this module's top
/// doc comment for why the root key isn't involved directly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportKeyBinding {
    pub device_id: DeviceId,
    /// The [`DeviceCertificate::generation`] this binding was made
    /// under — checked in [`Self::verify`] against the certificate
    /// presented, so a binding can't be replayed against a later
    /// certificate for the same device issued at a different
    /// generation (e.g. after a key rotation).
    pub certificate_generation: u64,
    pub transport_public_key: [u8; 32],
    /// 64 raw signature bytes as `Vec<u8>` — same reason
    /// [`DeviceCertificate::signature`]'s own doc comment already
    /// gives (confirmed by a real compile error against that exact
    /// field, not a guess): serde's derive doesn't support arrays
    /// longer than 32 elements without an extra crate.
    pub signature: Vec<u8>,
}

impl TransportKeyBinding {
    /// Called with the device's own private signing key — see
    /// [`crate::device_keys::NewDeviceKeys::bind_transport_key`] for
    /// the one real caller, which is the only place in this crate that
    /// actually holds that key.
    pub(crate) fn sign(
        device_signing_key: &SigningKey,
        device_id: DeviceId,
        certificate_generation: u64,
        transport_public_key: [u8; 32],
    ) -> Self {
        let payload = signing_payload(device_id, certificate_generation, &transport_public_key);
        let signature = device_signing_key.sign(&payload).to_bytes().to_vec();
        Self {
            device_id,
            certificate_generation,
            transport_public_key,
            signature,
        }
    }

    /// Verifies this binding against `certificate` — checks that the
    /// two agree on `device_id`/`generation` (see
    /// [`IdentityError::TransportKeyBindingMismatch`]), then verifies
    /// the signature against `certificate.device_public_key`. Does
    /// **not** itself verify `certificate`'s own signature against the
    /// account's root key, or check expiry/revocation — same
    /// deliberate separation of concerns
    /// [`DeviceCertificate::verify_signature`]'s own doc comment
    /// already establishes for those checks; a caller wanting the full
    /// chain calls all three.
    pub fn verify(&self, certificate: &DeviceCertificate) -> Result<(), IdentityError> {
        if self.device_id != certificate.device_id
            || self.certificate_generation != certificate.generation
        {
            return Err(IdentityError::TransportKeyBindingMismatch);
        }
        let payload = signing_payload(
            self.device_id,
            self.certificate_generation,
            &self.transport_public_key,
        );
        let signature_bytes: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::MalformedKey)?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);
        let verifying_key = VerifyingKey::from_bytes(&certificate.device_public_key)
            .map_err(|_| IdentityError::MalformedKey)?;
        verifying_key
            .verify(&payload, &signature)
            .map_err(|_| IdentityError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::DeviceCapabilitySet;
    use crate::device_keys::generate_new_device_keys;
    use crate::root_key::RootIdentityKey;
    use siar_domain::AccountId;

    #[test]
    fn a_transport_key_binding_verifies_against_its_devices_certificate() {
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
        let binding = new_device_keys
            .bind_transport_key(&certificate)
            .expect("this device's own certificate should bind cleanly");

        assert!(binding.verify(&certificate).is_ok());
        assert_eq!(binding.transport_public_key, public.device_transport_public);
    }

    #[test]
    fn a_binding_does_not_verify_against_a_different_devices_certificate() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device_id = DeviceId::new();
        let other_device_id = DeviceId::new();
        let new_device_keys = generate_new_device_keys();
        let other_new_device_keys = generate_new_device_keys();

        let certificate = DeviceCertificate::issue(
            &root,
            account,
            device_id,
            new_device_keys.public_keys().device_signing_public,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let other_certificate = DeviceCertificate::issue(
            &root,
            account,
            other_device_id,
            other_new_device_keys.public_keys().device_signing_public,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let binding = new_device_keys.bind_transport_key(&certificate).unwrap();

        assert_eq!(
            binding.verify(&other_certificate),
            Err(IdentityError::TransportKeyBindingMismatch)
        );
    }

    #[test]
    fn a_binding_does_not_verify_after_the_certificate_rotates_generation() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device_id = DeviceId::new();
        let new_device_keys = generate_new_device_keys();
        let public = new_device_keys.public_keys();

        let certificate_gen1 = DeviceCertificate::issue(
            &root,
            account,
            device_id,
            public.device_signing_public,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let binding = new_device_keys
            .bind_transport_key(&certificate_gen1)
            .unwrap();

        let certificate_gen2 = DeviceCertificate::issue(
            &root,
            account,
            device_id,
            public.device_signing_public,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            2,
        );
        assert_eq!(
            binding.verify(&certificate_gen2),
            Err(IdentityError::TransportKeyBindingMismatch)
        );
    }

    #[test]
    fn tampering_with_the_transport_key_invalidates_the_signature() {
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
        let mut binding = new_device_keys.bind_transport_key(&certificate).unwrap();
        binding.transport_public_key = [0xffu8; 32];

        assert_eq!(
            binding.verify(&certificate),
            Err(IdentityError::InvalidSignature)
        );
    }

    #[test]
    fn binding_fails_when_the_certificate_was_not_issued_for_this_devices_signing_key() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device_id = DeviceId::new();
        let new_device_keys = generate_new_device_keys();
        let some_other_signing_key = [9u8; 32]; // not this device's real signing key

        let certificate = DeviceCertificate::issue(
            &root,
            account,
            device_id,
            some_other_signing_key,
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        assert_eq!(
            new_device_keys
                .bind_transport_key(&certificate)
                .unwrap_err(),
            IdentityError::CertificateNotForThisDevice
        );
    }
}
