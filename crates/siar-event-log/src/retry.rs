//! §12's own optimistic-concurrency contract, from a real caller's
//! side: [`EventStore::append`] rejects a stale `expected_version`
//! outright rather than retrying for the caller — deliberately, per
//! `store.rs`'s own doc comment on §21 keeping `append` itself simple.
//! A real caller overwhelmingly wants "make this event stick, whatever
//! the stream's current version turns out to be" rather than to
//! hand-roll the same retry loop at every call site — the first real
//! one is `siar_messaging::service`'s `record_messaging_event` (§33's
//! call sites, closing the "no real `append` caller anywhere" gap
//! named in this crate's own `lib.rs`).
//!
//! Safe to retry blindly with the SAME [`NewEvent`] (same `event_id`)
//! on a [`EventStoreError::ConcurrencyConflict`]: §11's own atomicity
//! guarantee is that a rejected append leaves no partial trace, so a
//! losing attempt never partially writes anything for `append`'s own
//! §24 idempotency check to later treat as "already seen" — retrying
//! the identical event is not a duplicate-append risk, it's the same
//! attempt with a corrected version guess.
//!
//! `max_attempts` exists so a truly pathological write storm on one
//! stream fails loudly (the last [`EventStoreError::ConcurrencyConflict`]
//! seen) instead of looping forever — this is a bound on retries
//! against a hostile/degenerate case, not a real backoff/scheduling
//! policy; a high-contention stream in practice should probably not be
//! sharing one [`crate::ids::StreamId`] at all (§15's own "not a job
//! queue" guidance, already quoted in `stoolap_store`'s doc comment).

use crate::ids::StreamId;
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError, NewEvent};

/// See this module's own doc comment.
pub async fn append_with_retry(
    store: &dyn EventStore,
    stream_id: StreamId,
    event: NewEvent,
    max_attempts: u32,
) -> Result<AppendResult, EventStoreError> {
    let mut expected_version = 0u64;
    let mut last_conflict = None;

    for _ in 0..max_attempts.max(1) {
        let request = AppendRequest {
            stream_id,
            expected_version,
            events: vec![event.clone()],
        };
        match store.append(request).await {
            Ok(result) => return Ok(result),
            Err(EventStoreError::ConcurrencyConflict {
                actual_version,
                stream_id,
                expected_version: attempted_version,
            }) => {
                last_conflict = Some(EventStoreError::ConcurrencyConflict {
                    stream_id,
                    expected_version: attempted_version,
                    actual_version,
                });
                expected_version = actual_version;
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_conflict.expect("loop runs at least once, so a conflict path sets this"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, StreamId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use siar_domain::DeviceId;

    fn event() -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp::now(),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: vec![],
        }
    }

    #[tokio::test]
    async fn a_brand_new_stream_succeeds_on_the_first_attempt() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("test");
        let result = append_with_retry(&store, stream, event(), 5).await.unwrap();
        assert_eq!(result.new_version, 1);
    }

    #[tokio::test]
    async fn a_stream_already_ahead_succeeds_after_one_retry() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("test");
        // Pre-populate the stream so its real version (2) doesn't match
        // this helper's initial guess (0) — the whole scenario this
        // helper exists for.
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event(), event()],
            })
            .await
            .unwrap();

        let result = append_with_retry(&store, stream, event(), 5).await.unwrap();
        assert_eq!(result.new_version, 3);
    }

    #[tokio::test]
    async fn exhausting_retries_returns_the_last_conflict_seen() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("test");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event()],
            })
            .await
            .unwrap();

        // max_attempts = 1: the first (and only) attempt still guesses
        // version 0, which is already stale — no retry budget left to
        // correct it.
        let result = append_with_retry(&store, stream, event(), 1).await;
        assert_eq!(
            result,
            Err(EventStoreError::ConcurrencyConflict {
                stream_id: stream,
                expected_version: 0,
                actual_version: 1,
            })
        );
    }

    #[tokio::test]
    async fn a_successful_append_does_not_burn_extra_attempts() {
        // `InMemoryEventStore` can't produce a `Backend`/`Corrupt`
        // error to exercise the immediate-return-on-other-errors
        // branch directly, but this at least confirms the common case
        // doesn't loop: one call, one event appended, even with a
        // generous `max_attempts`.
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("test");
        append_with_retry(&store, stream, event(), 10)
            .await
            .unwrap();
        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
    }
}
