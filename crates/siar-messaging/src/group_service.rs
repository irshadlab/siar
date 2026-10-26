//! `GroupService` (plan.md §27–28, §38–39): group membership and group
//! messaging wired onto the transport/crypto that already exist.
//!
//! ## Two group-crypto paths, side by side
//!
//! This module now has **two** independent ways to send/receive group
//! traffic, and deliberately keeps both rather than ripping the first
//! one out:
//!
//! - The **original per-device static-key path**
//!   (`create_group`/`add_member`/`remove_member`/`send_text`/
//!   `handle_incoming_event`) — plan.md §38's *starting point*, "start
//!   with explicit device fanout because its behavior is easier to
//!   reason about." No forward secrecy; removing a member is
//!   bookkeeping only, not cryptographic lockout (see each method's own
//!   doc comment, unchanged from before).
//! - The **new MLS path** (`create_group_mls`/`add_member_mls`/
//!   `remove_member_mls`/`send_text_mls`/`handle_incoming_mls`) —
//!   `siar-crypto-mls`'s `MlsGroupSession`, the real epoch-keyed group
//!   crypto next.md §28 asks for ("Old members must not decrypt future
//!   epochs"). This is what removing a member actually means
//!   cryptographically, not just administratively.
//!
//! Left as a genuinely open question, not decided here: whether/how an
//! existing static-key-path group ever migrates to the MLS path.
//! Nothing in this module attempts that migration — the two paths use
//! different `EnvelopeKind`s (`GroupEvent`/`Text` vs.
//! `GroupMlsCommit`/`GroupMlsWelcome`/`GroupMlsApplication`, see
//! `siar-protocol::v1::EnvelopeKind`'s doc comments) and a
//! `ConversationId` is never implicitly shared between them by this
//! code. A caller picks one path per conversation.
//!
//! ## What the MLS path still doesn't do (flagged, not hidden)
//!
//! - **Still in-memory here, even though a persistent option now
//!   exists upstream.** `siar-crypto-mls` now has
//!   `SqlitePersistentProvider` (real SQLite-backed `OpenMlsProvider`)
//!   and `MlsGroupSession<P>` is generic enough to use it — but this
//!   module's `mls_sessions` map still holds the default, in-memory
//!   `MlsGroupSession<OpenMlsRustCrypto>`, and nothing here constructs
//!   the persistent variant. A process restart still loses every MLS
//!   group's live crypto state; the durable `GroupState`/
//!   `group_members` bookkeeping (`groups` repository) survives, but
//!   the actual key material and epoch does not. Deciding where the
//!   SQLite file lives and how a restarting `GroupService` would
//!   rediscover which conversations to reopen is application-layer
//!   work `siar-crypto-mls` deliberately left unmade — see that
//!   crate's `persistent.rs` doc comment — and it's still unmade here
//!   too, not solved by this bullet's existence.
//! - **Key-package distribution/discovery, partially closed.**
//!   `add_member_mls` still takes bytes as a direct parameter for
//!   out-of-band/testing use, but `key_package_directory.rs`'s
//!   `KeyPackageDirectory` trait plus `publish_key_package`/
//!   `add_member_mls_from_directory` now give a real (if in-memory-only
//!   — see that module's own doc comment) publish/fetch path. Still not
//!   next.md §41–42's full contact-discovery/QR-pairing system — this
//!   only answers "what's this already-known device's key package,"
//!   not "how do I learn about a device in the first place."

