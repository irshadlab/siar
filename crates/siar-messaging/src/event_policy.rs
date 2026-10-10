//! The messaging domain's side of `siar-event-log`'s Phase 5-7 seams.
//!
//! `siar-event-log` is deliberately domain-free (its §1), so everything
//! it offers for outbox planning, replication scope, retention,
//! unknown-event handling, size limits, upcasting, restore safety and
//! deletion is a *mechanism* waiting for a domain to supply *policy*.
//! This module supplies messaging's: one small, reviewable function per
//! seam, each a pure value or pure function so the policy can be read
//! and tested without a store, a network, or a [`crate::MessageService`].
//!
//! | seam (`siar-event-log`) | messaging policy here |
//! |---|---|
//! | [`EffectPlanner`] (§30) | [`MessagingEffectPlanner`]: `MessageQueued` obliges a send; `MessageDelivered` discharges it |
//! | [`ScopeTable`] (§49) | [`messaging_scope_table`] |
//! | [`RetentionTable`] (§40) | [`messaging_retention_table`] |
//! | [`UnknownEventPolicy`] (§62) | [`messaging_unknown_policy`] |
//! | [`EventLimits`] (§55) | [`messaging_event_limits`] |
//! | [`UpcasterRegistry`] (§9) | [`messaging_upcasters`] |
//! | [`RestoreGuard`] (§71) | [`MessagingRestoreGuard`] |
//! | deletion (§41) | [`delete_message_content`] |
//! | namespaces (§64/65) | [`identity_conversation_stream_id`] |
//!
//! ## What is *not* wired, and why
//!
//! [`crate::MessageService`] still sends through `siar-storage`'s
//! `OutboxRepository`, and that is a decision, not an omission. That
//! repository is the system of record for message delivery
//! (`MessageService::with_event_log`'s own doc comment says the event log
//! is an audit/sync stream whose failures are logged and swallowed), and
//! it already commits message + outbox row together in `siar-storage`'s
//! own database. Swapping in the event log's outbox would make a second
//! durable queue authoritative for the same sends — two sources of truth
//! — and the event log's atomic append lives on traits
//! (`TransactionalOutbox`) that use native async and so cannot be held as
//! the `Arc<dyn EventStore>` the service stores today. The right
//! migration is a deliberate cut-over of the system of record, not a
//! side-by-side. Until then these policies make the event log
//! *capable* of everything the spec asks — rebuilding sends after a
//! restore, vetting them, compacting, deleting, replicating — and
//! `tests/event_log_wiring.rs` proves each against real messaging
//! events.
//!
//! ### A stream-layout finding that blocks own-device replication
//!
//! `siar-event-log`'s replication (§49-53) is cursor-based and therefore
//! requires **single-writer streams**. Messaging today appends every
//! device's events for a conversation to the *one* stream
//! [`crate::conversation_stream_id`]. Two of an account's devices
//! sending into the same conversation are two writers on one stream, and
//! the receiving side would report `Diverged` (by design — §54).
//!
//! The fix is a layout change, not a code fix:
//! [`replication_stream_id`] names a per-writer stream
//! (`conversation:{id}:writer:{device}`). Replication then moves each
//! device's *own* stream to its peers, and a projection reads the union.
//! That changes where events are written, so it is flagged here rather
//! than done silently to a stream with existing data.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use siar_domain::{ConversationId, DeviceId, MessageId};
use siar_event_log::backup::{GuardVerdict, RestoreGuard};
use siar_event_log::command_flow::{EffectIntent, EffectPlanner, EventRef};
use siar_event_log::deletion::{execute_deletion, DeletionError, DeletionReport, DeletionRequest};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::limits::EventLimits;
use siar_event_log::namespace::{namespaced_stream_id, NamespaceId};
use siar_event_log::outbox::{EffectKind, OperationId, Outbox, OutboxEntry};
use siar_event_log::redaction::RedactableStore;
use siar_event_log::replication::{ReplicationScope, ScopeTable};
use siar_event_log::retention::{RetentionClass, RetentionTable};
use siar_event_log::snapshot::SnapshotStore;
use siar_event_log::unknown::UnknownEventPolicy;
use siar_event_log::upcast::UpcasterRegistry;
use siar_event_log::EventStore;

