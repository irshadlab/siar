//! siar-crypto: identity keys + application-level E2EE (plan.md §12–13).
//!
//! Phase-1 scope only. This gives every message a real AEAD-encrypted
//! payload today, but the session layer here is intentionally minimal:
//! a static X25519 ECDH shared secret, not a ratchet. That means Phase 1
//! has confidentiality/integrity but **not** forward secrecy or
//! break-in recovery yet.
//!
//! plan.md §13 is explicit that a proven ratchet should be used rather
//! than inventing one — so the Phase-2 reliability pass should replace
//! `Session` with a real Double Ratchet implementation rather than
//! extending this one. `vodozemac` (Matrix's pure-Rust, audited-lineage
//! Olm/Megolm + Double Ratchet implementation) is the strongest
//! zero-C-dependency fit for that and is the recommended swap-in.
//!
//! `mailbox_token` (this pass) reuses this same identity/ECDH layer
//! for a different purpose: deriving rotating, unlinkable mailbox
//! capability tokens per next.md §32 — see that module's own doc
//! comment for the derivation and, importantly, what it still doesn't
//! solve.
//!
//! This crate is now also the home for several sections of Part 28
//! (Production Security / E2EE / Key Management / Privacy Architecture)
//! that don't depend on the still-unbuilt ratchet: `keystore` (§9-10,
//! opaque key handles + memory hygiene), `epoch` (§22, security epoch),
//! `envelope`/`replay` (§14-17, secure message envelope + associated
//! data + replay protection + deterministic nonce derivation),
//! `fanout` (§18, multi-device delivery), `history_policy` (§19's
//! missing piece — the enrollment flow itself lives in
//! `siar-identity-multidevice`), `revocation` (§20-21, a standalone
//! signed revocation record + epoch-based propagation mitigation, built
//! on top of — not replacing — `siar-identity-multidevice`'s directory-
//! generation revocation model), and `clone_detection`/`restore_safety`
//! (§23-24, device-instance clone detection and forcing restores onto a
//! safe path), and `platform_keystore` (§8, the policy-only
//! `KeyStorageBackend` abstraction `keystore.rs::SecureKeyStore::backend`
//! reports — no actual OS/hardware call is implemented anywhere in this
//! crate; real Android Keystore/Keychain/DPAPI/TPM adapters are a
//! separate, platform-specific effort), `domains` (§44, the canonical
//! `CryptoDomain` registry — reserved for new constructions; existing
//! ad hoc domain strings elsewhere in this crate are documented, not
//! renamed), `pseudonym` (§34-36, scoped per-context pseudonyms +
//! rotating proximity discovery IDs), and `trust` (§40-41, `PeerTrustState`
//! and `TrustSource`). See each module's own doc comment for how it
//! relates to — and, where relevant, deliberately doesn't modify
//! `session.rs` or `siar-identity-multidevice`.
//!
//! Also reconciliation-only this round (no new module — see this
//! crate's own git history/session notes, not restated per-line here):
//! §28-29 (File Encryption / File Content Key) are already
//! `siar-blob-manifest::encryption`'s `encrypt_blob`/`BlobEncryptionKey`
//! — a fresh, never-reused per-blob key, matching §29's own requirement,
//! in a crate that's independent of this one by design (see that
//! crate's own `lib.rs`), so no duplicate file-key type belongs here.
//! §30 (Chunk Encryption) is the same crate's whole-blob-AEAD-then-
//! content-hashed-chunks design — a different mechanism than §30's own
//! literal "each chunk independently authenticated," but one that
//! already delivers every functional property §30 lists (resume,
//! reordering, partial download, parallel paths) and was a deliberate
//! choice documented in that crate's own `encryption.rs`/`descriptor.rs`,
//! not an oversight to correct. §31 (Metadata Encryption) *was* a real
//! gap — closed this round as `siar-blob-manifest::metadata_encryption`,
//! not here, since it operates on that crate's own `FileMetadata` type.
//! §32-33 (DTN Security / Relay Security) hold by construction: every
//! DTN/relay path in this workspace only ever carries the ciphertext
//! this crate (or `siar-blob-manifest`) already produced, plus the
//! specific routing metadata `siar-dtn-bundle::DtnBundle` exposes
//! (bundle ID, expiry, priority, size via `PayloadReference` — no
//! broader plaintext field exists on that struct to accidentally leak).
//!
//! §5-7 (Identity Hierarchy / Account Identity / Device Identity) are
//! reconciliation notes rather than new modules — the types already
//! exist, just under different names than Part 28's own Rust sketches
//! use: §6's `AccountId([u8; 32])` ("a long-lived logical account
//! root... used rarely, mainly for authorizing/revoking devices,
//! account recovery, high-value trust changes... not for every
//! message") is conceptually `siar_identity_multidevice::RootIdentityKey`/
//! `RootPublicKey`, not `siar_domain::AccountId` (a `Uuid` used
//! throughout this workspace as the account's routing/addressing
//! identifier — a distinct, already-established concept that should
//! keep its existing name; renaming or duplicating it to match §6's
//! literal type name would recreate exactly the "one key for unrelated
//! purposes" confusion §5 warns against). §7's `DeviceCertificate` DTO
//! (account/device/device_public_key/issued_at/expires_at/signature)
//! matches `siar_identity_multidevice::DeviceCertificate` field-for-
//! field (which additionally carries `capabilities` and `generation`).
//! §5's own list — keep Account/Device/Session/Conversation/Group-
//! Epoch/File-Content/Transport/Backup keys distinct — already holds
//! across this workspace's existing types (`RootIdentityKey`,
//! `DeviceIdentity`, `Session`, `AttachmentKey` are all separate,
//! non-interchangeable types); Group Epoch Keys, Transport Keys, and
//! Backup Keys don't exist as concrete types yet (§25-27 and §112+
//! respectively — not built).

mod attachment;
mod clone_detection;
mod domains;
mod envelope;
mod epoch;
mod error;
mod fanout;
mod history_policy;
mod identity;
mod keystore;
mod mailbox_token;
mod platform_keystore;
mod pseudonym;
mod replay;
mod restore_safety;
mod revocation;
mod session;
mod trust;

pub use attachment::{
    decrypt_attachment, encrypt_attachment, AttachmentKey, BlobHash, EncryptedBlob,
};
pub use clone_detection::{CloneDetector, CloneVerdict, DeviceInstanceId};
pub use domains::CryptoDomain;
pub use envelope::{
    decrypt_envelope, encrypt_envelope, AuthenticationTag, MessageType, SecureMessageEnvelope,
    ENVELOPE_PROTOCOL_VERSION,
};
pub use epoch::SecurityEpoch;
pub use error::CryptoError;
pub use fanout::{fan_out_envelope, RecipientDevice};
pub use history_policy::HistoryAccessPolicy;
pub use identity::DeviceIdentity;
pub use keystore::{InMemorySecureKeyStore, KeyHandle, KeyPolicy, SecureKeyStore};
pub use mailbox_token::{epoch_for, MailboxToken, MailboxTokenSecret, EPOCH_LENGTH_MILLIS};
pub use platform_keystore::KeyStorageBackend;
pub use pseudonym::{
    derive_rotating_discovery_id, derive_scoped_pseudonym, RotatingDiscoveryId, ScopedPseudonym,
};
pub use replay::{ReplayError, ReplayGuard};
pub use restore_safety::{decide_restore, RestoreDecision};
pub use revocation::{is_epoch_stale_after_revocation, DeviceRevocation, RevocationReason};
pub use session::Session;
pub use trust::{PeerTrustState, TrustSource};
