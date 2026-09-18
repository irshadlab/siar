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
use crate::PeerTicket;
use siar_crypto::{DeviceIdentity, Session};
use siar_crypto_mls::{generate_identity, IncomingMlsMessage, MlsGroupSession, OpenMlsRustCrypto};
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
#[derive(Debug, Clone)]
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
}

impl InMemoryDeviceDirectory {
    pub fn new() -> Self {
        Self::default()
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
    mls_sessions: Mutex<HashMap<ConversationId, MlsGroupSession>>,
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
    pending_identity: Mutex<
        Option<(
            siar_crypto_mls::OpenMlsRustCrypto,
            siar_crypto_mls::MlsIdentity,
        )>,
    >,
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
        Self {
            device_id,
            local_account,
            identity,
            endpoint,
            messages,
            directory,
            groups,
            mls_sessions: Mutex::new(HashMap::new()),
            pending_identity: Mutex::new(None),
        }
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
        let provider = OpenMlsRustCrypto::default();
        let identity = generate_identity(self.device_id, &provider)?;
        let session = MlsGroupSession::create(provider, &identity)?;

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
                payload: welcome_bytes.clone(),
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
        let (provider, identity) = self
            .pending_identity
            .lock()
            .expect("pending_identity lock poisoned")
            .take()
            .ok_or(GroupServiceError::NoPendingKeyPackageIdentity)?;
        let session =
            MlsGroupSession::join_from_welcome(provider, identity.signature_keys, welcome_bytes)?;

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
        let provider = OpenMlsRustCrypto::default();
        let identity = generate_identity(self.device_id, &provider)?;
        let key_package_bytes =
            siar_crypto_mls::encode_key_package(identity.key_package.key_package())?;

        *self
            .pending_identity
            .lock()
            .expect("pending_identity lock poisoned") = Some((provider, identity));
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
                match session.process_incoming(&envelope.payload)? {
                    IncomingMlsMessage::CommitMerged => Ok(None),
                    // openmls 0.9.0-rc.2's own-message-echo case (see
                    // `IncomingMlsMessage::Ignored`'s doc comment in
                    // `siar-crypto-mls` for the full explanation) —
                    // genuinely nothing for a caller here to do with it
                    // either, same `Ok(None)` shape as `CommitMerged`
                    // and the "no envelope content produced" branches
                    // above.
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
