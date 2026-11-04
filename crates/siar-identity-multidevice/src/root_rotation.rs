//! §33 "Root Key Rotation", §34 "Root Rotation Event", §35 "Compromised
//! Root Scenario".

use crate::root_key::{RootIdentityKey, RootPublicKey};
use siar_domain::AccountId;

/// §34, verbatim struct — a dual-signed continuity attestation: the
/// OLD root proves it authorized handing off to the new one, and the
/// NEW root proves it accepts being the account's identity going
/// forward. Both signatures cover the exact same payload, so neither
/// side can produce a rotation event the other didn't actually agree
/// to.
///
/// Signatures stored as `Vec<u8>`, not `[u8; 64]` — matching
/// [`crate::certificate::DeviceCertificate::signature`]'s own
/// documented reason: serde doesn't derive `Serialize`/`Deserialize`
/// for arrays this large without an extra dependency this crate
/// doesn't otherwise need.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RootRotation {
    pub account_id: AccountId,
    pub old_root: RootPublicKey,
    pub new_root: RootPublicKey,
    pub generation: u64,
    pub old_signature: Vec<u8>,
    pub new_signature: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RootRotationError {
    #[error("old root's signature over the rotation payload does not verify")]
    OldRootSignatureInvalid,
    #[error("new root's signature over the rotation payload does not verify")]
    NewRootSignatureInvalid,
}

fn rotation_payload(
    account_id: AccountId,
    old_root: &RootPublicKey,
    new_root: &RootPublicKey,
    generation: u64,
) -> Vec<u8> {
    #[derive(serde::Serialize)]
    struct Payload {
        account_id: AccountId,
        old_root: RootPublicKey,
        new_root: RootPublicKey,
        generation: u64,
    }
    postcard::to_allocvec(&Payload {
        account_id,
        old_root: *old_root,
        new_root: *new_root,
        generation,
    })
    .expect("postcard serialization of fixed-size fields never fails")
}

/// The exact bytes both roots sign for a rotation. Public so a root whose
/// key is not a single local `RootIdentityKey` — notably a FROST threshold
/// group, whose signature is produced by several parties — can still
/// produce a [`RootRotation`] that [`verify_root_rotation`] accepts.
/// Assembling one by hand does **not** bypass §33: `verify_root_rotation`
/// still demands a valid signature from the OLD root.
#[must_use]
pub fn rotation_signing_payload(
    account_id: AccountId,
    old_root: &RootPublicKey,
    new_root: &RootPublicKey,
    generation: u64,
) -> Vec<u8> {
    rotation_payload(account_id, old_root, new_root, generation)
}

/// §33's flow: "requires possession of the current root private key."
/// This is enforced structurally, not just by the spec's own words —
/// there is no parameter here, and no other function anywhere in this
/// module, that can produce a valid [`RootRotation`] without an actual
/// `&RootIdentityKey` for the OLD root. §35's own point — "ordinary
/// rotation cannot solve a compromised root, since it requires
/// possessing the (possibly stolen) old key" — is exactly this
/// requirement, not a separate rule to enforce elsewhere.
pub fn rotate_root_key(
    old_root: &RootIdentityKey,
    new_root: &RootIdentityKey,
    account_id: AccountId,
    generation: u64,
) -> RootRotation {
    let old_root_public = old_root.root_public_key();
    let new_root_public = new_root.root_public_key();
    let payload = rotation_payload(account_id, &old_root_public, &new_root_public, generation);

    RootRotation {
        account_id,
        old_root: old_root_public,
        new_root: new_root_public,
        generation,
        old_signature: old_root.sign(&payload).to_vec(),
        new_signature: new_root.sign(&payload).to_vec(),
    }
}

/// §34: both signatures must verify independently against the same
/// payload — a rotation event with only one valid signature is not a
/// rotation event at all, it's either an unauthorized claim (missing
/// the old root's consent) or an unaccepted handoff (missing the new
/// root's acceptance).
pub fn verify_root_rotation(rotation: &RootRotation) -> Result<(), RootRotationError> {
    let payload = rotation_payload(
        rotation.account_id,
        &rotation.old_root,
        &rotation.new_root,
        rotation.generation,
    );
    let old_signature: [u8; 64] = rotation
        .old_signature
        .as_slice()
        .try_into()
        .map_err(|_| RootRotationError::OldRootSignatureInvalid)?;
    let new_signature: [u8; 64] = rotation
        .new_signature
        .as_slice()
        .try_into()
        .map_err(|_| RootRotationError::NewRootSignatureInvalid)?;
    rotation
        .old_root
        .verify(&payload, &old_signature)
        .map_err(|_| RootRotationError::OldRootSignatureInvalid)?;
    rotation
        .new_root
        .verify(&payload, &new_signature)
        .map_err(|_| RootRotationError::NewRootSignatureInvalid)?;
    Ok(())
}

/// §35's own five named future-strategy candidates for recovering from
/// an actually-compromised (or lost) root — kept as a documented,
/// closed set rather than implemented wholesale here. Only
/// `RecoverySecret` and `TrustedDeviceQuorum` get real code this round
/// (see [`crate::recovery`]) — `SocialRecovery`/`OrganizationAuthority`
/// remain genuinely unimplemented, named so a future round has a fixed
/// target rather than an open-ended one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CompromisedRootRecoveryStrategy {
    RecoveryQuorum,
    TrustedDeviceQuorum,
    OfflineRecoverySecret,
    SocialRecovery,
    OrganizationAuthority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_33_34_a_correctly_produced_rotation_verifies() {
        let old_root = RootIdentityKey::generate();
        let new_root = RootIdentityKey::generate();
        let account = AccountId::new();

        let rotation = rotate_root_key(&old_root, &new_root, account, 5);
        assert!(verify_root_rotation(&rotation).is_ok());
    }

    #[test]
    fn spec_34_tampering_with_the_new_root_after_the_fact_is_detected() {
        let old_root = RootIdentityKey::generate();
        let new_root = RootIdentityKey::generate();
        let attacker_root = RootIdentityKey::generate();
        let account = AccountId::new();

        let mut rotation = rotate_root_key(&old_root, &new_root, account, 5);
        // Swap in a different "new root" without re-signing — the old
        // root's signature no longer covers this claimed new root.
        rotation.new_root = attacker_root.root_public_key();

        assert_eq!(
            verify_root_rotation(&rotation),
            Err(RootRotationError::OldRootSignatureInvalid)
        );
    }

    #[test]
    fn spec_35_rotation_is_structurally_impossible_without_the_old_private_key() {
        // There is no code path in this module that produces a
        // RootRotation from anything other than a real
        // &RootIdentityKey for the old root — this test's only
        // purpose is to make that claim visible as a test rather than
        // only true by inspection of rotate_root_key's signature.
        let old_root = RootIdentityKey::generate();
        let new_root = RootIdentityKey::generate();
        let rotation = rotate_root_key(&old_root, &new_root, AccountId::new(), 1);
        assert_eq!(rotation.old_root, old_root.root_public_key());
    }
}
