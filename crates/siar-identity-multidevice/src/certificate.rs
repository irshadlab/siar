//! §8 "Device Certificate", §9 "Device Certificate Semantics", §30
//! "Device Expiry".
//!
//! **Resolved reconciliation** (see `MIGRATION.md`'s device-certificate
//! section): this workspace used to also have a *different* certificate
//! model, `siar_crypto::device_cert::DeviceCertificate` — device-
//! vouches-for-device, no account root key involved, built against an
//! older doc ("plan.md §42"). That model has been retired in favor of
//! this one (root-key-signed, per §6 "Root Key Strategy": the
//! account's root identity key signs every device certificate, and is
//! used for little else) — this is now the sole device-certificate
//! model in this workspace. The one real capability the old model had
//! that this one's literal §8 struct doesn't — binding a device's
//! transport key alongside its signing key in one certificate — is
//! closed separately by [`crate::transport_key_binding::
//! TransportKeyBinding`], not by extending this struct; see that
//! module's own doc comment for why.

use serde::{Deserialize, Serialize};

use crate::capability::DeviceCapabilitySet;
use crate::error::IdentityError;
use crate::root_key::{RootIdentityKey, RootPublicKey};
use siar_domain::{AccountId, DeviceId};

/// §8: binds Account → Device → device public key, signed by the
/// account's root key (§6) at a given [`DeviceCertificate::generation`]
/// (§24).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCertificate {
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub device_public_key: [u8; 32],
    pub issued_at_millis: u64,
    pub expires_at_millis: Option<u64>,
    pub capabilities: DeviceCapabilitySet,
    pub generation: u64,
    /// 64 raw signature bytes as `Vec<u8>`, not `[u8; 64]` — serde's
    /// derive doesn't implement `Serialize`/`Deserialize` for arrays
    /// past 32 elements without an extra crate (confirmed by a real
    /// compile error against this exact field, not a guess).
    /// [`crate::transport_key_binding::TransportKeyBinding::signature`]
    /// follows the same convention. `verify_signature`
    /// enforces it's actually 64 bytes.
    pub signature: Vec<u8>,
}

impl DeviceCertificate {
    /// Postcard, not ad-hoc byte concatenation — a fixed-shape
    /// struct-of-fields signing payload is easy to get subtly wrong by
    /// hand (field order, a missing field); postcard-encoding the exact
    /// same value that ends up on the certificate is what
    /// `verify_signature` re-derives too, so the two can never drift
    /// out of sync with each other the way two independently
    /// hand-written byte layouts could. Matches Part 01's §92 "Postcard
    /// Rules" (fixed-width integers, no `usize` on the wire — every
    /// field here already satisfies that).
    fn signing_payload(
        account_id: AccountId,
        device_id: DeviceId,
        device_public_key: &[u8; 32],
        issued_at_millis: u64,
        expires_at_millis: Option<u64>,
        capabilities: DeviceCapabilitySet,
        generation: u64,
    ) -> Vec<u8> {
        #[derive(Serialize)]
        struct Payload {
            account_id: AccountId,
            device_id: DeviceId,
            device_public_key: [u8; 32],
            issued_at_millis: u64,
            expires_at_millis: Option<u64>,
            capabilities: DeviceCapabilitySet,
            generation: u64,
        }
        postcard::to_allocvec(&Payload {
            account_id,
            device_id,
            device_public_key: *device_public_key,
            issued_at_millis,
            expires_at_millis,
            capabilities,
            generation,
        })
        .expect("postcard encoding of a fixed-shape struct cannot fail")
    }

    #[allow(clippy::too_many_arguments)]
    /// Issues a certificate — called on whatever holds the account's
    /// root key (§6: rarely online, not every session) binding a new
    /// device's public key to this account at a given `generation`
    /// (§24: the account's own monotonic counter, incremented by
    /// whoever calls this — this function does not itself track or
    /// enforce monotonicity; see [`crate::trust_store::TrustedAccountStore`]
    /// for the receiving side's enforcement, §56).
    pub fn issue(
        root_key: &RootIdentityKey,
        account_id: AccountId,
        device_id: DeviceId,
        device_public_key: [u8; 32],
        issued_at_millis: u64,
        expires_at_millis: Option<u64>,
        capabilities: DeviceCapabilitySet,
        generation: u64,
    ) -> Self {
        let payload = Self::signing_payload(
            account_id,
            device_id,
            &device_public_key,
            issued_at_millis,
            expires_at_millis,
            capabilities,
            generation,
        );
        let signature = root_key.sign(&payload).to_vec();
        Self {
            account_id,
            device_id,
            device_public_key,
            issued_at_millis,
            expires_at_millis,
            capabilities,
            generation,
            signature,
        }
    }

    /// §9: proves only "this device key belongs to this logical
    /// account" at the certificate's own `generation` — NOT that the
    /// device is currently trusted or unrevoked (that's
    /// [`crate::trust_store::TrustedAccountStore`]'s job, a deliberately
    /// separate check per §9's own explicit rule) and NOT that it
    /// hasn't expired (checked separately via
    /// [`DeviceCertificate::is_expired`], since "expired" and "invalid
    /// signature" are different failure modes a caller may want to
    /// handle differently — matching §30's "expiration is not a
    /// replacement for revocation," which only makes sense if the two
    /// checks are already independent of each other here, as the test
    /// below demonstrates).
    pub fn verify_signature(&self, root_public_key: &RootPublicKey) -> Result<(), IdentityError> {
        let payload = Self::signing_payload(
            self.account_id,
            self.device_id,
            &self.device_public_key,
            self.issued_at_millis,
            self.expires_at_millis,
            self.capabilities,
            self.generation,
        );
        let signature: [u8; 64] = self
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| IdentityError::MalformedKey)?;
        root_public_key.verify(&payload, &signature)
    }

    pub fn is_expired(&self, now_millis: u64) -> bool {
        self.expires_at_millis
            .map(|exp| now_millis >= exp)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert(
        root: &RootIdentityKey,
        account: AccountId,
        device: DeviceId,
        generation: u64,
    ) -> DeviceCertificate {
        DeviceCertificate::issue(
            root,
            account,
            device,
            [7u8; 32],
            1_000,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            generation,
        )
    }

    #[test]
    fn a_certificate_verifies_against_its_real_root_key() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let c = cert(&root, account, device, 1);
        assert!(c.verify_signature(&root.root_public_key()).is_ok());
    }

    #[test]
    fn a_certificate_does_not_verify_against_the_wrong_root_key() {
        let root = RootIdentityKey::generate();
        let impostor_root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let c = cert(&root, account, device, 1);
        assert!(c
            .verify_signature(&impostor_root.root_public_key())
            .is_err());
    }

    #[test]
    fn tampering_with_the_generation_invalidates_the_signature() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let mut c = cert(&root, account, device, 1);
        c.generation = 2;
        assert!(c.verify_signature(&root.root_public_key()).is_err());
    }

    #[test]
    fn expiry_check_is_independent_of_signature_validity() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let c = DeviceCertificate::issue(
            &root,
            account,
            device,
            [7u8; 32],
            1_000,
            Some(2_000),
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        assert!(c.verify_signature(&root.root_public_key()).is_ok());
        assert!(!c.is_expired(1_999));
        assert!(c.is_expired(2_000));
        // still verifies even though expired — §9/§30: separate checks
        assert!(c.verify_signature(&root.root_public_key()).is_ok());
    }
}
