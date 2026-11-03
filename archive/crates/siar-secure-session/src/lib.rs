//! # siar-secure-session — the stateful half of the ratchet
//!
//! `siar-ratchet` is pure cryptography. A messenger also needs to *keep* the
//! state: encrypted at rest, safe across crashes, safe against a restored
//! backup, with signed prekeys that rotate. This crate is that layer, and
//! [`SecureSessions`] is the one entry point the message path should use.
//!
//! | piece | spec |
//! |---|---|
//! | [`cipher`] — XChaCha20-Poly1305 at rest, per-purpose keys, AAD-bound blobs | §50-§51, §44 |
//! | [`store`] — crash-safe blob store (fsync + atomic rename), fault injection | §75 |
//! | [`vault`] — encrypted state + high-water marks outside the backup set | §24, §103 |
//! | [`prekeys`] — signed-prekey publication, rotation, bounded retention | §11, §46 |
//! | [`sessions`] — persist-before-send, commit-before-save, per-session lock | §75-§78 |
//!
//! ## Honest scope
//!
//! * **Not yet called by `siar-messaging`.** What exists is *publication* (a
//!   ticket can carry the signed prekey bundle) and this storage layer. The
//!   cut-over also needs the message **storage format** to change: history is
//!   currently kept as session ciphertext and re-decrypted on demand, which a
//!   ratchet makes impossible (keys are deleted on use). See ROADMAP.
//! * The high-water-mark store must genuinely sit **outside backups**; this
//!   crate cannot enforce where the caller puts it.
//! * Sessions are 1:1 only; group traffic stays on MLS.

#![forbid(unsafe_code)]

pub mod cipher;
pub mod prekeys;
pub mod sessions;
pub mod store;
pub mod vault;

pub use cipher::{AtRestKey, CipherError};
pub use prekeys::{PrekeyError, PrekeyManager, PrekeyPolicy};
pub use sessions::{
    CommitError, Introduction, PeerRef, ReceiveKind, Received, SecureSessions, SessionError,
};
pub use store::{BlobStore, FileBlobStore, MemBlobStore};
pub use vault::{SessionId, SessionVault, VaultError};

#[cfg(test)]
mod tests;
