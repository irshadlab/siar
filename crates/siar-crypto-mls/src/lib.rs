//! Group end-to-end encryption via MLS (RFC 9420) — architecture.md
//! §28/§106 and next.md §28 ("Design a scalable group-key protocol...
//! Old members must not decrypt future epochs").
//!
//! ## Why this took until now, and why it's careful rather than fast
//!
//! Every other JNI/transport/framing crate built in this workspace so
//! far is either pure application logic this workspace owns outright,
//! or a thin JNI shell whose worst failure mode is a build error or a
//! dropped packet. Group cryptography is different in kind: getting it
//! subtly wrong doesn't fail loudly, it silently weakens the exact
//! guarantee (§28's forward secrecy / post-compromise security across
//! membership changes) the whole feature exists to provide. That's why
//! `siar_domain::group`'s module doc has, until now, deliberately left
//! this as "an `openmls`-backed integration done with a compiler in
//! hand, not hand-rolled in this session."
//!
//! What changed: this workspace's sandbox has outbound access to
//! `github.com`/`raw.githubusercontent.com`, so rather than writing
//! this from memory of openmls's API shape, every type, method
//! signature, and version pin in this crate was checked directly
//! against openmls's own `main`-branch source on GitHub — specifically
//! `openmls/openmls/tests/book_code.rs`, the source file the project's
//! **own published documentation book is generated from** (each doc
//! page embeds `{{#include
//! ../../../openmls/tests/book_code.rs:anchor_name}}` sections of it
//! directly), meaning it's exercised by openmls's own CI, not merely
//! "an example that happened to look right." Every non-trivial call in
//! `group.rs` below has a matching line in that file. That's a
//! materially stronger basis than usual for this workspace's "verify,
//! don't guess" rule, but it is still not the same as this crate having
//! been compiled and tested here — see "What this crate does NOT do
//! yet" below.
//!
//! ## Scope of this delivery
//!
//! This crate wraps openmls into a `MlsGroupSession` type covering the
//! full lifecycle next.md §28 and architecture.md §13/§28 describe:
//! identity/key-package generation, group creation, adding/removing
//! members (commit + welcome), joining from a welcome, encrypting and
//! processing application messages, and merging commits — see
//! `group.rs`'s doc comment for the anchor-by-anchor mapping to
//! `book_code.rs`.
//!
//! ## What this crate does NOT do yet
//!
//! - **Wired into `siar-messaging::GroupService`, but only partially.**
//!   `GroupService` now has a second, MLS-backed path
//!   (`create_group_mls`/`add_member_mls`/`remove_member_mls`/
//!   `send_text_mls`/`handle_incoming_mls`/`join_group_mls`) alongside
//!   its original per-device static-key path — see that module's own
//!   top doc comment for the full split and what's still missing on
//!   its side (key-package distribution, migration between the two
//!   paths). `decode_key_package` exists specifically for that
//!   caller's `add_member_mls`, which needs to turn wire bytes it
//!   didn't generate into a `KeyPackage` `MlsGroupSession::add_member`
//!   can consume.
//! - **Real disk persistence now exists, but isn't the default and
//!   isn't wired into `GroupService`.** `persistent.rs`'s
//!   `SqlitePersistentProvider` is a second `OpenMlsProvider`
//!   implementation, backed by real SQLite via
//!   `openmls_sqlite_storage` (`0.3.0-rc.1`, same workspace this
//!   crate's other pins were checked against) instead of
//!   `OpenMlsRustCrypto`'s bundled in-memory
//!   `openmls_memory_storage::MemoryStorage` — confirmed directly from
//!   `openmls_rust_crypto/src/lib.rs`, not assumed. `MlsGroupSession`
//!   is now generic over the provider (`MlsGroupSession<P:
//!   OpenMlsProvider = OpenMlsRustCrypto>`) specifically so this didn't
//!   require touching anything already using the unparameterized
//!   default. See `persistent.rs`'s own doc comment for why it's a
//!   genuinely separate SQLite database from `siar-storage` (which
//!   turned out to run on `stoolap`, not SQLite — an earlier version of
//!   this very doc comment assumed otherwise and was wrong, corrected
//!   once actually checked) and for what's still not decided
//!   (where the file lives, how `GroupService` would pick between the
//!   default and persistent providers per conversation).
//! - **No test coverage against a real compiler.** Every other crate in
//!   this workspace has unit tests that (as far as this sandbox can
//!   verify without `cargo`) exercise real logic. This crate's
//!   correctness rests on the book_code.rs cross-check above, not on
//!   tests that have actually run — flagged explicitly rather than
//!   presented with the same confidence as, say, `siar-transport-
//!   bluetooth-classic::framing`'s tests, which need no external crate
//!   to be right about anything.

pub mod group;
pub mod identity;
pub mod persistent;

pub use group::{IncomingMlsMessage, MlsGroupError, MlsGroupSession};
pub use identity::{
    decode_key_package, encode_key_package, generate_identity, MlsIdentity, MlsIdentityError,
};
pub use persistent::{JsonCodec, PersistentProviderError, SqlitePersistentProvider};
// Re-exported so callers (e.g. `siar-messaging::group_service`) don't
// need their own direct dependency on `openmls_rust_crypto` just to
// construct a provider to hand to `MlsGroupSession::create`/
// `join_from_welcome`/`generate_identity` — the exact crate providing
// `OpenMlsProvider` is this crate's own pinned choice (see this file's
// top doc comment on why that pin was verified, not guessed), not
// something every caller should have to re-pin independently.
pub use openmls_basic_credential::SignatureKeyPair;
pub use openmls_rust_crypto::OpenMlsRustCrypto;

/// The MTI (mandatory-to-implement) RFC 9420 ciphersuite — verified as
/// `Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` directly
/// from `openmls_traits`' `types.rs`. architecture.md §106's
/// conservative-crypto rule reads the same way here as it did for
/// `siar-crypto`'s 1:1 sessions: pick the reference/most-audited
/// option, not a fashionable one — this is also the one openmls's own
/// README lists first under "Supported ciphersuites" and marks
/// "(MTI)".
pub use openmls_traits::types::Ciphersuite;
pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