use crate::key_package_directory::KeyPackageDirectory;
use crate::mls_store::{GroupRecord, MlsStore, PendingRecord};
use crate::PeerTicket;
use siar_crypto::{DeviceIdentity, Session};
use siar_crypto_mls::{
    generate_identity, IncomingMlsMessage, MlsGroupSession, SignatureKeyPair,
    SqlitePersistentProvider,
};
use siar_domain::{
    now_millis, AccountId, ConversationId, DeliveryState, DeviceId, DurableGroupEvent, GroupState,
    MessageContent, MessageId, MessageText,
};
use siar_protocol::v1::{Envelope, EnvelopeKind, CURRENT_VERSION};
use siar_protocol::WireMessage;
use siar_storage::{GroupRepository, MessageRepository, StoredMessage};
use siar_transport::{PeerTransport, SiarEndpoint};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GroupServiceError {
    #[error(transparent)]
    Storage(#[from] siar_storage::StorageError),
    #[error(transparent)]
    Transport(#[from] siar_transport::TransportError),
    #[error(transparent)]
    Crypto(#[from] siar_crypto::CryptoError),
    #[error("no local group state for this conversation")]
    UnknownGroup,
    /// Enforced now (was previously flagged as not wired up) in
    /// `add_member`/`remove_member`/`add_member_mls`/
    /// `remove_member_mls` — `GroupState::is_admin` is the check.
    #[error("caller ({caller:?}) is not an admin of this group")]
    NotAnAdmin { caller: AccountId },
    #[error(transparent)]
    Mls(#[from] siar_crypto_mls::MlsGroupError),
    #[error(transparent)]
    MlsIdentity(#[from] siar_crypto_mls::MlsIdentityError),
    #[error("no local MLS session for this conversation — call create_group_mls or join_group_mls first")]
    UnknownMlsSession,
    /// The MLS state directory or one of its SQLite files couldn't be
    /// opened, written or indexed.
    #[error("MLS state storage failed: {0}")]
    MlsStorage(String),
    /// Mirrors `MessageServiceError::Malformed` — a decrypted
    /// `GroupMlsApplication` payload that didn't postcard-decode as
    /// `MessageContent`. Not a `siar_storage::StorageError` (nothing
    /// touched storage yet at the point this is raised), so it gets its
    /// own variant rather than borrowing one that means something else.
    #[error("MLS application payload did not decode as MessageContent")]
    Malformed,
    /// `join_group_mls`'s check — see that method's doc comment and
    /// `pending_identity`'s field doc comment for why this can't just
    /// generate a fresh identity instead.
    #[error("no pending key-package identity — call publish_key_package before join_group_mls")]
    NoPendingKeyPackageIdentity,
    /// `add_member_mls_from_directory`'s check — the configured
    /// `KeyPackageDirectory` had nothing published for the requested
    /// device.
    #[error("no key package available for device {device:?}")]
    NoKeyPackageAvailable { device: DeviceId },
}

/// One recipient device's addressing + key info — exactly what
/// `PeerTicket` already carries for a 1:1 peer, paired with the
/// `DeviceId` group membership is tracked by (plan.md §7's
/// `DeviceId` != transport identity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDevice {
    pub device_id: DeviceId,
    pub ticket: PeerTicket,
}

/// Resolves an account to its known devices' addressing info
/// (plan.md §38's fanout target list). A real implementation backs
/// this with a device registry (`siar_identity_multidevice::
/// directory::DeviceDirectory` — a different, concrete type of the
/// same name, this workspace's real one since `siar_domain`'s former
/// `DeviceRegistry` was retired, see `MIGRATION.md`) plus a
/// contact/ticket store; tests can supply a closure-backed stub.
pub trait DeviceDirectory: Send + Sync {
    fn devices_for(&self, account: AccountId) -> Vec<MemberDevice>;

    /// Remember how to reach `device` of `account`. Called when an invite
    /// or a membership change TELLS this device about another member, so a
    /// member no longer has to be a saved contact to be reachable.
    /// Default: ignore (a directory that is read-only, or backed by
    /// something else, simply keeps its own view).
    fn learn(&self, _account: AccountId, _device: MemberDevice) {}
}

/// Reference `DeviceDirectory` implementation — in-memory, populated by
/// explicit `register` calls rather than any real discovery mechanism.
/// This is the group-membership counterpart to `PeerTicket`'s own
/// "Phase-1 stand-in — copy-paste a printed ticket to add a peer —
/// explicitly not meant to survive past Phase 1" role (see this
/// module's crate-level doc comment): until next.md §41's real contact
/// discovery exists, an application (e.g. `apps/cli`) registers each
/// peer's `(AccountId, MemberDevice)` the same hand-paired way it
/// already handles `PeerTicket`s for 1:1 chat, and this type just holds
/// that mapping so `GroupService`'s fanout logic has something real to
/// query.
#[derive(Default)]
pub struct InMemoryDeviceDirectory {
    devices: Mutex<HashMap<AccountId, Vec<MemberDevice>>>,
    /// When set, addresses LEARNED from invites/commits (not ones an app
    /// registers itself, e.g. from saved contacts, which have their own
    /// storage) are written here and reloaded at start-up.
    persist: Option<Arc<dyn siar_storage::PeerDirectoryRepository + Send + Sync>>,
}

impl InMemoryDeviceDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    /// A directory that remembers what it learns across restarts. Rows it
    /// can't understand (damaged, or from a future version) are skipped
    /// with a warning: a bad entry must not stop the app starting.
    pub fn persistent(repo: Arc<dyn siar_storage::PeerDirectoryRepository + Send + Sync>) -> Self {
        let directory = Self {
            devices: Mutex::default(),
            persist: Some(repo.clone()),
        };
        match repo.list() {
            Ok(rows) => {
                for row in rows {
                    let parsed = (|| {
                        let account = row.account_id.parse::<AccountId>().ok()?;
                        let device_id = row.device_id.parse::<DeviceId>().ok()?;
                        let ticket = PeerTicket::decode(&row.ticket_text).ok()?;
                        Some((account, MemberDevice { device_id, ticket }))
                    })();
                    match parsed {
                        Some((account, device)) => directory.register(account, device),
                        None => tracing::warn!("skipping an unreadable saved member address"),
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "couldn’t read saved member addresses"),
        }
        directory
    }

    /// Adds `device` to `account`'s known devices. Idempotent by
    /// `device_id`: re-registering the same device (e.g. with a
    /// refreshed `PeerTicket` after the peer's endpoint address
    /// changed) replaces the old entry rather than duplicating it —
    /// `fanout_targets` would otherwise send every group event twice to
    /// a device whose ticket got re-registered.
    pub fn register(&self, account: AccountId, device: MemberDevice) {
        let mut devices = self
            .devices
            .lock()
            .expect("InMemoryDeviceDirectory lock poisoned");
        let entry = devices.entry(account).or_default();
        entry.retain(|existing| existing.device_id != device.device_id);
        entry.push(device);
    }
}

impl DeviceDirectory for InMemoryDeviceDirectory {
    fn devices_for(&self, account: AccountId) -> Vec<MemberDevice> {
        self.devices
            .lock()
            .expect("InMemoryDeviceDirectory lock poisoned")
            .get(&account)
            .cloned()
            .unwrap_or_default()
    }

    fn learn(&self, account: AccountId, device: MemberDevice) {
        if let Some(repo) = &self.persist {
            let row = siar_storage::StoredPeerDevice {
                account_id: account.to_string(),
                device_id: device.device_id.to_string(),
                ticket_text: device.ticket.encode(),
            };
            if let Err(e) = repo.upsert(&row) {
                tracing::warn!(error = %e, "couldn’t save a learned member address");
            }
        }
        self.register(account, device);
    }
}

pub struct GroupService {
    device_id: DeviceId,
    /// The `AccountId` `device_id` belongs to — previously absent (see
    /// this field's own reason for existing: `GroupServiceError::NotAnAdmin`
    /// enforcement below needs to know *whose* membership/role to check,
    /// not just which device is sending). An account, not a device,
    /// because `MemberRole`/admin status is tracked per-`AccountId` in
    /// `GroupState` (plan.md §7's account-vs-device split — one admin
    /// account's several devices are all "the admin" for this purpose).
    local_account: AccountId,
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    directory: Arc<dyn DeviceDirectory>,
    groups: Arc<dyn GroupRepository + Send + Sync>,
    /// See this module's top doc comment ("What the MLS path still
    /// doesn't do — No persistence"): `std::sync::Mutex` rather than
    /// `tokio::sync::Mutex` because every `MlsGroupSession` operation
    /// is synchronous CPU work (openmls has no async API) — every
    /// method below that touches this map locks it, does its
    /// synchronous work, extracts whatever bytes it needs to send, and
    /// drops the guard *before* any `.await` (network send), so a
    /// std lock is both correct and cheaper than pulling in a second
    /// mutex flavor for one field.
    mls_sessions: Mutex<HashMap<ConversationId, MlsGroupSession<SqlitePersistentProvider>>>,
    /// Where each group's SQLite file and the rediscovery index live.
    mls_store: MlsStore,
    /// Holds this device's most recently generated-but-not-yet-consumed
    /// `(provider, identity)` pair from `publish_key_package` — see
    /// `MlsGroupSession::join_from_welcome`'s doc comment for exactly
    /// why this has to be kept and reused rather than regenerated:
    /// RFC 9420's `Welcome` is encrypted to that specific key package's
    /// private material. `None` until `publish_key_package` is called;
    /// `join_group_mls` takes it, so it's `None` again immediately
    /// after a successful join (single-use, matching the key package
    /// itself being single-use). Same in-memory-only caveat as
    /// `mls_sessions`.
    pending_identity: Mutex<Option<PendingKeyPackage>>,
}

/// One member device's address, as told to another member.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DirectoryEntry {
    pub account: AccountId,
    pub device: DeviceId,
    /// `PeerTicket::encode` text: how to reach it, plus its keys.
    pub ticket: String,
}

impl DirectoryEntry {
    fn of(account: AccountId, d: &MemberDevice) -> Self {
        Self {
            account,
            device: d.device_id,
            ticket: d.ticket.encode(),
        }
    }

    fn into_device(self) -> Option<(AccountId, MemberDevice)> {
        let ticket = PeerTicket::decode(&self.ticket).ok()?;
        Some((
            self.account,
            MemberDevice {
                device_id: self.device,
                ticket,
            },
        ))
    }
}

/// Most directory entries one invite may carry. A real group is far
/// smaller; the cap only bounds what a hostile invite can make us parse.
const MAX_DIRECTORY_ENTRIES: usize = 256;

/// What a `GroupMlsWelcome` envelope carries: the MLS Welcome, the group's
/// roster/admin record as of the add, and how to reach every other member.
///
/// MLS gives a joiner the cryptographic state, but not the application's
/// own roster, and nothing else delivers it — so before this the admin had
/// to hand-copy that record to the invitee. And without the member
/// addresses the joiner could only message members it had separately
/// saved as contacts. Sending both in the invite makes joining one step.
///
/// The first byte is a format version. Version 1 (roster only) is still
/// understood; a bare Welcome from an older client decodes to `None` and
/// takes the manual path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WelcomeBundle {
    pub welcome: Vec<u8>,
    pub state: GroupState,
    pub directory: Vec<DirectoryEntry>,
}

#[derive(serde::Deserialize)]
struct WelcomeBundleV1 {
    welcome: Vec<u8>,
    state: GroupState,
}

const WELCOME_BUNDLE_V1: u8 = 1;
const WELCOME_BUNDLE_V2: u8 = 2;

impl WelcomeBundle {
    pub fn encode(&self) -> Result<Vec<u8>, GroupServiceError> {
        let mut out = vec![WELCOME_BUNDLE_V2];
        out.extend(postcard::to_allocvec(self).map_err(|_| GroupServiceError::Malformed)?);
        Ok(out)
    }

    /// `None` for anything that isn't a known bundle version (including a
    /// bare Welcome from an older client).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&version, rest) = bytes.split_first()?;
        match version {
            WELCOME_BUNDLE_V2 => postcard::from_bytes(rest).ok(),
            WELCOME_BUNDLE_V1 => {
                let v1: WelcomeBundleV1 = postcard::from_bytes(rest).ok()?;
                Some(Self {
                    welcome: v1.welcome,
                    state: v1.state,
                    directory: Vec::new(),
                })
            }
            _ => None,
        }
    }
}

/// What a `GroupMlsCommit` envelope carries when the commit adds a member:
/// the MLS commit, plus who was added and how to reach them. Existing
/// members otherwise learn neither — the commit only advances their MLS
/// state — so a member added later would be invisible to everyone but the
/// admin who added them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitBundle {
    pub commit: Vec<u8>,
    pub added: DirectoryEntry,
}

const COMMIT_BUNDLE_V1: u8 = 1;

impl CommitBundle {
    pub fn encode(&self) -> Result<Vec<u8>, GroupServiceError> {
        let mut out = vec![COMMIT_BUNDLE_V1];
        out.extend(postcard::to_allocvec(self).map_err(|_| GroupServiceError::Malformed)?);
        Ok(out)
    }

    /// `None` for a bare MLS commit (an older client, or a commit that
    /// isn't an add).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&version, rest) = bytes.split_first()?;
        (version == COMMIT_BUNDLE_V1)
            .then(|| postcard::from_bytes(rest).ok())
            .flatten()
    }
}

