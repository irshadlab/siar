//! `MlsGroupSession`: the group lifecycle next.md §28 asks for, wrapped
//! around openmls. Every non-trivial call below has a directly
//! corresponding line in openmls's own `openmls/tests/book_code.rs`
//! (anchors named in each method's doc comment) — see this crate's
//! `lib.rs` doc comment for why that file specifically, and for what
//! this wrapper does not yet do (persistence, wiring into
//! `GroupService`).
//!
//! Two things this wrapper imposes that plain openmls doesn't, both
//! deliberate:
//!
//! 1. **Bytes in, bytes out.** Every openmls example in `book_code.rs`
//!    passes `MlsMessageOut`/`MlsMessageIn` around in-memory (it's test
//!    code — sender and receiver share a process). This wrapper always
//!    serializes to `Vec<u8>` at the boundary (`.to_bytes()` /
//!    `tls_deserialize_exact()`, both directly observed in
//!    `book_code.rs`'s `mls_message_in_from_bytes` anchor), because
//!    this workspace's sender and receiver are never the same process
//!    — the whole point is these bytes travel over `siar-transport`.
//! 2. **The adder merges its own commit immediately.** `book_code.rs`'s
//!    `alice_adds_bob` anchor is followed immediately (still inside the
//!    same anchor's surrounding code, not a separate anchor) by
//!    `alice_group.merge_pending_commit(alice_provider)` — i.e. openmls
//!    expects the caller to merge its own pending commit right after
//!    creating it, not wait for a round-trip. [`MlsGroupSession::add_member`]
//!    and [`MlsGroupSession::remove_member`] both do this internally so
//!    callers can't forget it.

use crate::CIPHERSUITE;
use openmls::prelude::*;
// `openmls::prelude::*` re-exports `tls_codec::*` via a nested glob
// (`pub use tls_codec::{self, *};` in openmls's own prelude.rs, verified
// against openmls 0.9.0-rc.1's actual source), but that nested glob's
// `Serialize`/`Deserialize` traits don't reliably reach this scope through
// a second-level `use ...::*` — confirmed by the real compiler error this
// crate hit (`tls_deserialize_exact` "not found in current scope" despite
// the glob import above). Importing the trait explicitly, exactly as
// rustc's own suggestion did, is what actually brings `tls_deserialize`/
// `tls_deserialize_exact` into scope for `MlsMessageIn` below.
use openmls::prelude::tls_codec::Deserialize as _;
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use siar_domain::DeviceId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MlsGroupError {
    #[error("failed to create MLS group: {0}")]
    Create(String),
    #[error("failed to add member: {0}")]
    AddMember(String),
    #[error("failed to remove member: {0}")]
    RemoveMember(String),
    #[error("failed to join group from welcome: {0}")]
    Join(String),
    #[error("failed to create application message: {0}")]
    Encrypt(String),
    #[error("failed to process incoming message: {0}")]
    Process(String),
    #[error("failed to merge commit: {0}")]
    Merge(String),
    #[error("failed to serialize MLS message: {0}")]
    Serialize(String),
    #[error("failed to deserialize MLS message: {0}")]
    Deserialize(String),
    #[error("no member in this group matches the requested device")]
    UnknownMember,
    #[error("failed to reload a persisted group: {0}")]
    Load(String),
}

