//! `04-offline-event-log-architecture.md` §69 "Read-Only Recovery
//! Mode," verbatim: "if the database is damaged, read-only mode may
//! allow viewing / export / diagnostics without risking additional
//! corruption." Before this, per `ROADMAP.md`'s own §69 row, a
//! `Corrupt` error "just returns `Err`; no read-only fallback mode."
//! [`ReadOnlyEventStore`] is that fallback mode — the same
//! wrapper-around-any-real-[`EventStore`] shape
//! [`crate::projection::ReplayMode`]'s sibling crate
//! (`siar-event-notify`'s `NotifyingEventStore`) already uses for a
//! different cross-cutting concern.
//!
//! ## What "damaged" means here, and what doesn't change
//!
//! This module doesn't detect damage — it's not itself a corruption
//! checker, and nothing here decides WHEN to switch a real backend
//! into this mode. §92 Phase 2's `stoolap_store`'s own on-read
//! checksum verification (§22) is what actually PRODUCES
//! [`EventStoreError::Corrupt`] today; the real caller who catches
//! that, decides recovery is warranted, and wraps its store in a
//! [`ReadOnlyEventStore`] from that point on is still open — same
//! half-closed shape every round in this series has been honest
//! about. What this DOES give that caller: a real, tested type that
//! makes "still readable, no longer writable" an enforced property
//! rather than a convention every call site has to remember on its
//! own.
//!
//! `read_stream`/`read_log` — §69's own "viewing/export/diagnostics" —
//! pass straight through, unrestricted; only `append` is blocked, with
//! [`EventStoreError::ReadOnly`] returned before the wrapped store is
//! ever touched, so "no risk of additional corruption" is real: a
//! wrapped store's own `append` is provably never called through this
//! type.

use crate::ids::{LocalLogOffset, StreamId};
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent};
use async_trait::async_trait;

/// See this module's own doc comment.
pub struct ReadOnlyEventStore<S: EventStore> {
    inner: S,
}

impl<S: EventStore> ReadOnlyEventStore<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// Hands back the wrapped store — for a real caller whose damage
    /// turned out to be transient (a remounted disk, a completed
    /// repair) and wants to resume writing, without this module
    /// needing its own "make it writable again" method: unwrapping is
    /// just taking the value back out.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

#[async_trait]
impl<S: EventStore> EventStore for ReadOnlyEventStore<S> {
    /// Always [`EventStoreError::ReadOnly`] — the wrapped store's own
    /// `append` is never called, which is the whole point (see this
    /// module's own doc comment on why that matters for "no risk of
    /// additional corruption").
    async fn append(&self, _request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        Err(EventStoreError::ReadOnly)
    }

    async fn read_stream(
        &self,
        stream: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.inner.read_stream(stream, from_version, limit).await
    }

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.inner.read_log(from_offset, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::NewEvent;

    fn new_event() -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp::now(),
            origin: EventOrigin::System,
            correlation_id: None,
            causation_id: None,
            payload: vec![1, 2, 3],
        }
    }

    #[tokio::test]
    async fn append_is_always_refused_even_on_a_brand_new_stream() {
        let store = ReadOnlyEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/read-only/a");
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event()],
            })
            .await;
        assert_eq!(result, Err(EventStoreError::ReadOnly));
    }

    #[tokio::test]
    async fn reads_still_work_for_data_that_existed_before_wrapping() {
        let inner = InMemoryEventStore::new();
        let stream = StreamId::from_name("test/read-only/b");
        inner
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(), new_event()],
            })
            .await
            .unwrap();

        let store = ReadOnlyEventStore::new(inner);
        let events = store.read_stream(stream, 0, 100).await.unwrap();
        assert_eq!(events.len(), 2);
        let log = store.read_log(LocalLogOffset(0), 100).await.unwrap();
        assert_eq!(log.len(), 2);

        // Still refused, even though the underlying store is
        // perfectly writable — the wrapper itself is the authority
        // here, not the wrapped store's own real capability.
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 2,
                events: vec![new_event()],
            })
            .await;
        assert_eq!(result, Err(EventStoreError::ReadOnly));
        assert_eq!(store.read_stream(stream, 0, 100).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn into_inner_hands_back_a_fully_writable_store() {
        let store = ReadOnlyEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/read-only/c");
        let inner = store.into_inner();
        inner
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event()],
            })
            .await
            .unwrap();
    }
}
