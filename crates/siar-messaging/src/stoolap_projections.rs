//! The durable counterpart to [`crate::projections::
//! ConversationSummaryProjection`] — same `stoolap`-backed pattern
//! `siar_event_log::stoolap_store::StoolapEventStore`/`siar_event_log::
//! stoolap_checkpoint_store::StoolapCheckpointStore` already
//! established, applied here to a real domain projection instead of
//! the event log or its checkpoints. This is the first real caller
//! `StoolapCheckpointStore` gets — see that module's own doc comment
//! on why pairing THIS (a durable projection) with an in-memory
//! checkpoint would have been wrong.
//!
//! ## Schema
//!
//! One row per `ConversationId` — `conversation_id` TEXT (a
//! [`siar_domain::ConversationId`]'s own `Display`/`Uuid` string
//! form, same convention `siar_event_log::stoolap_store`'s own
//! `event_id`/`correlation_id` columns already use), `message_count`
//! INTEGER, `last_message_id` TEXT (nullable — no message yet),
//! `last_activity_at` INTEGER (nullable for the same reason, though in
//! practice a row is never inserted without at least one event having
//! set it).
//!
//! ## Why this crate, not `siar-event-log`
//!
//! Same reasoning `siar_messaging::events`'s own doc comment gives for
//! why the §33 event catalog lives here and not in `siar-event-log`:
//! a `Projection` is domain-specific by definition (§16's own examples
//! span five-plus different domains), so a CONCRETE one belongs in the
//! crate that owns that domain, not in the reusable, domain-agnostic
//! event-log crate.
//!
//! ## Concurrency
//!
//! Same conservative choice `stoolap_store::append` already makes and
//! documents: a process-local [`std::sync::Mutex`] serializes the
//! whole read-modify-write critical section inside
//! [`StoolapConversationSummaryProjection::apply`]/`reset`, rather than
//! leaning on `stoolap`'s own isolation guarantees for a
//! read-then-write race. In this deployment, `apply` is only ever
//! actually called from `ProjectionRunner::catch_up`, itself only ever
//! called from `MessageService::record_messaging_event`'s own
//! single-append-then-catch-up sequence — so today this guard is
//! defensive against a hypothetical second caller, not a real observed
//! race, and is cheap enough to keep anyway.

use std::sync::Mutex;

use async_trait::async_trait;
use siar_domain::{ConversationId, MessageId};
use siar_event_log::ids::Timestamp;
use siar_event_log::projection::{Projection, ProjectionError, ProjectionId};
use siar_event_log::store::StoredEvent;
use stoolap::Database;
use uuid::Uuid;

use crate::events::{
    decode_messaging_event, MessagingEvent, EVENT_TYPE_MESSAGE_CREATED, EVENT_TYPE_REACTION_REMOVED,
};
use crate::projections::{ConversationSummary, ConversationSummaryQuery};

/// See this module's own doc comment.
pub struct StoolapConversationSummaryProjection {
    db: Database,
    apply_lock: Mutex<()>,
}

impl StoolapConversationSummaryProjection {
    /// Opens (or creates) a file-backed projection store at `path` and
    /// applies the schema. Same `"file://"`-prefixed DSN handling
    /// `StoolapEventStore::open`'s own doc comment explains.
    pub fn open(path: &str) -> Result<Self, ProjectionError> {
        let dsn = format!("file://{path}");
        let db = Database::open(&dsn).map_err(backend_err)?;
        Self::from_database(db)
    }

    /// In-memory — for tests, and for any caller that wants this real
    /// schema/query behavior without a file on disk.
    pub fn open_in_memory() -> Result<Self, ProjectionError> {
        let db = Database::open_in_memory().map_err(backend_err)?;
        Self::from_database(db)
    }

    fn from_database(db: Database) -> Result<Self, ProjectionError> {
        apply_schema(&db)?;
        Ok(Self {
            db,
            apply_lock: Mutex::new(()),
        })
    }
}

fn apply_schema(db: &Database) -> Result<(), ProjectionError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS conversation_summaries (
            conversation_id  TEXT NOT NULL,
            message_count    INTEGER NOT NULL,
            last_message_id  TEXT,
            last_activity_at INTEGER
        )",
        (),
    )
    .map_err(backend_err)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_conversation_summaries_id
         ON conversation_summaries (conversation_id)",
        (),
    )
    .map_err(backend_err)?;
    Ok(())
}

fn backend_err(e: impl std::fmt::Display) -> ProjectionError {
    ProjectionError::Projection(e.to_string())
}

fn parse_message_id(s: &str) -> Result<MessageId, ProjectionError> {
    Uuid::parse_str(s)
        .map(MessageId::from_uuid)
        .map_err(|e| ProjectionError::Projection(format!("corrupt last_message_id: {e}")))
}

#[async_trait]
impl Projection for StoolapConversationSummaryProjection {
    fn projection_id(&self) -> ProjectionId {
        ProjectionId("conversation_summary")
    }

    fn projection_version(&self) -> u16 {
        1
    }

