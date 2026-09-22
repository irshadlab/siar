//! `04-offline-event-log-architecture.md` §16's own worked example,
//! made real: `conversation_summary` is one of the seven named
//! materialized views in that section's list, and this is the first
//! real [`siar_event_log::projection::Projection`] implementation
//! anywhere in this workspace.
//!
//! ## What it materializes, and why exactly these three fields
//!
//! [`ConversationSummary`] tracks, per conversation: how many messages
//! exist (counting each message once, at the event that first proves
//! it exists — see [`ConversationSummaryProjection::apply`]'s own
//! comment on why `MessageCreated`/`MessageReceived` are the only two
//! variants that increment it, not all nine), which message was most
//! recent, and when this conversation last had any activity at all
//! (every one of the nine §33 events updates this last field, since a
//! delivery confirmation or a reaction is still "this conversation is
//! alive" for a UI's own recency sort, even though it isn't a new
//! message). Deliberately NOT message content, delivery state per
//! individual message, or per-message history — `siar-storage`'s own
//! `messages`/`StoredMessage` already IS that, real and durable; this
//! projection exists to prove out §16's own architecture end to end
//! with something genuinely useful, not to duplicate what already
//! exists.
//!
//! ## Not durable
//!
//! In-memory only ([`std::collections::HashMap`] behind a
//! [`std::sync::Mutex`], same shape
//! [`siar_event_log::memory_store::InMemoryEventStore`] itself uses) —
//! matching [`siar_event_log::projection::InMemoryCheckpointStore`],
//! which is the only [`siar_event_log::projection::
//! ProjectionCheckpointStore`] this workspace has so far (see that
//! module's own doc comment). A durable version would need a real
//! table, following `stoolap_store`'s own schema pattern — not built
//! this round, since nothing about this specific projection needed
//! cross-restart durability to prove out §16/§18 for real.
//!
//! ## Read-your-writes, wired
//!
//! `MessageService::record_messaging_event` calls
//! [`siar_event_log::projection::ProjectionRunner::catch_up`] against
//! this projection synchronously, right after every real `append`
//! succeeds (see that method's own doc comment) — `MessageService::
//! conversation_summary` is the query side, immediately reflecting
//! what was just appended, exactly §18's own worked example
//! ("SendMessage succeeds locally → conversation immediately shows
//! message"), just for a summary view rather than the message itself
//! (which `siar-storage`'s own `insert_if_new`/`enqueue` already
//! updates synchronously, with or without any of this).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use siar_domain::{ConversationId, MessageId};
use siar_event_log::ids::Timestamp;
use siar_event_log::projection::{Projection, ProjectionError, ProjectionId};
use siar_event_log::store::StoredEvent;

use crate::events::{
    decode_messaging_event, MessagingEvent, EVENT_TYPE_MESSAGE_CREATED, EVENT_TYPE_REACTION_REMOVED,
};

/// One conversation's materialized summary — see this module's own
/// doc comment for exactly what each field means and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConversationSummary {
    pub message_count: u64,
    pub last_message_id: Option<MessageId>,
    pub last_activity_at: Option<Timestamp>,
}

/// The read side both the in-memory [`ConversationSummaryProjection`]
/// and the durable `siar_messaging::stoolap_projections::
/// StoolapConversationSummaryProjection` implement — a supertrait of
/// [`Projection`] (not a separate, unrelated trait) so
/// `MessageService` can hold either backend as one
/// `Arc<dyn ConversationSummaryQuery>` and pass the SAME value to both
/// `ProjectionRunner::catch_up` (which only needs the `Projection`
/// half) and its own query method (which needs this trait's own
/// `get`), via Rust's trait-object upcasting (stable since 1.86 — this
/// workspace already pins a newer toolchain, see the root
/// `rust-toolchain.toml`) rather than storing the same projection
/// behind two separate `Arc`s that would have to be kept in sync by
/// construction alone.
///
/// Async even for the in-memory implementation, where the underlying
/// lookup is a synchronous `Mutex` lock with no real I/O — matching
/// the durable implementation's own real query, which IS a `stoolap`
/// read (a `Result`, since that one really can fail). One shared
/// signature both backends actually satisfy, rather than a sync
/// signature that would have to lie about the durable backend's real
/// failure modes, or two different signatures `MessageService` would
/// have to match on by backend.
#[async_trait]
pub trait ConversationSummaryQuery: Projection {
    async fn get(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ConversationSummary>, ProjectionError>;
}

/// See this module's own doc comment.
#[derive(Default)]
pub struct ConversationSummaryProjection {
    summaries: Mutex<HashMap<ConversationId, ConversationSummary>>,
}

impl ConversationSummaryProjection {
    pub fn new() -> Self {
        Self::default()
    }