/// A published, not-yet-used key package's secrets: the provider whose
/// storage holds its private keys, the signing key, and which file that
/// is (so it can be recorded in the index once a welcome consumes it).
struct PendingKeyPackage {
    provider: SqlitePersistentProvider,
    signature_keys: SignatureKeyPair,
    file: String,
}

fn open_mls_file(
    store: &MlsStore,
    file: &str,
) -> Result<SqlitePersistentProvider, GroupServiceError> {
    SqlitePersistentProvider::open(&store.path_of(file))
        .map_err(|e| GroupServiceError::MlsStorage(e.to_string()))
}

impl GroupService {
    /// `groups` is a `siar_storage::StoolapGroupRepository` in
    /// production — durable, unlike the in-memory placeholder this
    /// replaced. Taking it as a trait object (matching `messages`'s
    /// shape) keeps tests able to swap in a stub without touching
    /// stoolap at all.
    pub fn new(
        device_id: DeviceId,
        local_account: AccountId,
        identity: DeviceIdentity,
        endpoint: Arc<SiarEndpoint>,
        messages: Arc<dyn MessageRepository + Send + Sync>,
        directory: Arc<dyn DeviceDirectory>,
        groups: Arc<dyn GroupRepository + Send + Sync>,
    ) -> Self {
        // Not persistent: a private scratch directory nobody reopens. This
        // keeps the CLI/Android/test behaviour of `new` exactly as it was
        // (MLS state dies with the process); only callers that pass a real
        // directory to `new_persistent` get durable groups.
        let store =
            MlsStore::temporary().expect("couldn’t create a scratch directory for MLS state");
        Self::with_store(
            device_id,
            local_account,
            identity,
            endpoint,
            messages,
            directory,
            groups,
            store,
        )
    }