    /// Same event-type-range filter, same only-`MessageCreated`/
    /// `MessageReceived`-bump-the-count logic
    /// `ConversationSummaryProjection::apply`'s own comments already
    /// explain in full — this is the durable version of the identical
    /// decision, not a different one.
    async fn apply(&self, event: &StoredEvent) -> Result<(), ProjectionError> {
        let event_type = event.envelope.event_type;
        if !(EVENT_TYPE_MESSAGE_CREATED.0..=EVENT_TYPE_REACTION_REMOVED.0).contains(&event_type.0) {
            return Ok(());
        }

        let messaging_event =
            decode_messaging_event(event.envelope.schema_version, &event.envelope.payload)
                .map_err(|e| ProjectionError::Projection(format!("undecodable §33 event: {e}")))?;
        let conversation_id = messaging_event.conversation_id();
        let conversation_id_str = conversation_id.to_string();

        let _guard = self
            .apply_lock
            .lock()
            .expect("StoolapConversationSummaryProjection lock poisoned");

        // Fetch BOTH existing fields, not just `message_count`: an
        // event that doesn't prove a new message exists (everything
        // except `MessageCreated`/`MessageReceived`) must carry the
        // existing `last_message_id` FORWARD, not overwrite it with
        // `NULL` — the same "preserve what `apply` didn't touch"
        // semantics `HashMap::entry().or_default()` gives the
        // in-memory version of this projection for free, which a
        // manual `DELETE`+`INSERT` here does NOT get for free without
        // explicitly carrying every untouched column forward itself.
        let (existing_count, mut last_message_id): (i64, Option<String>) = {
            let mut rows = self
                .db
                .query(
                    "SELECT message_count, last_message_id
                     FROM conversation_summaries WHERE conversation_id = $1",
                    (conversation_id_str.clone(),),
                )
                .map_err(backend_err)?;
            match rows.next() {
                Some(row) => {
                    let row = row.map_err(backend_err)?;
                    (
                        row.get(0).map_err(backend_err)?,
                        row.get(1).map_err(backend_err)?,
                    )
                }
                None => (0, None),
            }
        };

        let mut message_count = existing_count;
        if let MessagingEvent::MessageCreated { message_id, .. }
        | MessagingEvent::MessageReceived { message_id, .. } = &messaging_event
        {
            message_count += 1;
            last_message_id = Some(message_id.to_string());
        }
        let last_activity_at = event.envelope.created_at.0 as i64;

        self.db
            .execute(
                "DELETE FROM conversation_summaries WHERE conversation_id = $1",
                (conversation_id_str.clone(),),
            )
            .map_err(backend_err)?;
        self.db
            .execute(
                "INSERT INTO conversation_summaries
                    (conversation_id, message_count, last_message_id, last_activity_at)
                 VALUES ($1, $2, $3, $4)",
                (
                    conversation_id_str,
                    message_count,
                    last_message_id,
                    last_activity_at,
                ),
            )
            .map_err(backend_err)?;

        Ok(())
    }

    async fn reset(&self) -> Result<(), ProjectionError> {
        let _guard = self
            .apply_lock
            .lock()
            .expect("StoolapConversationSummaryProjection lock poisoned");
        self.db
            .execute("DELETE FROM conversation_summaries", ())
            .map_err(backend_err)?;
        Ok(())
    }
}