/// The classification of a processed incoming MLS message — the
/// receive-side mirror of the fact that `add_member`/`remove_member`
/// produce a commit (and, for adds, a welcome). Deliberately not a
/// 1:1 wrap of openmls's own `ProcessedMessageContent` — that enum
/// also has `ProposalMessage` and `ExternalJoinProposalMessage`
/// variants this crate's current scope (next.md §28's direct
/// add/remove, not standalone propose-then-commit) has no caller for
/// yet; see `process_incoming`'s doc comment.
pub enum IncomingMlsMessage {
    /// A decrypted application (chat) message — book_code.rs's
    /// `inspect_application_message` anchor.
    Application(Vec<u8>),
    /// A commit from another member (an add, a remove, or an update)
    /// was received, validated, and merged into this group's state.
    /// The caller finds out *what* changed (if it needs to update
    /// `siar_domain::GroupState` bookkeeping to match) by comparing
    /// `members()` before and after, or by inspecting the specific
    /// proposal types — not exposed further here; see book_code.rs's
    /// `inspect_staged_commit`/`remove_operation` anchors for what's
    /// available on the `StagedCommit` this method already consumed.
    CommitMerged,
    /// This client's own commit or application message was fanned back
    /// by the delivery service and carries no new information — real
    /// cases openmls 0.9.0-rc.2's `ProcessedMessageContent` added
    /// (`OwnPendingCommit`/`OwnPrivateMessage`, confirmed against that
    /// enum's own doc comments in openmls's source, not guessed) that
    /// an earlier version of this match against an older openmls
    /// didn't need to cover. `OwnPendingCommit` (our own commit,
    /// already locally pending, fanned back matching) still causes a
    /// real state change — see `process_incoming`'s own handling, which
    /// calls `merge_pending_commit` and returns `CommitMerged` for that
    /// case rather than this variant. This variant is for
    /// `OwnPrivateMessage` specifically: an own-authored ciphertext
    /// commit or application message the delivery service echoed back,
    /// which this client's own sender ratchet can't decrypt by design
    /// (openmls's own doc comment: "Applications should treat this
    /// variant as a hint to skip the message") — there is genuinely
    /// nothing to merge or apply.
    Ignored,
}

/// One MLS group's live state for the local device — one instance per
/// `(ConversationId, DeviceId)` pair, matching
/// `siar_domain::group`'s device-level (not account-level) membership
/// model. Holds the `provider` alongside the group because every
/// openmls call needs both together (book_code.rs never calls a method
/// on `MlsGroup` without also passing the provider it was created
/// with).
///
/// Generic over `P: OpenMlsProvider`, defaulting to `OpenMlsRustCrypto`
/// (this crate's original in-memory-only provider) so every existing
/// caller's plain `MlsGroupSession` — no type argument written anywhere
/// in `siar-messaging::group_service` — keeps meaning exactly what it
/// already meant, unchanged. `MlsGroupSession<SqlitePersistentProvider>`
/// (see `persistent.rs`) is the same type with real disk persistence
/// instead — see that module's doc comment for what "real" means here
/// and doesn't yet.
pub struct MlsGroupSession<P: OpenMlsProvider = OpenMlsRustCrypto> {
    group: MlsGroup,
    provider: P,
    signature_keys: SignatureKeyPair,
}

impl<P: OpenMlsProvider> MlsGroupSession<P> {
    /// Creates a brand-new group with `founder_identity` as its sole
    /// member — book_code.rs anchors `mls_group_create_config_example`
    /// (using the simpler no-extensions default here — see
    /// `identity.rs`'s note on why this pass has no real extension
    /// content to put there) and `alice_create_group`.
    pub fn create(
        provider: P,
        founder_identity: &crate::MlsIdentity,
    ) -> Result<Self, MlsGroupError> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            // Without this a Welcome does not carry the ratchet tree, and
            // `join_from_welcome` (which has no out-of-band tree to pass)
            // fails with `MissingRatchetTree`: no member could ever have
            // joined a group. Found by the persistence tests, the first to
            // exercise a real create → add → join round trip.
            .use_ratchet_tree_extension(true)
            .build();
        let group = MlsGroup::new(
            &provider,
            &founder_identity.signature_keys,
            &config,
            founder_identity.credential_with_key.clone(),
        )
        .map_err(|e| MlsGroupError::Create(format!("{e:?}")))?;

