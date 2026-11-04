//! Advanced cryptography for Siar — every primitive comes from a maintained,
//! widely used upstream crate; **nothing here implements a cryptographic
//! primitive**. This crate only composes them and enforces the rules around
//! them (domain separation, canonical encodings, "verify both halves",
//! proof-of-possession before aggregation, replay windows).
//!
//! | Module | Upstream | Audit status of that upstream (as of 2026-11) |
//! |---|---|---|
//! | [`frost_root`] | `frost-ed25519` 3.0 (ZF FROST, RFC 9591) | NCC Group audited v0.6.0 of frost-core/frost-ed25519; **not** re-audited at 3.0 |
//! | [`hybrid`] | `ml-dsa` 0.1 (RustCrypto, FIPS 204) + `ed25519-dalek` 2 | `ml-dsa`: **never independently audited** (its own README says so); dalek: widely reviewed |
//! | [`bls_audit`] | `blst` 0.3 (supranational) | independent audit commissioned by the Ethereum Foundation / Protocol Labs |
//! | [`bbs_relay`] | `zkryptium` 0.8 (IETF BBS draft-14, on `bls12_381_plus`) | **no audit found**; actively maintained |
//!
//! The two **unaudited** rows are named here, not hidden: `ml-dsa` and the
//! BBS stack. The hybrid construction limits the blast radius of the first
//! (both halves must verify, so a break of ML-DSA alone forges nothing); the
//! BBS layer has no such backstop and should be treated as experimental until
//! an audit exists.
#![forbid(unsafe_code)]

pub mod bbs_relay;
pub mod bls_audit;
pub mod frost_root;
pub mod hybrid;

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid parameter: {0}")]
    InvalidParameter(&'static str),
    #[error("malformed encoding: {0}")]
    Malformed(&'static str),
    #[error("signature or proof did not verify")]
    VerificationFailed,
    #[error("upstream crypto error: {0}")]
    Upstream(String),
    #[error("operating-system randomness unavailable")]
    Randomness,
}

/// Fill from the OS CSPRNG. Centralised so every module draws randomness the
/// same way and none touches a userspace PRNG.
pub(crate) fn fill_random(buf: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(buf).map_err(|_| Error::Randomness)
}