    /// Like `new`, but MLS group state lives in `mls_dir` and survives a
    /// restart: call [`restore_mls_sessions`](Self::restore_mls_sessions)
    /// right after construction to reopen what an earlier run left there.
    #[allow(clippy::too_many_arguments)]
    pub fn new_persistent(
        device_id: DeviceId,
        local_account: AccountId,
        identity: DeviceIdentity,
        endpoint: Arc<SiarEndpoint>,
        messages: Arc<dyn MessageRepository + Send + Sync>,
        directory: Arc<dyn DeviceDirectory>,
        groups: Arc<dyn GroupRepository + Send + Sync>,
        mls_dir: impl Into<std::path::PathBuf>,
    ) -> Result<Self, GroupServiceError> {
        let store =
            MlsStore::new(mls_dir).map_err(|e| GroupServiceError::MlsStorage(e.to_string()))?;
        Ok(Self::with_store(
            device_id,
            local_account,
            identity,
            endpoint,
            messages,
            directory,
            groups,
            store,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn with_store(
        device_id: DeviceId,
        local_account: AccountId,
        identity: DeviceIdentity,
        endpoint: Arc<SiarEndpoint>,
        messages: Arc<dyn MessageRepository + Send + Sync>,
        directory: Arc<dyn DeviceDirectory>,
        groups: Arc<dyn GroupRepository + Send + Sync>,
        mls_store: MlsStore,
    ) -> Self {
        Self {
            device_id,
            local_account,
            identity,
            endpoint,
            messages,
            directory,
            groups,
            mls_sessions: Mutex::new(HashMap::new()),
            mls_store,
            pending_identity: Mutex::new(None),
        }
    }

    /// Reopens every MLS group an earlier run persisted, and returns the
    /// conversations that came back. A group that fails to reload (missing
    /// or corrupt file) is logged and skipped — it must not stop the
    /// others, or the app, from starting.
    pub fn restore_mls_sessions(&self) -> Vec<ConversationId> {
        let index = self.mls_store.load();
        let mut restored = Vec::new();
        for record in index.groups {
            let loaded = open_mls_file(&self.mls_store, &record.file).and_then(|provider| {
                MlsGroupSession::load(provider, &record.group_id, &record.signature_public_key)
                    .map_err(GroupServiceError::from)
            });
            match loaded {
                Ok(Some(session)) => {
                    self.mls_sessions
                        .lock()
                        .expect("mls_sessions lock poisoned")
                        .insert(record.conversation, session);
                    restored.push(record.conversation);
                }
                Ok(None) => {
                    tracing::warn!(conversation = ?record.conversation, "MLS file holds no such group")
                }
                Err(e) => {
                    tracing::warn!(conversation = ?record.conversation, error = %e, "couldn’t reopen an MLS group")
                }
            }
        }
        restored
    }

    /// Makes sure this device has a published key package, reusing the one
    /// from before a restart if there is one: a key package already given
    /// to someone is worthless if its private half died with the process.
    /// Falls back to generating a fresh one.
    pub fn ensure_key_package_published(
        &self,
        directory: &dyn KeyPackageDirectory,
    ) -> Result<(), GroupServiceError> {
        if let Some(pending) = self.mls_store.load().pending {
            let reopened = open_mls_file(&self.mls_store, &pending.file).and_then(|provider| {
                let keys =
                    MlsGroupSession::load_signature_keys(&provider, &pending.signature_public_key)?;
                Ok((provider, keys))
            });
            match reopened {
                Ok((provider, signature_keys)) => {
                    *self
                        .pending_identity
                        .lock()
                        .expect("pending_identity lock poisoned") = Some(PendingKeyPackage {
                        provider,
                        signature_keys,
                        file: pending.file,
                    });
                    directory.publish(self.device_id, pending.key_package);
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "couldn’t reuse the saved key package; making a new one")
                }
            }
        }
        self.publish_key_package(directory)
    }

    pub fn group_state(
        &self,
        conversation: ConversationId,
    ) -> Result<Option<GroupState>, GroupServiceError> {
        Ok(self.groups.get(conversation)?)
    }

    /// Creates a new group locally with the caller as founder/admin
    /// (plan.md §27's `GroupState::new`). Nothing is sent yet — members
    /// are added via `add_member`, each of which fans out its own
    /// durable event.
    pub fn create_group(
        &self,
        conversation: ConversationId,
        founder: AccountId,
    ) -> Result<GroupState, GroupServiceError> {
        let state = GroupState::new(conversation, founder);
        self.groups.upsert(&state)?;
        Ok(state)
    }

    /// plan.md §40/§27: admits `new_member`, advances the epoch, and
    /// fans the durable event out to every device of every *current*
    /// member (including the new one, so they learn their own
    /// admission) — mirroring plan.md §39's "the sender's other devices
    /// need this too" rule.
    ///
    /// Admin-only (`GroupState::is_admin`) — the enforcement this
    /// module's error type flagged as missing until now.
    pub async fn add_member(
        &self,
        conversation: ConversationId,
        new_member: AccountId,
    ) -> Result<(), GroupServiceError> {
        let mut state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;
        if !state.is_admin(self.local_account) {
            return Err(GroupServiceError::NotAnAdmin {
                caller: self.local_account,
            });
        }
        let next_epoch = state.epoch.next();
        let event = DurableGroupEvent::MemberAdded {
            account: new_member,
            epoch: next_epoch,
        };

        state.apply(&event);
        state.apply(&DurableGroupEvent::EpochAdvanced {
            new_epoch: next_epoch,
        });
        self.groups.upsert(&state)?;

        self.fanout_event(&state, &event).await
    }

    /// plan.md §40: revokes membership locally and tells every
    /// remaining device — see this module's top doc comment for why
    /// that's bookkeeping, not cryptographic lockout, until real group
    /// key rotation lands.
    ///
    /// Admin-only, same as `add_member`.
    pub async fn remove_member(
        &self,
        conversation: ConversationId,
        member: AccountId,
    ) -> Result<(), GroupServiceError> {
        let mut state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;
        if !state.is_admin(self.local_account) {
            return Err(GroupServiceError::NotAnAdmin {
                caller: self.local_account,
            });
        }
        let next_epoch = state.epoch.next();
        let event = DurableGroupEvent::MemberRemoved {
            account: member,
            epoch: next_epoch,
        };

        state.apply(&event);
        state.apply(&DurableGroupEvent::EpochAdvanced {
            new_epoch: next_epoch,
        });
        self.groups.upsert(&state)?;

        self.fanout_event(&state, &event).await
    }

    /// plan.md §16-17's persist-before-send, applied to group text: one
    /// local `StoredMessage` (group messages live in the same table as
    /// 1:1 ones — `conversation_id` is what makes it a group's history,
    /// not a separate schema), then one independently-encrypted send
    /// per member device.
    pub async fn send_text(
        &self,
        conversation: ConversationId,
        text: MessageText,
    ) -> Result<MessageId, GroupServiceError> {
        let state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;

        let message_id = MessageId::new();
        let now = now_millis();
        let content = MessageContent::Text(text);
        let plaintext = postcard::to_allocvec(&content).expect("MessageContent always serializes");

        let stored = StoredMessage {
            message_id,
            conversation_id: conversation,
            sender_device: self.device_id,
            sequence: now,
            timestamp_millis: now,
            delivery_state: DeliveryState::Local,
            // Local copy stays plaintext-shaped-but-unencrypted-at-rest
            // is *not* what this stores — each recipient gets its own
            // ciphertext below; what we persist locally is our own
            // record, encrypted to nobody in particular, matching how
            // `MessageService::send_text` treats the sender's own copy.
            payload: plaintext.clone(),
        };
        self.messages.insert_if_new(&stored)?;

        for device in self.fanout_targets(&state) {
            let session = Session::establish(
                &self.identity,
                &x25519_dalek::PublicKey::from(device.ticket.x25519_public),
            );
            let ciphertext = session.encrypt(&plaintext)?;

            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id,
                conversation_id: conversation,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::Text,
                payload: ciphertext,
            };

            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                // plan.md §33: a single offline member's device doesn't
                // fail the whole group send — this is exactly the "peer
                // offline" branch the retry scheduler exists for. Group
                // sends don't yet feed the outbox per-recipient (that
                // needs a per-recipient outbox row shape the current
                // schema doesn't have), so for now a missed member
                // relies on a future message in the conversation to
                // catch them up, not an automatic retry. Tracked as a
                // follow-up, not silently swallowed.
                tracing::warn!(error = %e, device = ?device.device_id, "group send failed for one member device");
            }
        }

        Ok(message_id)
    }

    /// Applies a `DurableGroupEvent` received (already decrypted) from
    /// another member — the receive-side mirror of `add_member`/
    /// `remove_member`'s local `state.apply` calls.
    pub fn handle_incoming_event(
        &self,
        conversation: ConversationId,
        event: DurableGroupEvent,
    ) -> Result<(), GroupServiceError> {
        let Some(mut state) = self.groups.get(conversation)? else {
            // We don't have local state for this conversation yet (e.g.
            // this is our own `MemberAdded` event arriving before we've
            // ever called `create_group` locally). Phase-appropriate
            // behavior: drop it rather than guess at reconstructing a
            // `GroupState` from a single event — a real implementation
            // would fetch full state from a member instead (plan.md
            // §82's incremental sync), which isn't wired up yet.
            tracing::warn!(
                ?conversation,
                "group event for unknown local group; dropping"
            );
            return Ok(());
        };
        state.apply(&event);
        self.groups.upsert(&state)?;
        Ok(())
    }

    fn fanout_targets(&self, state: &GroupState) -> Vec<MemberDevice> {
        state
            .members()
            .iter()
            .flat_map(|m| self.directory.devices_for(m.account))
            .filter(|d| d.device_id != self.device_id)
            .collect()
    }

    /// This device as another member would address it.
    fn own_member_device(&self) -> MemberDevice {
        MemberDevice {
            device_id: self.device_id,
            ticket: PeerTicket {
                endpoint_addr: self.endpoint.addr(),
                x25519_public: self.identity.x25519_public().to_bytes(),
                ed25519_verifying: self.identity.verifying_key().to_bytes(),
            },
        }
    }

    /// Adopts addresses an invite or commit told us about — but only for
    /// accounts that really are members, never for ourselves, and never
    /// more than a sane number, so a hostile message can't plant arbitrary
    /// entries or make us parse an unbounded list.
    fn learn_directory(&self, state: &GroupState, entries: Vec<DirectoryEntry>) {
        for entry in entries.into_iter().take(MAX_DIRECTORY_ENTRIES) {
            if entry.account == self.local_account || !state.is_member(entry.account) {
                continue;
            }
            if let Some((account, device)) = entry.into_device() {
                self.directory.learn(account, device);
            }
        }
    }