    /// The synchronous, infallible query — kept as a plain inherent
    /// method (not just the [`ConversationSummaryQuery`] trait's own
    /// `async fn get`, which this type also implements) since this
    /// backend's own lookup genuinely can't fail and genuinely doesn't
    /// need to be `.await`ed; this module's own unit tests use this
    /// form directly. `None` means "no activity recorded for this
    /// conversation yet" (either genuinely none, or this projection
    /// simply hasn't been asked to catch up since it happened; see
    /// this module's own doc comment on how `MessageService` keeps the
    /// two in sync in practice).
    pub fn get_sync(&self, conversation_id: ConversationId) -> Option<ConversationSummary> {
        self.summaries
            .lock()
            .expect("ConversationSummaryProjection lock poisoned")
            .get(&conversation_id)
            .copied()
    }
}

#[async_trait]
impl ConversationSummaryQuery for ConversationSummaryProjection {
    async fn get(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ConversationSummary>, ProjectionError> {
        Ok(self.get_sync(conversation_id))
    }
}

#[async_trait]
impl Projection for ConversationSummaryProjection {
    fn projection_id(&self) -> ProjectionId {
        ProjectionId("conversation_summary")
    }

    fn projection_version(&self) -> u16 {
        1
    }

    async fn apply(&self, event: &StoredEvent) -> Result<(), ProjectionError> {
        let event_type = event.envelope.event_type;
        // §33's own numeric range (100-108, see `events.rs`'s own doc
        // comment on the informal per-domain convention) — a real
        // requirement once this projection's own `EventStore` is ever
        // shared with identity's or files' event catalogs on the same
        // physical log, not a hypothetical: `read_log` (which
        // `ProjectionRunner::catch_up` calls) spans every stream,
        // messaging or not. Skip, don't error — an unrecognized event
        // type here is an expected, ordinary case, not corruption.
        if !(EVENT_TYPE_MESSAGE_CREATED.0..=EVENT_TYPE_REACTION_REMOVED.0).contains(&event_type.0) {
            return Ok(());
        }

        let messaging_event =
            decode_messaging_event(event.envelope.schema_version, &event.envelope.payload)
                .map_err(|e| ProjectionError::Projection(format!("undecodable §33 event: {e}")))?;
        let conversation_id = messaging_event.conversation_id();

        let mut summaries = self
            .summaries
            .lock()
            .expect("ConversationSummaryProjection lock poisoned");
        let summary = summaries.entry(conversation_id).or_default();
        summary.last_activity_at = Some(event.envelope.created_at);

        // Only these two variants mean "a message that didn't exist
        // before now exists" — `MessageQueued`/`Delivered`/`Read`/
        // `Edited`/`Deleted` and both reaction events are all about a
        // message this projection already counted (or, for
        // `MessageQueued`, is about to be counted a moment later by
        // the very next event `send_text` records — see
        // `service.rs`'s own comment on why `MessageCreated` and
        // `MessageQueued` are always recorded together). Counting on
        // any of those too would double-count every message at least
        // once.
        if let MessagingEvent::MessageCreated { message_id, .. }
        | MessagingEvent::MessageReceived { message_id, .. } = &messaging_event
        {
            summary.message_count += 1;
            summary.last_message_id = Some(*message_id);
        }

        Ok(())
    }