use crate::events::{
    conversation_stream_id, decode_messaging_event, MessagingEvent,
    CURRENT_MESSAGING_EVENT_SCHEMA_VERSION, EVENT_TYPE_MESSAGE_CREATED, EVENT_TYPE_MESSAGE_DELETED,
    EVENT_TYPE_MESSAGE_DELIVERED, EVENT_TYPE_MESSAGE_EDITED, EVENT_TYPE_MESSAGE_QUEUED,
    EVENT_TYPE_MESSAGE_READ, EVENT_TYPE_MESSAGE_RECEIVED, EVENT_TYPE_REACTION_ADDED,
    EVENT_TYPE_REACTION_REMOVED,
};

/// Every messaging event type — the single list the tables below are
/// built from, so adding a tenth type is one edit that every policy
/// sees.
pub const ALL_MESSAGING_EVENT_TYPES: [EventTypeId; 9] = [
    EVENT_TYPE_MESSAGE_CREATED,
    EVENT_TYPE_MESSAGE_QUEUED,
    EVENT_TYPE_MESSAGE_RECEIVED,
    EVENT_TYPE_MESSAGE_DELIVERED,
    EVENT_TYPE_MESSAGE_READ,
    EVENT_TYPE_MESSAGE_EDITED,
    EVENT_TYPE_MESSAGE_DELETED,
    EVENT_TYPE_REACTION_ADDED,
    EVENT_TYPE_REACTION_REMOVED,
];

/// Outbox effect: transmit a queued message. Numbered in messaging's
/// own block alongside its event types.
pub const EFFECT_SEND_MESSAGE: EffectKind = EffectKind(100);

/// What the send effect carries: pointers, never the ciphertext (§15 —
/// the ciphertext lives in `siar-storage` and in the `MessageCreated`
/// event).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageIntent {
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
}

/// §30: `MessageQueued` obliges a send; `MessageDelivered` discharges
/// it. Discharge is found through the event's `causation_id` — the
/// chain `MessageService`'s correlation registry builds
/// (`Created → Queued → Delivered`). When a process restart lost that
/// registry the `Delivered` event has no causation and discharges
/// nothing; the operation then looks owed to history, which is exactly
/// what [`MessagingRestoreGuard`] exists to catch with state the log
/// doesn't have.
#[derive(Debug, Clone, Copy, Default)]
pub struct MessagingEffectPlanner;

impl EffectPlanner for MessagingEffectPlanner {
    fn plan(&self, event: &EventRef<'_>) -> Vec<EffectIntent> {
        if event.event_type != EVENT_TYPE_MESSAGE_QUEUED {
            return Vec::new();
        }
        match decode_messaging_event(event.schema_version, event.payload) {
            Ok(MessagingEvent::MessageQueued {
                conversation_id,
                message_id,
            }) => vec![EffectIntent {
                kind: EFFECT_SEND_MESSAGE,
                slot: 0,
                payload: postcard::to_allocvec(&SendMessageIntent {
                    conversation_id,
                    message_id,
                })
                .expect("SendMessageIntent always serializes"),
            }],
            // An undecodable Queued event creates no obligation: it is
            // reported by §62/§9 machinery, not turned into a send.
            _ => Vec::new(),
        }
    }

    fn closes(&self, event: &EventRef<'_>) -> Vec<OperationId> {
        match (
            event.event_type == EVENT_TYPE_MESSAGE_DELIVERED,
            event.causation_id,
        ) {
            (true, Some(queued)) => vec![OperationId::derive(queued, EFFECT_SEND_MESSAGE, 0)],
            _ => Vec::new(),
        }
    }
}

/// §49/§50: every messaging event replicates to the account's own
/// devices. All nine, deliberately — a mixed `LocalOnly`/`OwnDevices`
/// stream would leave version holes the receiver can't tell from loss
/// (`siar-event-log::replication`'s stream-layout rule). Delivery and
/// read state is a fact about the message that the user's other devices
/// should show.
pub fn messaging_scope_table() -> ScopeTable {
    ALL_MESSAGING_EVENT_TYPES
        .into_iter()
        .fold(ScopeTable::new(), |t, ty| {
            t.with(ty, ReplicationScope::OwnDevices)
        })
}