    /// An existing member's side of "an admin added someone": record the
    /// roster change and learn how to reach the new member. Without this
    /// only the admin's roster ever changed, so everyone else kept sending
    /// to the old member list.
    fn apply_member_added(
        &self,
        conversation: ConversationId,
        sender: DeviceId,
        added: DirectoryEntry,
    ) {
        let Ok(Some(mut state)) = self.groups.get(conversation) else {
            tracing::warn!(
                ?conversation,
                "commit for a group with no stored roster; roster not updated"
            );
            return;
        };
        // Only an admin may add members: find the sender's account in OUR
        // view of the group and check it.
        let sender_account = state.members().iter().map(|m| m.account).find(|a| {
            self.directory
                .devices_for(*a)
                .iter()
                .any(|d| d.device_id == sender)
        });
        let Some(sender_account) = sender_account.filter(|a| state.is_admin(*a)) else {
            tracing::warn!(
                ?conversation,
                "member-add commit from a device that isn't a known admin; roster not updated"
            );
            return;
        };
        let _ = sender_account;
        let account = added.account;
        if !state.is_member(account) {
            let next = state.epoch.next();
            state.apply(&DurableGroupEvent::MemberAdded {
                account,
                epoch: next,
            });
            state.apply(&DurableGroupEvent::EpochAdvanced { new_epoch: next });
            if let Err(e) = self.groups.upsert(&state) {
                tracing::warn!(error = %e, "couldn’t save the updated roster");
                return;
            }
        }
        if account != self.local_account {
            if let Some((account, device)) = added.into_device() {
                self.directory.learn(account, device);
            }
        }
    }