    async fn reset(&self) -> Result<(), ProjectionError> {
        self.summaries
            .lock()
            .expect("ConversationSummaryProjection lock poisoned")
            .clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::conversation_stream_id;
    use siar_domain::DeviceId;
    use siar_event_log::envelope::EventOrigin;
    use siar_event_log::ids::EventId;
    use siar_event_log::memory_store::InMemoryEventStore;
    use siar_event_log::projection::{InMemoryCheckpointStore, ProjectionRunner};
    use siar_event_log::store::{AppendRequest, EventStore};

    async fn append(store: &InMemoryEventStore, event: MessagingEvent, device: DeviceId) {
        let stream_id = event.stream_id();
        store
            .append(AppendRequest {
                stream_id,
                expected_version: 0,
                events: vec![event.into_new_event(
                    EventId::new(),
                    EventOrigin::LocalDevice(device),
                    None,
                    None,
                )],
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_created_message_is_counted_and_becomes_the_latest() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let projection = ConversationSummaryProjection::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let device = DeviceId::new();

        append(
            &store,
            MessagingEvent::MessageCreated {
                conversation_id,
                message_id,
                sender_device: device,
                sequence: 1,
                ciphertext: vec![1, 2, 3],
            },
            device,
        )
        .await;

        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection, 200).await.unwrap();

        let summary = projection.get_sync(conversation_id).unwrap();
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.last_message_id, Some(message_id));
    }

    #[tokio::test]
    async fn delivery_and_read_events_update_activity_but_not_the_count() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let projection = ConversationSummaryProjection::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let device = DeviceId::new();

        // Use different streams-per-append-call is fine here since
        // each call already starts a fresh `expected_version: 0` in
        // the test helper above — this test only cares about
        // `apply`'s own per-event-type behavior, not stream
        // versioning (already covered in `siar_event_log::projection`'s
        // own tests).
        append(
            &store,
            MessagingEvent::MessageCreated {
                conversation_id,
                message_id,
                sender_device: device,
                sequence: 1,
                ciphertext: vec![],
            },
            device,
        )
        .await;
        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection, 200).await.unwrap();
        let after_created = projection.get_sync(conversation_id).unwrap();

        store
            .append(AppendRequest {
                stream_id: conversation_stream_id(conversation_id),
                expected_version: 1,
                events: vec![MessagingEvent::MessageDelivered {
                    conversation_id,
                    message_id,
                }
                .into_new_event(
                    EventId::new(),
                    EventOrigin::RemoteDevice(device),
                    None,
                    None,
                )],
            })
            .await
            .unwrap();
        runner.catch_up(&projection, 200).await.unwrap();

        let after_delivered = projection.get_sync(conversation_id).unwrap();
        assert_eq!(
            after_delivered.message_count, after_created.message_count,
            "MessageDelivered must not double-count the message"
        );
        assert!(after_delivered.last_activity_at >= after_created.last_activity_at);
    }

    #[tokio::test]
    async fn an_event_type_outside_messaging_s_own_range_is_silently_skipped() {
        use siar_event_log::ids::{EventId, EventTypeId, StreamId};
        use siar_event_log::store::NewEvent;

        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let projection = ConversationSummaryProjection::new();

        // EventTypeId(1) is identity's own range (see `events.rs`'s
        // doc comment) — nothing here should panic or error just
        // because a non-messaging event shares the log.
        store
            .append(AppendRequest {
                stream_id: StreamId::from_name("identity:some-account"),
                expected_version: 0,
                events: vec![NewEvent {
                    event_id: EventId::new(),
                    event_type: EventTypeId(1),
                    schema_version: 1,
                    created_at: Timestamp::now(),
                    origin: EventOrigin::LocalDevice(DeviceId::new()),
                    correlation_id: None,
                    causation_id: None,
                    payload: vec![0xFF, 0xFF],
                }],
            })
            .await
            .unwrap();

        let runner = ProjectionRunner::new(&store, &checkpoints);
        let applied = runner.catch_up(&projection, 200).await.unwrap();
        assert_eq!(applied, 1, "the event was still consumed from the log");
        // No panic, no error, and nothing materialized for it.
    }

    #[tokio::test]
    async fn reset_clears_every_conversation() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let projection = ConversationSummaryProjection::new();
        let conversation_id = ConversationId::new();
        let device = DeviceId::new();

        append(
            &store,
            MessagingEvent::MessageCreated {
                conversation_id,
                message_id: MessageId::new(),
                sender_device: device,
                sequence: 1,
                ciphertext: vec![],
            },
            device,
        )
        .await;
        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection, 200).await.unwrap();
        assert!(projection.get_sync(conversation_id).is_some());

        projection.reset().await.unwrap();
        assert!(projection.get_sync(conversation_id).is_none());
    }
}
