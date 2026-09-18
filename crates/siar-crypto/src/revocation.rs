//! Device revocation record and propagation mitigation (Part 28 §20,
//! §21).
//!
//! §20's revocation *mechanics* — "revoke DeviceId, rotate affected
//! security state, stop accepting new sessions" — are already real,
//! working code in `siar_identity_multidevice::revocation::revoke_device`
//! (that crate's own Part 02 §25-29). This module is deliberately not a
//! second implementation of that: it builds the one piece §20 asks for
//! that the directory-generation model doesn't produce — a standalone,
//! individually-signed `DeviceRevocation` record matching this spec's
//! own literal shape (`account`, `device`, `revoked_at`, `reason`,
//! `signature`) — as an artifact built *from* a real
//! `revoke_device`/`verify_revocation` outcome, not an alternative to
//! it. Where a `DeviceDirectory` update is the account's whole current
//! device-trust state, a `DeviceRevocation` is a single self-contained,
//! independently-verifiable fact ("this one device was revoked, at this
//! time, for this reason, and here's the signature proving the account's
//! root key said so") — useful anywhere a full directory snapshot would
//! be overkill: a gossip message, an audit log entry, an offline
//! signed file.
//!
//! §21 "Revocation Propagation" is honest about its own limits: "use
//! all available paths... a disconnected peer may not learn instantly."
//! The actual multi-path propagation (direct P2P / relay / DTN /
//! organization policy / offline signed file or QR) needs transport
//! infrastructure this crate doesn't have and isn't attempted here. What
//! *is* buildable now is §21's own suggested mitigation — "shorter-lived
//! device credentials or security epochs can reduce risk" — which this
//! module wires directly to §22's `SecurityEpoch`: producing a
//! `DeviceRevocation` always returns the account's next epoch alongside
//! it, and `is_epoch_stale_after_revocation` lets a caller decide
//! whether a message's `SecurityEpoch` (from `envelope.rs`) is recent
//! enough to trust without an explicit, confirmed revocation check —
//! not full propagation, but a real, checkable bound on exposure while
//! propagation is still in flight.

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};
use siar_identity_multidevice::RootIdentityKey;

use crate::epoch::SecurityEpoch;

/// §20's reasons, kept intentionally small — a real cause a caller can
/// branch on (e.g. surface different UI copy or different follow-up
/// actions for a self-initiated removal versus a suspected compromise),
/// not a free-form string a signature would authenticate the exact
/// wording of forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevocationReason {
    /// The account owner removed a device they still control (e.g.
    /// upgrading phones).
    UserInitiated,
    /// The device is lost or believed stolen.
    LostOrStolen,
    /// The device's key material is believed compromised without the
    /// device itself necessarily being lost.
    SuspectedCompromise,
    /// Revoked by organization/account policy rather than by the
    /// device's own owner (§21 lists "organization policy" as a
    /// propagation path, implying policy-driven revocation is a real,
    /// distinct case from the other three).
    OrganizationPolicy,
}

/// §20's literal DTO, field-for-field: `account`, `device`,
/// `revoked_at`, `reason`, `signature`. `revoked_at` follows this
/// workspace's existing convention of caller-supplied `u64` millis
/// (matching `siar-crash-recovery`'s `*_millis` fields) rather than
/// reading the wall clock inside this crate — keeps this testable
/// without time mocking, same as every other timestamped type here.
///
/// `signature` is `Vec<u8>`, not `[u8; 64]` — the same serde derive
/// limitation every other 64-byte signature in this workspace already
/// works around this way (see
/// `siar_identity_multidevice::certificate`, `::directory`, `::invite`:
/// serde's built-in array impls stop at 32 elements; a 64-byte array
/// needs either `serde-big-array` or, as here, storing `Vec<u8>` on the
/// wire and converting to `[u8; 64]` at the point of use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRevocation {
    pub account: AccountId,
    pub device: DeviceId,
    pub revoked_at_millis: u64,
    pub reason: RevocationReason,
    pub signature: Vec<u8>,
}