/// §40: user-authored content is the user's to delete
/// (`UserControlled` — never auto-compacted); delivery bookkeeping is
/// operational noise once old.
pub fn messaging_retention_table() -> RetentionTable {
    use RetentionClass::{OperationalShortTerm, UserControlled};
    RetentionTable::new()
        .with(EVENT_TYPE_MESSAGE_CREATED, UserControlled)
        .with(EVENT_TYPE_MESSAGE_EDITED, UserControlled)
        .with(EVENT_TYPE_MESSAGE_DELETED, UserControlled)
        .with(EVENT_TYPE_MESSAGE_RECEIVED, UserControlled)
        .with(EVENT_TYPE_REACTION_ADDED, UserControlled)
        .with(EVENT_TYPE_REACTION_REMOVED, UserControlled)
        .with(EVENT_TYPE_MESSAGE_QUEUED, OperationalShortTerm)
        .with(EVENT_TYPE_MESSAGE_DELIVERED, OperationalShortTerm)
        .with(EVENT_TYPE_MESSAGE_READ, OperationalShortTerm)
}

/// §62: this build understands exactly its nine types. No optional
/// range: an unknown *messaging* event could be an edit or a deletion,
/// and ignoring one would show a message the user has deleted.
pub fn messaging_unknown_policy() -> UnknownEventPolicy {
    UnknownEventPolicy::new().with_known(ALL_MESSAGING_EVENT_TYPES)
}

/// §55: content-bearing events keep the store default; everything else
/// is a few ids and must stay that small. A `ReactionAdded` carrying
/// kilobytes of "emoji" is a bug or an attack, not a reaction.
pub fn messaging_event_limits() -> EventLimits {
    let small = 1024;
    [
        EVENT_TYPE_MESSAGE_QUEUED,
        EVENT_TYPE_MESSAGE_RECEIVED,
        EVENT_TYPE_MESSAGE_DELIVERED,
        EVENT_TYPE_MESSAGE_READ,
        EVENT_TYPE_MESSAGE_DELETED,
        EVENT_TYPE_REACTION_ADDED,
        EVENT_TYPE_REACTION_REMOVED,
    ]
    .into_iter()
    .fold(EventLimits::new(), |l, ty| {
        l.with_type(ty, small).expect("1 KiB is within bounds")
    })
}

/// §9: every messaging type is at schema version
/// [`CURRENT_MESSAGING_EVENT_SCHEMA_VERSION`] with no history to
/// upcast. The registry exists *now* so that the day a `V2` lands,
/// adding its step and bumping the version is the whole change, and
/// `verify_chain` in the tests fails if the step is forgotten.
pub fn messaging_upcasters() -> UpcasterRegistry {
    let mut r = UpcasterRegistry::new();
    for ty in ALL_MESSAGING_EVENT_TYPES {
        r.set_latest(ty, CURRENT_MESSAGING_EVENT_SCHEMA_VERSION);
    }
    r
}

/// What the system of record (`siar-storage`) knows about a message —
/// the facts §71 says the event log cannot supply after a restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    /// Still needs sending.
    Pending,
    /// Already delivered (acknowledged after the backup was taken).
    Delivered,
    /// The user deleted it since.
    Deleted,
    /// Too old to be worth sending now.
    Expired,
    /// The recipient device/account has been revoked.
    RecipientRevoked,
}

/// §71's guard for messaging: consult current state per message before
/// letting a restored send run. `lookup` is a closure so this crate
/// doesn't dictate *how* the caller reads `siar-storage`.
pub struct MessagingRestoreGuard {
    lookup: Arc<dyn Fn(ConversationId, MessageId) -> MessageStatus + Send + Sync>,
}

impl MessagingRestoreGuard {
    pub fn new(
        lookup: impl Fn(ConversationId, MessageId) -> MessageStatus + Send + Sync + 'static,
    ) -> Self {
        Self {
            lookup: Arc::new(lookup),
        }
    }
}