        Ok(Self {
            group,
            provider,
            signature_keys: founder_identity.signature_keys.clone(),
        })
    }

    /// Joins a group from a welcome message another member's
    /// `add_member` produced — book_code.rs anchor
    /// `bob_joins_with_welcome`, adapted to take serialized bytes
    /// (`welcome_bytes`, as sent over `siar-transport`) rather than an
    /// in-memory `MlsMessageOut`, deserialized the same way
    /// `process_incoming` deserializes application/commit messages
    /// (`mls_message_in_from_bytes` anchor) before narrowing to
    /// specifically a welcome via `into_welcome()`.
    ///
    /// **`provider` and `signature_keys` must be the exact same pair
    /// `generate_identity` produced when the key package this welcome
    /// consumes was created and published** — not a freshly generated
    /// identity, even for the same device. RFC 9420's `Welcome` message
    /// is encrypted to that specific key package's private HPKE init
    /// key, which only exists inside the provider it was generated
    /// against; there's no way to derive it after the fact from a new
    /// identity. `siar_messaging::group_service::GroupService`'s
    /// `pending_identity` field exists specifically to hold that pair
    /// alive between `publish_key_package` and whichever later call
    /// consumes the resulting welcome — see that module's doc comment.
    /// (An earlier version of this crate's `GroupService` integration
    /// generated a fresh identity right here instead, which would have
    /// silently failed to join — caught and fixed before it shipped
    /// anywhere near working, not left as a known bug.)
    pub fn join_from_welcome(
        provider: P,
        signature_keys: SignatureKeyPair,
        welcome_bytes: &[u8],
    ) -> Result<Self, MlsGroupError> {
        let config = MlsGroupJoinConfig::builder().build();

        let message_in = MlsMessageIn::tls_deserialize_exact(welcome_bytes)
            .map_err(|e| MlsGroupError::Deserialize(format!("{e:?}")))?;
        // `MlsMessageIn::into_welcome()` doesn't exist in openmls
        // 0.9.0-rc.2 (confirmed against its real source —
        // `message_in.rs`'s only public extraction method is
        // `extract(self) -> MlsMessageBodyIn`, a real compiler error
        // this file hit, not a guessed rename). `extract()` then
        // matching on `MlsMessageBodyIn::Welcome` is the actual
        // replacement.
        let welcome = match message_in.extract() {
            MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => return Err(MlsGroupError::Join("message is not a welcome".to_string())),
        };

        let staged_join = StagedWelcome::new_from_welcome(&provider, &config, welcome, None)
            .map_err(|e| MlsGroupError::Join(format!("{e:?}")))?;
        let group = staged_join
            .into_group(&provider)
            .map_err(|e| MlsGroupError::Join(format!("{e:?}")))?;

        Ok(Self {
            group,
            provider,
            signature_keys,
        })
    }

    /// Reopens a group that an earlier run persisted through `provider`'s
    /// storage (only meaningful for a persistent provider, e.g.
    /// `SqlitePersistentProvider`; an in-memory provider is empty on
    /// every start, so this returns `None` there).
    ///
    /// `group_id` and `signature_public_key` are what `group_id_bytes`
    /// and `signature_public_key` returned while the group was live —
    /// the caller persists those two public, non-secret values next to
    /// the database file (see `GroupService`), because openmls can look a
    /// group up by id but cannot enumerate them.
    ///
    /// `Ok(None)`: the database holds no such group. `Err`: it holds the
    /// group but its signing key is gone or unreadable — a real
    /// corruption, deliberately not treated the same as "absent".
    pub fn load(
        provider: P,
        group_id: &[u8],
        signature_public_key: &[u8],
    ) -> Result<Option<Self>, MlsGroupError> {
        let id = GroupId::from_slice(group_id);
        let Some(group) = MlsGroup::load(provider.storage(), &id)
            .map_err(|e| MlsGroupError::Load(format!("{e:?}")))?
        else {
            return Ok(None);
        };
        let signature_keys = SignatureKeyPair::read(
            provider.storage(),
            signature_public_key,
            CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| MlsGroupError::Load("the group's signing key is missing".to_string()))?;
        Ok(Some(Self {
            group,
            provider,
            signature_keys,
        }))
    }

    /// Reads back a signing key pair previously stored by
    /// `generate_identity` in `provider`'s storage — for a key package
    /// that was published but not yet used when the process stopped.
    pub fn load_signature_keys(
        provider: &P,
        signature_public_key: &[u8],
    ) -> Result<SignatureKeyPair, MlsGroupError> {
        SignatureKeyPair::read(
            provider.storage(),
            signature_public_key,
            CIPHERSUITE.signature_algorithm(),
        )
        .ok_or_else(|| MlsGroupError::Load("the signing key is missing".to_string()))
    }

    /// The MLS group id (public). Persist it to be able to `load` later.
    pub fn group_id_bytes(&self) -> Vec<u8> {
        self.group.group_id().as_slice().to_vec()
    }

    /// This member's signing PUBLIC key (never the private half). Persist
    /// it alongside `group_id_bytes` to be able to `load` later.
    pub fn signature_public_key(&self) -> Vec<u8> {
        self.signature_keys.to_public_vec()
    }

    /// Adds one new member (via their published `KeyPackage`) and
    /// immediately merges the resulting commit locally — see this
    /// module's top doc comment on why the merge happens here rather
    /// than being left to the caller. book_code.rs anchor
    /// `alice_adds_bob`, plus the immediately-following (same test,
    /// same surrounding block, not a separately named anchor)
    /// `merge_pending_commit` call.
    ///
    /// Returns `(commit_bytes, welcome_bytes)`: `commit_bytes` goes to
    /// every *existing* member (so they can `process_incoming` it and
    /// merge the same commit), `welcome_bytes` goes to the *new*
    /// member only (so they can `join_from_welcome`). Distributing
    /// each to the right recipients is `GroupService`'s job once this
    /// crate is wired in — see `lib.rs`'s "What this crate does NOT do
    /// yet".
    pub fn add_member(
        &mut self,
        new_member_key_package: &KeyPackage,
    ) -> Result<(Vec<u8>, Vec<u8>), MlsGroupError> {
        let (commit_out, welcome_out, _group_info) = self
            .group
            .add_members(
                &self.provider,
                &self.signature_keys,
                core::slice::from_ref(new_member_key_package),
            )
            .map_err(|e| MlsGroupError::AddMember(format!("{e:?}")))?;

        self.group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsGroupError::Merge(format!("{e:?}")))?;

        let commit_bytes = commit_out
            .to_bytes()
            .map_err(|e| MlsGroupError::Serialize(format!("{e:?}")))?;
        let welcome_bytes = welcome_out
            .to_bytes()
            .map_err(|e| MlsGroupError::Serialize(format!("{e:?}")))?;
        Ok((commit_bytes, welcome_bytes))
    }

    /// Removes `device` (matched against the group's members by
    /// comparing their credential's identity bytes to `device`'s UUID
    /// bytes — see `identity.rs`'s `generate_identity` for why that
    /// comparison is valid) and merges the resulting commit locally.
    /// book_code.rs anchor `charlie_removes_bob`, same
    /// immediately-following `merge_pending_commit` pattern as
    /// `add_member`.
    ///
    /// Per next.md §28 ("Old members must not decrypt future epochs"),
    /// this is the actual cryptographic lockout `siar_messaging::
    /// GroupService`'s current bookkeeping-only `remove_member`
    /// explicitly does not provide yet (see that module's own doc
    /// comment) — once wired in, this is what closes that gap.
    ///
    /// Returns `commit_bytes` for every remaining member to
    /// `process_incoming`. Unlike `add_member`, there is no welcome to
    /// distribute — a pure removal's welcome is always `None`
    /// (book_code.rs: `assert!(welcome_option.is_none())` right after
    /// `charlie_removes_bob`).
    pub fn remove_member(&mut self, device: DeviceId) -> Result<Vec<u8>, MlsGroupError> {
        let leaf_index = self
            .find_member_leaf_index(device)
            .ok_or(MlsGroupError::UnknownMember)?;

        let (commit_out, _welcome_option, _group_info) = self
            .group
            .remove_members(&self.provider, &self.signature_keys, &[leaf_index])
            .map_err(|e| MlsGroupError::RemoveMember(format!("{e:?}")))?;

        self.group
            .merge_pending_commit(&self.provider)
            .map_err(|e| MlsGroupError::Merge(format!("{e:?}")))?;

        commit_out
            .to_bytes()
            .map_err(|e| MlsGroupError::Serialize(format!("{e:?}")))
    }

    /// Encrypts an application (chat) message to the current epoch's
    /// group key. book_code.rs anchor `create_application_message`,
    /// plus its immediately-following `to_bytes()` serialization
    /// (shown separately from the anchor, but part of the same test
    /// flow, right before `mls_message_in_from_bytes`).
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, MlsGroupError> {
        let message_out = self
            .group
            .create_message(&self.provider, &self.signature_keys, plaintext)
            .map_err(|e| MlsGroupError::Encrypt(format!("{e:?}")))?;
        message_out
            .to_bytes()
            .map_err(|e| MlsGroupError::Serialize(format!("{e:?}")))
    }

    /// Deserializes and processes one incoming MLS wire message —
    /// covers both `process_message` anchor (application messages) and
    /// the `StagedCommitMessage` handling from `self_update`/
    /// `commit_to_proposals`/`charlie_removes_bob`'s anchors (commits).
    ///
    /// A `ProposalMessage` (bare proposal, not yet committed) is
    /// treated as an error here rather than a third `IncomingMlsMessage`
    /// variant — this crate's current scope only produces commits
    /// directly via `add_member`/`remove_member`
    /// (`group.add_members`/`group.remove_members`, not
    /// `propose_add_members` + a separate `commit_to_pending_proposals`
    /// step), so a bare proposal arriving here would mean a peer is
    /// speaking a part of the MLS protocol this wrapper doesn't
    /// generate — worth surfacing loudly as `Process`, not silently
    /// swallowing it as if it were expected.
    pub fn process_incoming(&mut self, bytes: &[u8]) -> Result<IncomingMlsMessage, MlsGroupError> {
        let message_in = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|e| MlsGroupError::Deserialize(format!("{e:?}")))?;
        let protocol_message: ProtocolMessage =
            message_in.try_into_protocol_message().map_err(|_| {
                MlsGroupError::Process(
                    "message is not a PublicMessage or PrivateMessage".to_string(),
                )
            })?;

        let processed = self
            .group
            .process_message(&self.provider, protocol_message)
            .map_err(|e| MlsGroupError::Process(format!("{e:?}")))?;

        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(application_message) => {
                Ok(IncomingMlsMessage::Application(application_message.into_bytes()))
            }
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                self.group
                    .merge_staged_commit(&self.provider, *staged_commit)
                    .map_err(|e| MlsGroupError::Merge(format!("{e:?}")))?;
                Ok(IncomingMlsMessage::CommitMerged)
            }
            ProcessedMessageContent::ProposalMessage(_) | ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                Err(MlsGroupError::Process(
                    "received a bare proposal — this wrapper's current scope only produces/expects direct commits, see this method's doc comment".to_string(),
                ))
            }
            ProcessedMessageContent::OwnPendingCommit => {
                // The delivery service fanned our own commit back to
                // us, and it matches what we already have staged
                // locally (openmls's own doc comment on this variant:
                // "To apply it, merge the pending commit using
                // `MlsGroup::merge_pending_commit()`" — a different
                // method than `StagedCommitMessage`'s `merge_staged_
                // commit`, since there's no `StagedCommit` payload on
                // this variant to merge). Same externally-visible
                // result as the other commit path, so it's reported the
                // same way.
                self.group
                    .merge_pending_commit(&self.provider)
                    .map_err(|e| MlsGroupError::Merge(format!("{e:?}")))?;
                Ok(IncomingMlsMessage::CommitMerged)
            }
            ProcessedMessageContent::OwnPrivateMessage => {
                // Genuinely nothing to do here — see
                // `IncomingMlsMessage::Ignored`'s own doc comment for
                // why openmls's own guidance is to skip this variant
                // outright, not an oversight in this match.
                Ok(IncomingMlsMessage::Ignored)
            }
        }
    }

    /// This device's key package for *another* device's `add_member`
    /// call to publish/consume. Not the same key package
    /// `MlsIdentity::key_package` was created with once it's been
    /// consumed by an add — MLS key packages are single-use
    /// (book_code.rs doesn't show regeneration explicitly, but RFC
    /// 9420 itself specifies this; a device that's about to be added to
    /// a second group needs a fresh `generate_identity` call for a new
    /// key package, not reuse of this one). Exposed here only as a
    /// pass-through for callers that generated the identity elsewhere
    /// and want the group session to hand it onward.
    pub fn own_epoch_authenticator(&self) -> Vec<u8> {
        // book_code.rs's own equality check across members
        // (`alice_group.epoch_authenticator().as_slice() ==
        // bob_group.epoch_authenticator().as_slice()`) is exactly the
        // property next.md §28's epoch model needs a cheap way to spot-
        // check: two sessions agree on this iff they're at the same
        // epoch with the same key material.
        self.group.epoch_authenticator().as_slice().to_vec()
    }

    pub fn member_count(&self) -> usize {
        self.group.members().count()
    }

    fn find_member_leaf_index(&self, device: DeviceId) -> Option<LeafNodeIndex> {
        let target = device.as_uuid().as_bytes().to_vec();
        self.group
            .members()
            .find(|member| member.credential.serialized_content() == target.as_slice())
            .map(|member| member.index)
    }
}