    async fn fanout_event(
        &self,
        state: &GroupState,
        event: &DurableGroupEvent,
    ) -> Result<(), GroupServiceError> {
        let plaintext = postcard::to_allocvec(event).expect("DurableGroupEvent always serializes");
        let now = now_millis();

        for device in self.fanout_targets(state) {
            let session = Session::establish(
                &self.identity,
                &x25519_dalek::PublicKey::from(device.ticket.x25519_public),
            );
            let ciphertext = session.encrypt(&plaintext)?;

            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id: MessageId::new(),
                conversation_id: state.conversation_id,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::GroupEvent,
                payload: ciphertext,
            };

            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                tracing::warn!(error = %e, device = ?device.device_id, "group event fanout failed for one member device");
            }
        }

        Ok(())
    }

    // ---- MLS path (see this module's top doc comment) ----

    /// Creates a brand-new MLS group with the caller as its sole
    /// member — mirrors `create_group`'s "nothing is sent yet" shape.
    /// Also creates the local `GroupState` bookkeeping row (same as
    /// `create_group`) so `group_state`/`fanout_targets` keep working
    /// identically for both paths.
    pub fn create_group_mls(
        &self,
        conversation: ConversationId,
        founder: AccountId,
    ) -> Result<GroupState, GroupServiceError> {
        let file = format!("{conversation}.sqlite3");
        let provider = open_mls_file(&self.mls_store, &file)?;
        let identity = generate_identity(self.device_id, &provider)?;
        let session = MlsGroupSession::create(provider, &identity)?;
        self.mls_store
            .record_group(GroupRecord {
                conversation,
                file,
                group_id: session.group_id_bytes(),
                signature_public_key: session.signature_public_key(),
            })
            .map_err(|e| GroupServiceError::MlsStorage(e.to_string()))?;

        self.mls_sessions
            .lock()
            .expect("mls_sessions lock poisoned")
            .insert(conversation, session);

        let state = GroupState::new(conversation, founder);
        self.groups.upsert(&state)?;
        Ok(state)
    }

    /// Admits `new_member_device` using their `new_member_key_package_bytes`
    /// (caller-supplied — see this module's top doc comment on why this
    /// crate can't fetch that itself yet), advances the group's local
    /// bookkeeping epoch to match, and fans the commit out to every
    /// *other* current member's devices (`GroupMlsCommit`) plus the
    /// welcome to the new member alone (`GroupMlsWelcome`).
    ///
    /// Note this fans out over the **existing** `fanout_targets` (the
    /// members list *before* this admission), consistent with
    /// `add_member`'s own "who already knows about this group" logic —
    /// the new member doesn't need the commit, they have the welcome,
    /// which already reflects post-admission state.
    ///
    /// Admin-only, same check and error as `add_member`.
    pub async fn add_member_mls(
        &self,
        conversation: ConversationId,
        new_member: AccountId,
        new_member_device: DeviceId,
        new_member_key_package_bytes: &[u8],
    ) -> Result<(), GroupServiceError> {
        let key_package = siar_crypto_mls::decode_key_package(new_member_key_package_bytes)?;

        let mut state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;
        if !state.is_admin(self.local_account) {
            return Err(GroupServiceError::NotAnAdmin {
                caller: self.local_account,
            });
        }
        let fanout_before = self.fanout_targets(&state);

        let (commit_bytes, welcome_bytes) = {
            let mut sessions = self
                .mls_sessions
                .lock()
                .expect("mls_sessions lock poisoned");
            let session = sessions
                .get_mut(&conversation)
                .ok_or(GroupServiceError::UnknownMlsSession)?;
            session.add_member(&key_package)?
        };

        let next_epoch = state.epoch.next();
        state.apply(&DurableGroupEvent::MemberAdded {
            account: new_member,
            epoch: next_epoch,
        });
        state.apply(&DurableGroupEvent::EpochAdvanced {
            new_epoch: next_epoch,
        });
        self.groups.upsert(&state)?;

        // The added member's address, as known to this admin (it is needed
        // anyway to deliver the welcome).
        let added_entry = self
            .directory
            .devices_for(new_member)
            .into_iter()
            .find(|d| d.device_id == new_member_device)
            .map(|d| DirectoryEntry::of(new_member, &d));
        // Existing members learn who was added and how to reach them.
        let commit_payload = match &added_entry {
            Some(added) => CommitBundle {
                commit: commit_bytes.clone(),
                added: added.clone(),
            }
            .encode()?,
            None => commit_bytes.clone(), // can't describe them: a bare commit, as before
        };
        // The new member learns how to reach everyone else, including us.
        let mut directory_entries: Vec<DirectoryEntry> = Vec::new();
        let own = self.own_member_device();
        let mut push = |account: AccountId, d: &MemberDevice| {
            let entry = DirectoryEntry::of(account, d);
            if !directory_entries
                .iter()
                .any(|e| e.account == entry.account && e.device == entry.device)
            {
                directory_entries.push(entry);
            }
        };
        push(self.local_account, &own);
        for m in state.members() {
            if m.account == new_member {
                continue;
            }
            for d in self.directory.devices_for(m.account) {
                if d.device_id != new_member_device {
                    push(m.account, &d);
                }
            }
        }
        // The Welcome travels with the roster/admin record it belongs to.
        let welcome_payload = WelcomeBundle {
            welcome: welcome_bytes,
            state: state.clone(),
            directory: directory_entries,
        }
        .encode()?;

        let now = now_millis();
        for device in fanout_before {
            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id: MessageId::new(),
                conversation_id: conversation,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::GroupMlsCommit,
                payload: commit_payload.clone(),
            };
            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                tracing::warn!(error = %e, device = ?device.device_id, "MLS commit fanout failed for one member device");
            }
        }

        for device in self.directory.devices_for(new_member) {
            if device.device_id != new_member_device {
                continue;
            }
            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id: MessageId::new(),
                conversation_id: conversation,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::GroupMlsWelcome,
                payload: welcome_payload.clone(),
            };
            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                tracing::warn!(error = %e, device = ?device.device_id, "MLS welcome send failed");
            }
        }

        Ok(())
    }

    /// Accepts an invite whose Welcome payload is a [`WelcomeBundle`]:
    /// joins the MLS group and returns the group record that came with it.
    ///
    /// The record is checked before anything is trusted: it must be for
    /// THIS conversation and must list this account as a member. A mismatch
    /// is refused rather than adopted, since the roster otherwise comes
    /// from whoever sent the invite.
    pub fn join_group_from_invite(
        &self,
        conversation: ConversationId,
        payload: &[u8],
    ) -> Result<GroupState, GroupServiceError> {
        let bundle = WelcomeBundle::decode(payload).ok_or(GroupServiceError::Malformed)?;
        if bundle.state.conversation_id != conversation
            || !bundle.state.is_member(self.local_account)
        {
            return Err(GroupServiceError::Malformed);
        }
        self.join_group_mls(conversation, &bundle.welcome, bundle.state.clone())?;
        self.learn_directory(&bundle.state, bundle.directory);
        Ok(bundle.state)
    }

    /// The MLS-path counterpart to `remove_member` — but unlike that
    /// method, this one is next.md §28's actual cryptographic lockout:
    /// `MlsGroupSession::remove_member` advances the group's real MLS
    /// epoch, so `member`'s device can no longer derive future epochs'
    /// key material even if it keeps running a modified client, which
    /// is exactly the gap the static-key path's own doc comment flags.
    ///
    /// Admin-only, same check and error as `remove_member`.
    pub async fn remove_member_mls(
        &self,
        conversation: ConversationId,
        member: AccountId,
        member_device: DeviceId,
    ) -> Result<(), GroupServiceError> {
        let mut state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;
        if !state.is_admin(self.local_account) {
            return Err(GroupServiceError::NotAnAdmin {
                caller: self.local_account,
            });
        }
        // fanout_targets is computed *after* the state removal below so
        // the departing member's own devices don't receive a commit
        // they're cryptographically no longer able to use anyway —
        // matches `remove_member`'s existing "tell every remaining
        // device" framing.
        let commit_bytes = {
            let mut sessions = self
                .mls_sessions
                .lock()
                .expect("mls_sessions lock poisoned");
            let session = sessions
                .get_mut(&conversation)
                .ok_or(GroupServiceError::UnknownMlsSession)?;
            session.remove_member(member_device)?
        };

        let next_epoch = state.epoch.next();
        state.apply(&DurableGroupEvent::MemberRemoved {
            account: member,
            epoch: next_epoch,
        });
        state.apply(&DurableGroupEvent::EpochAdvanced {
            new_epoch: next_epoch,
        });
        self.groups.upsert(&state)?;

        let now = now_millis();
        for device in self.fanout_targets(&state) {
            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id: MessageId::new(),
                conversation_id: conversation,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::GroupMlsCommit,
                payload: commit_bytes.clone(),
            };
            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                tracing::warn!(error = %e, device = ?device.device_id, "MLS commit fanout failed for one member device");
            }
        }

        Ok(())
    }

    /// The MLS-path counterpart to `send_text` — same persist-then-send
    /// shape, but a single `MlsGroupSession::encrypt` call replaces
    /// `send_text`'s per-recipient `Session::encrypt` loop (MLS
    /// ciphertext is the same bytes for every recipient — that's the
    /// point of a shared group key — so there is exactly one
    /// `EnvelopeKind::GroupMlsApplication` envelope built here, fanned
    /// out unchanged rather than re-encrypted per device).
    pub async fn send_text_mls(
        &self,
        conversation: ConversationId,
        text: MessageText,
    ) -> Result<MessageId, GroupServiceError> {
        let state = self
            .groups
            .get(conversation)?
            .ok_or(GroupServiceError::UnknownGroup)?;

        let message_id = MessageId::new();
        let now = now_millis();
        let content = MessageContent::Text(text);
        let plaintext = postcard::to_allocvec(&content).expect("MessageContent always serializes");

        let ciphertext = {
            let mut sessions = self
                .mls_sessions
                .lock()
                .expect("mls_sessions lock poisoned");
            let session = sessions
                .get_mut(&conversation)
                .ok_or(GroupServiceError::UnknownMlsSession)?;
            session.encrypt(&plaintext)?
        };

        let stored = StoredMessage {
            message_id,
            conversation_id: conversation,
            sender_device: self.device_id,
            sequence: now,
            timestamp_millis: now,
            delivery_state: DeliveryState::Local,
            payload: plaintext,
        };
        self.messages.insert_if_new(&stored)?;

        for device in self.fanout_targets(&state) {
            let envelope = Envelope {
                version: CURRENT_VERSION,
                message_id,
                conversation_id: conversation,
                sender: self.device_id,
                timestamp_millis: now,
                sequence: now,
                kind: EnvelopeKind::GroupMlsApplication,
                payload: ciphertext.clone(),
            };
            if let Err(e) = self
                .endpoint
                .send(
                    device.ticket.endpoint_addr.clone(),
                    &WireMessage::V1(envelope),
                )
                .await
            {
                tracing::warn!(error = %e, device = ?device.device_id, "MLS application message send failed for one member device");
            }
        }

        Ok(message_id)
    }

    /// Joins an MLS group from a `GroupMlsWelcome` envelope's payload —
    /// call this when `handle_incoming_mls` reports it couldn't process
    /// a welcome because there's no local `GroupState` for it yet (see
    /// that method's doc comment).
    ///
    /// `initial_state` is the group's current membership/roles/epoch
    /// bookkeeping, supplied by the caller rather than derived from the
    /// MLS session this method joins. This was a real gap found while
    /// actually trying to use this method end-to-end (not caught at
    /// the time it first shipped): `GroupState` and an
    /// `MlsGroupSession`'s real cryptographic membership are two
    /// separate pieces of bookkeeping this codebase keeps in parallel
    /// (see `add_member_mls`/`remove_member_mls`, which update both
    /// explicitly), and there is no code anywhere that derives one from
    /// the other — an MLS credential's identity bytes map back to a
    /// `DeviceId` (see `identity.rs`'s `generate_identity`), not
    /// directly to the `AccountId` `GroupState` is keyed by, so
    /// reconstructing membership from `MlsGroupSession::member_count`/
    /// internals here would need a device-to-account lookup this
    /// method doesn't have access to. Rather than guess at that
    /// mapping, the caller — whoever told this device about the
    /// group in the first place — is responsible for also telling it
    /// who's in it. `add_member_mls`'s doc comment on the founder's
    /// side doesn't currently transmit this alongside the welcome
    /// either; wiring that hand-off is real follow-up work, not solved
    /// by adding this parameter.
    ///
    /// Uses `pending_identity` — the `(provider, identity)` pair
    /// `publish_key_package` generated and stashed — rather than
    /// generating a fresh identity here. This was a real bug in an
    /// earlier version of this method: a freshly generated identity's
    /// signing key doesn't match whatever key package the adder
    /// actually consumed, so it would have looked plausible and failed
    /// (or worse, silently produced a group session that couldn't
    /// decrypt anything) — see `MlsGroupSession::join_from_welcome`'s
    /// doc comment for exactly why. Returns
    /// `GroupServiceError::NoPendingKeyPackageIdentity` if
    /// `publish_key_package` was never called (or its result was
    /// already consumed by an earlier `join_group_mls` call) — you
    /// can't join with a welcome for a key package you never actually
    /// published.
    pub fn join_group_mls(
        &self,
        conversation: ConversationId,
        welcome_bytes: &[u8],
        initial_state: GroupState,
    ) -> Result<(), GroupServiceError> {
        let pending = self
            .pending_identity
            .lock()
            .expect("pending_identity lock poisoned")
            .take()
            .ok_or(GroupServiceError::NoPendingKeyPackageIdentity)?;
        let file = pending.file;
        let session = MlsGroupSession::join_from_welcome(
            pending.provider,
            pending.signature_keys,
            welcome_bytes,
        )?;
        // The key package is spent; the group now lives in its file.
        self.mls_store
            .record_group(GroupRecord {
                conversation,
                file,
                group_id: session.group_id_bytes(),
                signature_public_key: session.signature_public_key(),
            })
            .and_then(|()| self.mls_store.clear_pending())
            .map_err(|e| GroupServiceError::MlsStorage(e.to_string()))?;

        self.mls_sessions
            .lock()
            .expect("mls_sessions lock poisoned")
            .insert(conversation, session);
        // Without this, a joined member could never call
        // `send_text_mls`/`add_member_mls`/`remove_member_mls`
        // afterward — all three require `self.groups.get(conversation)`
        // to already have a row (`GroupServiceError::UnknownGroup`
        // otherwise), which only `create_group_mls` populated before
        // this fix.
        self.groups.upsert(&initial_state)?;
        Ok(())
    }

    /// Generates a fresh MLS identity for this device (not tied to any
    /// particular conversation — a key package is what lets *any*
    /// group admin add this device, before it's a member of anything),
    /// publishes its serialized key package via `directory`, and stashes
    /// the identity in `pending_identity` for `join_group_mls` to
    /// consume once someone actually uses it. See `pending_identity`'s
    /// field doc comment for why that stash matters.
    ///
    /// Only one pending identity is held at a time — a second call
    /// before the first is consumed silently replaces it (the old
    /// identity is dropped; if a `Welcome` for the *old* key package
    /// arrives after that, `join_group_mls` will fail with
    /// `NoPendingKeyPackageIdentity`'s cousin — well, actually
    /// `MlsGroupError` from `join_from_welcome` mismatching, since
    /// `pending_identity` would hold the *new* pair by then). Real
    /// deployments publishing more than one key package at a time need
    /// a keyed pool here instead — flagged as a real, not yet needed,
    /// limitation of this pass's scope rather than solved speculatively.
    pub fn publish_key_package(
        &self,
        directory: &dyn KeyPackageDirectory,
    ) -> Result<(), GroupServiceError> {
        let file = format!("kp-{}.sqlite3", ConversationId::new());
        let provider = open_mls_file(&self.mls_store, &file)?;
        let identity = generate_identity(self.device_id, &provider)?;
        let key_package_bytes =
            siar_crypto_mls::encode_key_package(identity.key_package.key_package())?;
        self.mls_store
            .record_pending(PendingRecord {
                file: file.clone(),
                signature_public_key: identity.signature_keys.to_public_vec(),
                key_package: key_package_bytes.clone(),
            })
            .map_err(|e| GroupServiceError::MlsStorage(e.to_string()))?;

        *self
            .pending_identity
            .lock()
            .expect("pending_identity lock poisoned") = Some(PendingKeyPackage {
            provider,
            signature_keys: identity.signature_keys,
            file,
        });
        directory.publish(self.device_id, key_package_bytes);
        Ok(())
    }

    /// Convenience wrapper around `add_member_mls` that looks
    /// `new_member_device`'s key package up in `directory` instead of
    /// requiring the caller to have obtained the bytes itself — the
    /// directory-backed half of the gap this module's top doc comment
    /// flags under "No key-package distribution/discovery." Still not
    /// next.md §41's full contact-discovery system — see
    /// `key_package_directory.rs`'s own doc comment on scope.
    pub async fn add_member_mls_from_directory(
        &self,
        conversation: ConversationId,
        new_member: AccountId,
        new_member_device: DeviceId,
        directory: &dyn KeyPackageDirectory,
    ) -> Result<(), GroupServiceError> {
        let key_package_bytes =
            directory
                .take(new_member_device)
                .ok_or(GroupServiceError::NoKeyPackageAvailable {
                    device: new_member_device,
                })?;
        self.add_member_mls(
            conversation,
            new_member,
            new_member_device,
            &key_package_bytes,
        )
        .await
    }

    /// Receive-side dispatch for all three `GroupMls*` envelope kinds —
    /// the MLS-path mirror of `handle_incoming_event`. Returns the
    /// decoded `MessageContent` for a `GroupMlsApplication` frame (the
    /// caller persists/displays it, matching how `MessageService::
    /// handle_incoming` hands `Text` content back rather than storing
    /// it itself), and `None` for a commit merge or an already-handled
    /// welcome.
    ///
    /// A `GroupMlsWelcome` arriving with no local MLS session yet is
    /// **not** auto-joined here — that would mean this function decides
    /// unilaterally that being invited means joining, with no chance
    /// for the caller's UI to ask the user first. It's handed back as
    /// `Ok(None)` with a warning logged, same "drop rather than guess"
    /// stance `handle_incoming_event` already takes for an unknown
    /// group; `join_group_mls` is the caller's explicit accept action.
    pub fn handle_incoming_mls(
        &self,
        conversation: ConversationId,
        envelope: &Envelope,
    ) -> Result<Option<MessageContent>, GroupServiceError> {
        match envelope.kind {
            EnvelopeKind::GroupMlsWelcome => {
                let sessions = self
                    .mls_sessions
                    .lock()
                    .expect("mls_sessions lock poisoned");
                if sessions.contains_key(&conversation) {
                    tracing::warn!(
                        ?conversation,
                        "MLS welcome for a conversation we already have a session for; ignoring"
                    );
                    return Ok(None);
                }
                drop(sessions);
                tracing::warn!(?conversation, "MLS welcome received but no local session exists yet — call join_group_mls explicitly to accept");
                Ok(None)
            }
            EnvelopeKind::GroupMlsCommit | EnvelopeKind::GroupMlsApplication => {
                // An add-member commit may arrive wrapped with the new
                // member's address; a bare commit (older client) still works.
                let bundle = matches!(envelope.kind, EnvelopeKind::GroupMlsCommit)
                    .then(|| CommitBundle::decode(&envelope.payload))
                    .flatten();
                let mls_payload: &[u8] = bundle
                    .as_ref()
                    .map(|b| b.commit.as_slice())
                    .unwrap_or(&envelope.payload);
                let outcome = {
                    let mut sessions = self
                        .mls_sessions
                        .lock()
                        .expect("mls_sessions lock poisoned");
                    let Some(session) = sessions.get_mut(&conversation) else {
                        tracing::warn!(
                            ?conversation,
                            "MLS frame for unknown local session; dropping"
                        );
                        return Ok(None);
                    };
                    session.process_incoming(mls_payload)?
                };
                match outcome {
                    IncomingMlsMessage::CommitMerged => {
                        if let Some(b) = bundle {
                            self.apply_member_added(conversation, envelope.sender, b.added);
                        }
                        Ok(None)
                    }
                    // openmls 0.9.0-rc.2's own-message-echo case (see
                    // `IncomingMlsMessage::Ignored`'s doc comment in
                    // `siar-crypto-mls`): nothing for a caller to do.
                    IncomingMlsMessage::Ignored => Ok(None),
                    IncomingMlsMessage::Application(plaintext) => {
                        let content: MessageContent = postcard::from_bytes(&plaintext)
                            .map_err(|_| GroupServiceError::Malformed)?;
                        Ok(Some(content))
                    }
                }
            }
            _ => Ok(None),
        }
    }
}