#[async_trait]
impl ConversationSummaryQuery for StoolapConversationSummaryProjection {
    async fn get(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Option<ConversationSummary>, ProjectionError> {
        let mut rows = self
            .db
            .query(
                "SELECT message_count, last_message_id, last_activity_at
                 FROM conversation_summaries WHERE conversation_id = $1",
                (conversation_id.to_string(),),
            )
            .map_err(backend_err)?;

        match rows.next() {
            Some(row) => {
                let row = row.map_err(backend_err)?;
                let message_count: i64 = row.get(0).map_err(backend_err)?;
                let last_message_id: Option<String> = row.get(1).map_err(backend_err)?;
                let last_activity_at: Option<i64> = row.get(2).map_err(backend_err)?;
                let last_message_id = last_message_id.map(|s| parse_message_id(&s)).transpose()?;
                Ok(Some(ConversationSummary {
                    message_count: message_count as u64,
                    last_message_id,
                    last_activity_at: last_activity_at.map(|t| Timestamp(t as u64)),
                }))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MessagingEvent;
    use siar_domain::DeviceId;
    use siar_event_log::envelope::EventEnvelope;
    use siar_event_log::envelope::EventOrigin;
    use siar_event_log::ids::EventId;

    fn stored_event(event: MessagingEvent, device: DeviceId) -> StoredEvent {
        let new_event =
            event.into_new_event(EventId::new(), EventOrigin::LocalDevice(device), None, None);
        StoredEvent {
            envelope: EventEnvelope {
                event_id: new_event.event_id,
                stream_id: siar_event_log::ids::StreamId::from_name("test"),
                stream_version: 1,
                event_type: new_event.event_type,
                schema_version: new_event.schema_version,
                created_at: new_event.created_at,
                origin: new_event.origin,
                correlation_id: new_event.correlation_id,
                causation_id: new_event.causation_id,
                payload: new_event.payload,
            },
            local_offset: siar_event_log::ids::LocalLogOffset(1),
        }
    }

    #[tokio::test]
    async fn a_created_message_is_counted_and_becomes_the_latest() {
        let projection = StoolapConversationSummaryProjection::open_in_memory().unwrap();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let device = DeviceId::new();

        projection
            .apply(&stored_event(
                MessagingEvent::MessageCreated {
                    conversation_id,
                    message_id,
                    sender_device: device,
                    sequence: 1,
                    ciphertext: vec![1, 2, 3],
                },
                device,
            ))
            .await
            .unwrap();

        let summary = projection.get(conversation_id).await.unwrap().unwrap();
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.last_message_id, Some(message_id));
    }

    /// The real bug caught and fixed while writing this module (see
    /// `apply`'s own doc comment on carrying `last_message_id`
    /// forward): a `MessageDelivered` event must not blank out the
    /// `last_message_id` a prior `MessageCreated` already set, and
    /// must not bump `message_count` again either.
    #[tokio::test]
    async fn a_delivery_event_preserves_the_existing_last_message_id_and_does_not_double_count() {
        let projection = StoolapConversationSummaryProjection::open_in_memory().unwrap();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let device = DeviceId::new();

        projection
            .apply(&stored_event(
                MessagingEvent::MessageCreated {
                    conversation_id,
                    message_id,
                    sender_device: device,
                    sequence: 1,
                    ciphertext: vec![],
                },
                device,
            ))
            .await
            .unwrap();
        let after_created = projection.get(conversation_id).await.unwrap().unwrap();

        projection
            .apply(&stored_event(
                MessagingEvent::MessageDelivered {
                    conversation_id,
                    message_id,
                },
                device,
            ))
            .await
            .unwrap();
        let after_delivered = projection.get(conversation_id).await.unwrap().unwrap();

        assert_eq!(after_delivered.message_count, after_created.message_count);
        assert_eq!(after_delivered.last_message_id, Some(message_id));
        assert!(after_delivered.last_activity_at >= after_created.last_activity_at);
    }

    #[tokio::test]
    async fn an_event_type_outside_messaging_s_own_range_is_silently_skipped() {
        let projection = StoolapConversationSummaryProjection::open_in_memory().unwrap();
        let event = StoredEvent {
            envelope: EventEnvelope {
                event_id: EventId::new(),
                stream_id: siar_event_log::ids::StreamId::from_name("identity:some-account"),
                stream_version: 1,
                // Identity's own range (1-8) — see `crate::events`'s
                // doc comment on the informal per-domain convention.
                event_type: siar_event_log::ids::EventTypeId(1),
                schema_version: 1,
                created_at: Timestamp::now(),
                origin: EventOrigin::LocalDevice(DeviceId::new()),
                correlation_id: None,
                causation_id: None,
                payload: vec![0xFF, 0xFF],
            },
            local_offset: siar_event_log::ids::LocalLogOffset(1),
        };

        // No panic, no error.
        projection.apply(&event).await.unwrap();
    }

    #[tokio::test]
    async fn reset_clears_every_conversation() {
        let projection = StoolapConversationSummaryProjection::open_in_memory().unwrap();
        let conversation_id = ConversationId::new();
        let device = DeviceId::new();

        projection
            .apply(&stored_event(
                MessagingEvent::MessageCreated {
                    conversation_id,
                    message_id: MessageId::new(),
                    sender_device: device,
                    sequence: 1,
                    ciphertext: vec![],
                },
                device,
            ))
            .await
            .unwrap();
        assert!(projection.get(conversation_id).await.unwrap().is_some());

        projection.reset().await.unwrap();
        assert!(projection.get(conversation_id).await.unwrap().is_none());
    }

    /// Same real durability proof `stoolap_store`/
    /// `stoolap_checkpoint_store`'s own equivalent tests make: an
    /// actual `std::fs` round trip through a real file.
    #[tokio::test]
    async fn summaries_survive_closing_and_reopening_the_same_file() {
        let path = std::env::temp_dir().join(format!(
            "siar-messaging-conversation-summary-durability-test-{}",
            Uuid::new_v4()
        ));
        let path_str = path.to_str().unwrap().to_string();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let device = DeviceId::new();

        {
            let projection = StoolapConversationSummaryProjection::open(&path_str).unwrap();
            projection
                .apply(&stored_event(
                    MessagingEvent::MessageCreated {
                        conversation_id,
                        message_id,
                        sender_device: device,
                        sequence: 1,
                        ciphertext: vec![],
                    },
                    device,
                ))
                .await
                .unwrap();
        }

        {
            let projection = StoolapConversationSummaryProjection::open(&path_str).unwrap();
            let summary = projection.get(conversation_id).await.unwrap().unwrap();
            assert_eq!(summary.message_count, 1);
            assert_eq!(summary.last_message_id, Some(message_id));
        }

        let _ = std::fs::remove_file(&path);
    }
}
