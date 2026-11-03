//! # siar-ratchet — one-to-one E2EE with forward secrecy (Part 28 §11-§17, §75-§78)
//!
//! Closes the gap `siar-crypto` documents about itself: its `Session` is a
//! static-key AEAD channel (confidentiality and integrity, **no forward
//! secrecy**). This crate is the Double Ratchet it said was still unbuilt,
//! plus the authenticated prekey handshake that starts it.
//!
//! | piece | spec |
//! |---|---|
//! | [`handshake`] — signed prekey bundles, 3-DH agreement, transcript binding | §11, §15, §37-§41, §115 |
//! | [`state`] — Double Ratchet: FS, post-compromise recovery, replay, nonce safety | §12-§17 |
//! | functional transitions (`encrypt`/`decrypt` return the *next* state) | §75-§77 |
//! | [`vault`] — high-water marks against stale restores | §24, §103 |
//! | [`kdf`] — versioned labels, suite in every derivation, no downgrade | §44, §46-§48 |
//!
//! ## Honest scope
//!
//! * **Not yet wired into `siar-messaging`.** `MessageService` still uses
//!   `siar_crypto::Session`; switching it is a wire-format change and needs a
//!   migration plan (see ROADMAP).
//! * **Not externally reviewed.** Primitives are the audited RustCrypto/dalek
//!   implementations and the construction follows Signal's published
//!   specification, but the composition here has had no third-party review
//!   (§123 Phase 9). Treat it as ready for review, not as reviewed.
//! * **No header encryption**: the DH public key and counters are visible to a
//!   network observer (authenticated, not secret).
//! * **No one-time prekeys**: see [`handshake`] for what that costs.
//! * Skipped-key storage is bounded (`MAX_SKIP` per jump, 2000 total); a peer
//!   that exceeds it forces re-establishment (§78).

#![forbid(unsafe_code)]

pub mod handshake;
pub mod kdf;
pub mod state;
pub mod vault;

pub use handshake::{
    initiate, new_signed_prekey, respond, HandshakeError, InitialHeader, PrekeyBundle,
    SignedPrekeySecret,
};
pub use state::{RatchetError, RatchetState, MAX_SKIP, MAX_STORED_SKIPPED, SUITE, VERSION};
pub use vault::HighWaterMarks;

#[cfg(test)]
mod tests;