// Moved here from right after `InMemoryDeviceDirectory`'s own `impl`
// block — clippy's `items_after_test_module` flags a `#[cfg(test)]`
// module followed by more non-test items in the same file as a real
// readability smell (a reader scanning top-to-bottom hits "tests" and
// reasonably assumes the file's real logic is done, then keeps finding
// more of it below). `GroupService`'s own struct/impl/tests all follow
// this module now, same "tests at the end" shape as every other file
// in this workspace.
#[cfg(test)]
mod device_directory_tests {
    use super::*;
    use iroh::{EndpointAddr, EndpointId};

    // Same construction `ticket.rs`'s own tests use — a syntactically
    // valid `EndpointId`, distinguished per test case by `seed`; we only
    // care that different `MemberDevice`s compare as different, not
    // that any of these are routable.
    fn member(seed: u8) -> MemberDevice {
        let identity = siar_crypto::DeviceIdentity::generate();
        let id = EndpointId::from_bytes(&[seed; 32]).unwrap();
        MemberDevice {
            device_id: DeviceId::new(),
            ticket: PeerTicket {
                endpoint_addr: EndpointAddr::new(id),
                x25519_public: identity.x25519_public().to_bytes(),
                ed25519_verifying: identity.verifying_key().to_bytes(),
            },
        }
    }

