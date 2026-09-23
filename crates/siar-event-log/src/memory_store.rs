//! A real, in-memory [`EventStore`] implementation. §19 names SQLite as
//! the recommended backend — that's §92's own Phase 2, not Phase 1
//! (`EventId`/`StreamId`/`EventEnvelope`/`LocalLogOffset`/`EventStore
//! trait`); this crate stops at Phase 1 plus this in-memory
//! implementation, which exists to make the trait's own contract
//! (atomicity, optimistic concurrency, idempotency) real and testable
//! without a database dependency, the same role
//! `siar-identity-multidevice::TrustedAccountStore` plays for that
//! crate.

use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::envelope::EventEnvelope;
use crate::ids::{EventId, LocalLogOffset, StreamId};
use crate::store::{
    validate_payload_size, AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent,
};

#[derive(Default)]
struct Inner {
    streams: HashMap<StreamId, Vec<StoredEvent>>,
    log: Vec<StoredEvent>,
    seen_event_ids: HashSet<EventId>,
}

#[derive(Default)]
pub struct InMemoryEventStore {
    inner: Mutex<Inner>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    /// §11's transaction, made real: the whole batch is evaluated
    /// under one lock acquisition (this crate's stand-in for "BEGIN...
    /// COMMIT" — a real SQLite backend would use an actual DB
    /// transaction here instead), so a concurrent `append` from
    /// another task never observes a half-applied batch.
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        // §55: checked before touching any state at all — a whole
        // batch containing one oversized event is rejected outright,
        // never partially applied.
        for new_event in &request.events {
            validate_payload_size(new_event.event_type, new_event.payload.len())?;
        }

        let mut inner = self.inner.lock().expect("InMemoryEventStore lock poisoned");

        let current_version = inner
            .streams
            .get(&request.stream_id)
            .map(|events| events.len() as u64)
            .unwrap_or(0);
        if request.expected_version != current_version {
            return Err(EventStoreError::ConcurrencyConflict {
                stream_id: request.stream_id,
                expected_version: request.expected_version,
                actual_version: current_version,
            });
        }

        let mut new_version = current_version;
        let mut local_offsets = Vec::with_capacity(request.events.len());

        for new_event in request.events {
            // §24: a duplicate `EventId` is an idempotent no-op, not an
            // error — skip it without advancing the stream version or
            // assigning it an offset, but keep processing the rest of
            // the batch.
            if inner.seen_event_ids.contains(&new_event.event_id) {
                local_offsets.push(None);
                continue;
            }

            new_version += 1;
            let envelope = EventEnvelope {
                event_id: new_event.event_id,
                stream_id: request.stream_id,
                stream_version: new_version,
                event_type: new_event.event_type,
                schema_version: new_event.schema_version,
                created_at: new_event.created_at,
                origin: new_event.origin,
                correlation_id: new_event.correlation_id,
                causation_id: new_event.causation_id,
                payload: new_event.payload,
            };
            let offset = LocalLogOffset(inner.log.len() as u64 + 1);
            let stored = StoredEvent {
                envelope,
                local_offset: offset,
            };

            inner.seen_event_ids.insert(stored.envelope.event_id);
            inner.log.push(stored.clone());
            inner
                .streams
                .entry(request.stream_id)
                .or_default()
                .push(stored);
            local_offsets.push(Some(offset));
        }

        Ok(AppendResult {
            stream_id: request.stream_id,
            new_version,
            local_offsets,
        })
    }

    async fn read_stream(
        &self,
        stream: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let inner = self.inner.lock().expect("InMemoryEventStore lock poisoned");
        let events = inner.streams.get(&stream).cloned().unwrap_or_default();
        Ok(events
            .into_iter()
            .filter(|e| e.envelope.stream_version > from_version)
            .take(limit)
            .collect())
    }

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let inner = self.inner.lock().expect("InMemoryEventStore lock poisoned");
        Ok(inner
            .log
            .iter()
            .filter(|e| e.local_offset.0 > from_offset.0)
            .take(limit)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventTypeId, Timestamp};
    use crate::store::NewEvent;
    use siar_domain::DeviceId;

    fn new_event(event_id: EventId) -> NewEvent {
        NewEvent {
            event_id,
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp::now(),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: vec![1, 2, 3],
        }
    }

    #[tokio::test]
    async fn appending_to_a_new_stream_at_version_zero_succeeds() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();
        assert_eq!(result.new_version, 1);
        assert_eq!(result.local_offsets.len(), 1);
        assert!(result.local_offsets[0].is_some());
    }

    #[tokio::test]
    async fn a_stale_expected_version_is_a_real_concurrency_conflict() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();

        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await;
        assert_eq!(
            result,
            Err(EventStoreError::ConcurrencyConflict {
                stream_id: stream,
                expected_version: 0,
                actual_version: 1
            })
        );
    }

    #[tokio::test]
    async fn appending_a_previously_seen_event_id_is_an_idempotent_no_op() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        let event_id = EventId::new();

        let first = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(event_id)],
            })
            .await
            .unwrap();
        assert_eq!(first.new_version, 1);

        // Same event_id, but the caller (correctly) still supplies the
        // stream's now-current expected_version — a retried command
        // after a crash before the ack, not a fresh append attempt.
        let second = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 1,
                events: vec![new_event(event_id)],
            })
            .await
            .unwrap();
        assert_eq!(second.new_version, 1); // unchanged — nothing new was appended
        assert_eq!(second.local_offsets, vec![None]);

        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1); // still just the one real event
    }

    #[tokio::test]
    async fn a_batch_append_is_all_or_nothing_on_concurrency_conflict() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 5,
                events: vec![new_event(EventId::new()), new_event(EventId::new())],
            })
            .await;
        assert!(result.is_err());
        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(events.is_empty()); // nothing partially applied
    }

    #[tokio::test]
    async fn read_stream_only_returns_events_after_from_version() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![
                    new_event(EventId::new()),
                    new_event(EventId::new()),
                    new_event(EventId::new()),
                ],
            })
            .await
            .unwrap();

        let events = store.read_stream(stream, 1, 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].envelope.stream_version, 2);
    }

    #[tokio::test]
    async fn read_log_spans_multiple_streams_in_append_order() {
        let store = InMemoryEventStore::new();
        let stream_a = StreamId::from_name("conversation/a");
        let stream_b = StreamId::from_name("conversation/b");
        store
            .append(AppendRequest {
                stream_id: stream_a,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();
        store
            .append(AppendRequest {
                stream_id: stream_b,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();

        let events = store.read_log(LocalLogOffset(0), 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].envelope.stream_id, stream_a);
        assert_eq!(events[1].envelope.stream_id, stream_b);
    }

    #[tokio::test]
    async fn an_oversized_payload_is_rejected_before_touching_any_state() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        let mut oversized = new_event(EventId::new());
        oversized.payload = vec![0u8; crate::store::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];

        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![oversized],
            })
            .await;
        assert!(matches!(
            result,
            Err(EventStoreError::PayloadTooLarge { .. })
        ));

        // Nothing was appended — a batch that fails validation must
        // leave the stream exactly as it was before the attempt.
        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn an_oversized_event_anywhere_in_a_batch_rejects_the_whole_batch() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("conversation/abc");
        let mut oversized = new_event(EventId::new());
        oversized.payload = vec![0u8; crate::store::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];

        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                // A normal-sized event first, then the oversized one —
                // §11's own atomicity means even the normal one must
                // not land.
                events: vec![new_event(EventId::new()), oversized],
            })
            .await;
        assert!(result.is_err());

        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(events.is_empty());
    }
}
