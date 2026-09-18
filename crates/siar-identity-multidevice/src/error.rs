//! Typed errors for this crate — Part 01's own §27/§93 precedent
//! (`siar-protocol-ext`'s `ExtensionError`, stable
//! extension-scoped error codes) applied here: no `anyhow`, matching
//! §190 "No `anyhow` in Public Domain API" from this crate's own spec
//! document.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("malformed key bytes")]
    MalformedKey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("device certificate's account_id does not match the expected account")]
    AccountMismatch,
    /// §56 "Rollback Protection": a directory generation at or below one
    /// already trusted for this account.
    #[error("directory generation {given} is not newer than the highest trusted generation {highest} for this account — rejected to prevent rollback")]
    RollbackRejected { given: u64, highest: u64 },
    #[error("directory signature did not verify against the account's trusted root public key")]
    DirectorySignatureInvalid,
    /// Used by [`crate::revocation::verify_revocation`] — a directory
    /// claims to reflect a specific revocation but its actual contents
    /// don't match (the target device isn't `Revoked`, or an unrelated
    /// device's status changed too). Deliberately its own variant
    /// rather than reusing `AccountMismatch` — that one means something
    /// different (a certificate's `account_id` field doesn't match),
    /// and conflating the two would make error handling silently wrong
    /// for a caller matching on this enum.
    #[error("directory does not correctly reflect the claimed revocation")]
    RevocationMismatch,
    /// §57 "Fork Detection": two different signed directories for the
    /// same account at the same generation number — a bug, a
    /// compromise, or a concurrent invalid update, per spec §57's own
    /// three named causes. §57's own explicit instruction: "do not
    /// silently choose one... require reconciliation/security
    /// handling" — this variant, not a silent pick, is that
    /// requirement.
    #[error("identity fork detected for this account at generation {generation}: two different signed directories claim the same generation — do not silently choose one, this requires reconciliation/security handling")]
    IdentityForkDetected { generation: u64 },
    /// [`crate::transport_key_binding::TransportKeyBinding::verify`] —
    /// the binding's own `device_id`/`certificate_generation` don't
    /// match the [`crate::certificate::DeviceCertificate`] it's being
    /// checked against. Distinct from `InvalidSignature`: the
    /// signature bytes might be perfectly valid, just for a different
    /// certificate than the one the caller is checking against here.
    #[error("transport key binding's device_id/generation does not match the certificate it was checked against")]
    TransportKeyBindingMismatch,
    /// [`crate::device_keys::NewDeviceKeys::bind_transport_key`] — the
    /// certificate presented was not issued for *this* device's own
    /// signing key. Distinct from `TransportKeyBindingMismatch`: that
    /// one is about an existing binding checked against the wrong
    /// certificate; this one is caught before a binding is even
    /// created, so signing anything would be meaningless.
    #[error("certificate's device_public_key does not match this device's own signing key")]
    CertificateNotForThisDevice,
}