    #[test]
    fn devices_for_unknown_account_is_empty() {
        let dir = InMemoryDeviceDirectory::new();
        assert!(dir.devices_for(AccountId::new()).is_empty());
    }

    #[test]
    fn register_then_devices_for_finds_it() {
        let dir = InMemoryDeviceDirectory::new();
        let account = AccountId::new();
        let device = member(1);
        let device_id = device.device_id;

        dir.register(account, device);

        let found = dir.devices_for(account);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].device_id, device_id);
    }

    #[test]
    fn re_registering_the_same_device_replaces_rather_than_duplicates() {
        let dir = InMemoryDeviceDirectory::new();
        let account = AccountId::new();
        let mut device = member(1);
        dir.register(account, device.clone());

        // Same device_id, different endpoint (as if the peer's address
        // changed) — a fresh EndpointId built from a different seed.
        let new_id = EndpointId::from_bytes(&[9u8; 32]).unwrap();
        device.ticket.endpoint_addr = EndpointAddr::new(new_id);
        dir.register(account, device.clone());

        let found = dir.devices_for(account);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ticket.endpoint_addr.id, new_id);
    }

    #[test]
    fn devices_for_different_accounts_dont_cross_over() {
        let dir = InMemoryDeviceDirectory::new();
        let alice = AccountId::new();
        let bob = AccountId::new();
        dir.register(alice, member(1));

        assert!(dir.devices_for(bob).is_empty());
        assert_eq!(dir.devices_for(alice).len(), 1);
    }
}

#[cfg(test)]
mod persistent_directory_tests {
    use super::*;
    use siar_storage::{PeerDirectoryRepository, StoolapPeerDirectoryRepository};

    fn device(seed: u8) -> MemberDevice {
        let identity = siar_crypto::DeviceIdentity::generate();
        MemberDevice {
            device_id: DeviceId::new(),
            ticket: PeerTicket {
                endpoint_addr: iroh::EndpointAddr::new(
                    iroh::SecretKey::from_bytes(&[seed; 32]).public(),
                ),
                x25519_public: identity.x25519_public().to_bytes(),
                ed25519_verifying: identity.verifying_key().to_bytes(),
            },
        }
    }

    #[test]
    fn learned_addresses_survive_a_restart_but_registered_ones_are_not_duplicated_there() {
        let repo: Arc<dyn PeerDirectoryRepository + Send + Sync> = Arc::new(
            StoolapPeerDirectoryRepository::new(siar_storage::open_in_memory().unwrap()),
        );
        let (learned_account, registered_account) = (AccountId::new(), AccountId::new());
        let (learned, registered) = (device(1), device(2));

        let first = InMemoryDeviceDirectory::persistent(repo.clone());
        first.learn(learned_account, learned.clone());
        first.register(registered_account, registered); // a saved contact: its own storage's job
        assert_eq!(first.devices_for(learned_account), vec![learned.clone()]);

        let second = InMemoryDeviceDirectory::persistent(repo.clone()); // a "restart"
        assert_eq!(second.devices_for(learned_account), vec![learned]);
        assert!(second.devices_for(registered_account).is_empty());
        assert_eq!(repo.list().unwrap().len(), 1);
    }

    #[test]
    fn learning_the_same_device_again_replaces_it_everywhere() {
        let repo: Arc<dyn PeerDirectoryRepository + Send + Sync> = Arc::new(
            StoolapPeerDirectoryRepository::new(siar_storage::open_in_memory().unwrap()),
        );
        let account = AccountId::new();
        let mut d = device(3);
        let dir = InMemoryDeviceDirectory::persistent(repo.clone());
        dir.learn(account, d.clone());
        d.ticket.endpoint_addr =
            iroh::EndpointAddr::new(iroh::SecretKey::from_bytes(&[4; 32]).public());
        dir.learn(account, d.clone());
        assert_eq!(dir.devices_for(account), vec![d.clone()]);
        assert_eq!(
            InMemoryDeviceDirectory::persistent(repo).devices_for(account),
            vec![d]
        );
    }

    #[test]
    fn unreadable_rows_are_skipped_not_fatal() {
        let repo: Arc<dyn PeerDirectoryRepository + Send + Sync> = Arc::new(
            StoolapPeerDirectoryRepository::new(siar_storage::open_in_memory().unwrap()),
        );
        let good_account = AccountId::new();
        let good = device(5);
        repo.upsert(&siar_storage::StoredPeerDevice {
            account_id: "not-a-uuid".into(),
            device_id: "x".into(),
            ticket_text: "garbage".into(),
        })
        .unwrap();
        repo.upsert(&siar_storage::StoredPeerDevice {
            account_id: good_account.to_string(),
            device_id: good.device_id.to_string(),
            ticket_text: good.ticket.encode(),
        })
        .unwrap();
        let dir = InMemoryDeviceDirectory::persistent(repo);
        assert_eq!(dir.devices_for(good_account), vec![good]);
    }
}