impl DeviceRevocation {
    /// Builds the exact bytes a `DeviceRevocation`'s signature covers.
    /// Fixed field order, same reasoning as `envelope.rs`'s associated
    /// data: this is part of the format, not something to reorder later
    /// without every previously-issued revocation's signature breaking.
    fn signable_bytes(
        account: AccountId,
        device: DeviceId,
        revoked_at_millis: u64,
        reason: RevocationReason,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + 16 + 8 + 1);
        bytes.extend_from_slice(account.as_uuid().as_bytes());
        bytes.extend_from_slice(device.as_uuid().as_bytes());
        bytes.extend_from_slice(&revoked_at_millis.to_be_bytes());
        bytes.push(reason as u8);
        bytes
    }

    /// Issues a signed revocation record using the account's root key —
    /// the same authority `siar_identity_multidevice::revoke_device`
    /// requires, since a `DeviceRevocation` is meant to be independently
    /// verifiable by anything that trusts that same root public key,
    /// with or without a full `DeviceDirectory` on hand.
    ///
    /// This does not itself call `revoke_device` — a caller performing
    /// a real revocation should call both: `revoke_device` to update the
    /// authoritative directory, and this to produce the portable,
    /// individually-verifiable record of that same event for gossip/
    /// audit/offline propagation. Keeping them separate rather than
    /// having one call the other keeps this crate free of a dependency
    /// on `DeviceDirectory`'s specific shape — it only needs the root
    /// key and the four facts §20 asks a revocation record to state.
    pub fn issue(
        root_key: &RootIdentityKey,
        account: AccountId,
        device: DeviceId,
        revoked_at_millis: u64,
        reason: RevocationReason,
    ) -> Self {
        let signable = Self::signable_bytes(account, device, revoked_at_millis, reason);
        let signature = root_key.sign(&signable);
        Self {
            account,
            device,
            revoked_at_millis,
            reason,
            signature: signature.to_vec(),
        }
    }

    /// Verifies this record's signature against `root_public_key`.
    /// Independent of, and not a substitute for,
    /// `siar_identity_multidevice::verify_revocation` — that function
    /// checks a *directory transition* is well-formed; this checks a
    /// *standalone record* is authentically signed. A caller receiving
    /// a `DeviceRevocation` via gossip with no directory to compare
    /// against (§21's whole point) can still verify it via this alone.
    pub fn verify(&self, root_public_key: &siar_identity_multidevice::RootPublicKey) -> bool {
        let signable = Self::signable_bytes(
            self.account,
            self.device,
            self.revoked_at_millis,
            self.reason,
        );
        let Ok(signature): Result<[u8; 64], _> = self.signature.clone().try_into() else {
            return false;
        };
        root_public_key.verify(&signable, &signature).is_ok()
    }
}

/// §21's own suggested mitigation, made concrete: given the epoch a
/// revocation advanced *to* (`revoked_into_epoch`) and the epoch stamped
/// on some other message or session (`observed_epoch`), is the observed
/// epoch stale enough that it might predate a revocation this caller
/// hasn't necessarily heard about yet?
///
/// This is a bound on exposure, not a substitute for actually checking
/// device trust: a caller with a fresh, confirmed `DeviceDirectory`
/// should prefer `is_device_trusted` over this. This function exists
/// for exactly the disconnected-peer case §21 describes, where no fresh
/// directory is available and a coarse, local check is the only option
/// while propagation is still in flight.
pub fn is_epoch_stale_after_revocation(
    observed_epoch: SecurityEpoch,
    revoked_into_epoch: SecurityEpoch,
) -> bool {
    observed_epoch < revoked_into_epoch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correctly_signed_revocation_verifies() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();

        let revocation = DeviceRevocation::issue(
            &root,
            account,
            device,
            1_700_000_000_000,
            RevocationReason::LostOrStolen,
        );

        assert!(revocation.verify(&root.root_public_key()));
    }

    #[test]
    fn a_revocation_signed_by_a_different_root_key_fails_verification() {
        let root = RootIdentityKey::generate();
        let other_root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();

        let revocation = DeviceRevocation::issue(
            &root,
            account,
            device,
            1_700_000_000_000,
            RevocationReason::SuspectedCompromise,
        );

        // Verified against the wrong account's root public key — must
        // not validate, or any account's revocations could be forged
        // by signing with an unrelated key.
        assert!(!revocation.verify(&other_root.root_public_key()));
    }

    #[test]
    fn tampering_with_the_reason_after_signing_invalidates_the_record() {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();

        let mut revocation = DeviceRevocation::issue(
            &root,
            account,
            device,
            1_700_000_000_000,
            RevocationReason::UserInitiated,
        );

        // Swap a benign self-removal into a compromise report after
        // the fact — the reason is part of what's signed, so this must
        // be caught, not silently accepted.
        revocation.reason = RevocationReason::SuspectedCompromise;
        assert!(!revocation.verify(&root.root_public_key()));
    }

    #[test]
    fn epoch_staleness_check_flags_pre_revocation_epochs() {
        let revoked_into = SecurityEpoch(5);
        assert!(is_epoch_stale_after_revocation(
            SecurityEpoch(4),
            revoked_into
        ));
        assert!(!is_epoch_stale_after_revocation(
            SecurityEpoch(5),
            revoked_into
        ));
        assert!(!is_epoch_stale_after_revocation(
            SecurityEpoch(6),
            revoked_into
        ));
    }
}