impl RestoreGuard for MessagingRestoreGuard {
    fn assess(&self, entry: &OutboxEntry, _now: Timestamp) -> GuardVerdict {
        if entry.kind != EFFECT_SEND_MESSAGE {
            // Not ours to judge: refuse rather than blindly resend.
            return GuardVerdict::Drop {
                reason: "unrecognised effect kind",
            };
        }
        let Ok(intent) = postcard::from_bytes::<SendMessageIntent>(&entry.payload) else {
            return GuardVerdict::Drop {
                reason: "undecodable send intent",
            };
        };
        match (self.lookup)(intent.conversation_id, intent.message_id) {
            MessageStatus::Pending => GuardVerdict::Proceed,
            MessageStatus::Delivered => GuardVerdict::Drop {
                reason: "already delivered",
            },
            MessageStatus::Deleted => GuardVerdict::Drop {
                reason: "message deleted",
            },
            MessageStatus::Expired => GuardVerdict::Drop { reason: "expired" },
            MessageStatus::RecipientRevoked => GuardVerdict::Drop {
                reason: "recipient revoked",
            },
        }
    }
}

/// §64/§65: the conversation's stream within one identity's namespace.
/// Isolation itself comes from giving each identity its own store (see
/// `siar_event_log::namespace`); this id is the defence-in-depth layer.
pub fn identity_conversation_stream_id(
    namespace: &NamespaceId,
    conversation_id: ConversationId,
) -> StreamId {
    namespaced_stream_id(namespace, &format!("conversation:{conversation_id}"))
}

/// A per-writer stream for one conversation — the single-writer layout
/// cursor replication needs. See the module doc's layout finding.
pub fn replication_stream_id(conversation_id: ConversationId, writer: DeviceId) -> StreamId {
    StreamId::from_name(&format!("conversation:{conversation_id}:writer:{writer}"))
}

/// §41 for one message: physically remove its content from every copy
/// `siar-event-log` owns, and report what it couldn't reach.
///
/// Redacts the payload of the message's `MessageCreated` and
/// `MessageEdited` events (the ciphertext-bearing ones — everything
/// else is ids), purges the conversation's snapshots, and scrubs the
/// message's send operation(s). It does **not** append the logical
/// `MessageDeleted` event: that is what [`crate::MessageService`]
/// already records, and keeping the two separate means a deletion can be
/// replicated (the logical event) independently of the local physical
/// erasure.
///
/// `siar-storage`'s own message row is outside this function's reach —
/// deleting it stays `MessageRepository`'s job — and so are projections
/// ([`DeletionReport::affected_streams`] says which to rebuild) and peers'
/// copies.
pub async fn delete_message_content<S, N, O>(
    store: &S,
    snapshots: &N,
    outbox: &O,
    conversation_id: ConversationId,
    message_id: MessageId,
    now: Timestamp,
) -> Result<DeletionReport, MessageDeletionError>
where
    S: RedactableStore + EventStore + ?Sized,
    N: SnapshotStore + ?Sized,
    O: Outbox + ?Sized,
{
    let stream = conversation_stream_id(conversation_id);
    let mut events: Vec<EventId> = Vec::new();
    let mut operations: Vec<OperationId> = Vec::new();

    let mut from = 0u64;
    loop {
        let page = store.read_stream(stream, from, 256).await?;
        let Some(last) = page.last() else { break };
        from = last.envelope.stream_version;
        for stored in &page {
            // Events this build can't decode can't be attributed to a
            // message; they are left alone rather than guessed at.
            let Ok(ev) =
                decode_messaging_event(stored.envelope.schema_version, &stored.envelope.payload)
            else {
                continue;
            };
            match ev {
                MessagingEvent::MessageCreated { message_id: m, .. }
                | MessagingEvent::MessageEdited { message_id: m, .. }
                    if m == message_id =>
                {
                    events.push(stored.envelope.event_id);
                }
                MessagingEvent::MessageQueued { message_id: m, .. } if m == message_id => {
                    operations.push(OperationId::derive(
                        stored.envelope.event_id,
                        EFFECT_SEND_MESSAGE,
                        0,
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(execute_deletion(
        store,
        snapshots,
        outbox,
        &DeletionRequest {
            events,
            streams: vec![stream],
            operations,
        },
        now,
    )
    .await?)
}

#[derive(Debug, thiserror::Error)]
pub enum MessageDeletionError {
    #[error(transparent)]
    Store(#[from] siar_event_log::EventStoreError),
    #[error(transparent)]
    Deletion(#[from] DeletionError),
}

/// Origin helper so callers and tests build events the way
/// `MessageService` does.
pub fn local_origin(device: DeviceId) -> EventOrigin {
    EventOrigin::LocalDevice(device)
}
