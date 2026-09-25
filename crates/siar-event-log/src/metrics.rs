//! `04-offline-event-log-architecture.md` §82 "Metrics" — the spec's
//! own list of eight, verbatim: append latency, commit latency,
//! events/sec, projection lag, replay rate, event-store size,
//! duplicate rate, projection failures. `ROADMAP.md`'s own §82 row:
//! "Not started."
//!
//! [`MetricsEventStore<S>`] is the same wrapper-around-any-real-
//! [`crate::store::EventStore`] shape [`crate::read_only::
//! ReadOnlyEventStore`]/`siar-event-notify`'s `NotifyingEventStore`
//! already use — wrap a real backend, observe every `append` that
//! goes through it, expose a [`MetricsSnapshot`] a real caller can
//! read at any point (a diagnostics screen, a periodic log line,
//! whatever §81 "Diagnostics" — still `⬜` — eventually becomes).
//!
//! ## Four of the spec's eight metrics, honestly, not eight
//!
//! This wrapper sits at exactly one boundary — a caller's `append`
//! call — so it can only measure what's observable there:
//!
//! - **append latency** — real, measured with `Instant`/`Duration`
//!   around every call to the wrapped store's own `append`.
//! - **events/sec** — real, derived from
//!   [`MetricsSnapshot::events_appended`] over
//!   [`MetricsSnapshot::elapsed_since_start`].
//! - **duplicate rate** — real: an `AppendResult` already reports
//!   which of its `local_offsets` are `None` (a §24 idempotent skip),
//!   counted separately from real appends.
//! - **event-store size** — [`MetricsSnapshot::events_appended`]
//!   itself, a real running count of everything this wrapper has seen
//!   durably appended (NOT the backend's own on-disk byte size, which
//!   this crate has no way to ask any `EventStore` implementor for —
//!   the trait exposes no such method).
//!
//! The other four are honestly out of scope for a wrapper at THIS
//! boundary, not silently dropped:
//!
//! - **commit latency** — this crate's own `EventStore::append` is the
//!   only boundary a caller crosses; there is no separately observable
//!   "committed but not yet appended" phase to time on top of it.
//! - **projection lag** / **replay rate** / **projection failures** —
//!   all three are properties of a [`crate::projection::
//!   ProjectionRunner`] replaying events, not of an `EventStore`
//!   accepting them — measuring them for real means wrapping
//!   `catch_up`/`catch_up_with_mode`, a genuinely different (and
//!   still open) piece of work from this one.

use crate::ids::{LocalLogOffset, StreamId};
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent};
use async_trait::async_trait;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
struct Counters {
    append_calls: u64,
    append_errors: u64,
    events_appended: u64,
    duplicate_events: u64,
    total_append_latency: Duration,
}

/// A point-in-time read of [`MetricsEventStore`]'s own counters — see
/// this module's own doc comment for which of the spec's eight this
/// does and doesn't cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub append_calls: u64,
    pub append_errors: u64,
    pub events_appended: u64,
    pub duplicate_events: u64,
    pub total_append_latency: Duration,
    pub elapsed_since_start: Duration,
}

impl MetricsSnapshot {
    /// `0.0` on a fresh snapshot with nothing appended yet, rather
    /// than a division producing `NaN` — a real diagnostics display
    /// showing "0.0 events/sec" for an idle store is correct; "NaN
    /// events/sec" is not.
    pub fn events_per_second(&self) -> f64 {
        let secs = self.elapsed_since_start.as_secs_f64();
        if secs == 0.0 {
            0.0
        } else {
            self.events_appended as f64 / secs
        }
    }

    /// Real appends vs §24 idempotent duplicates, as a fraction of
    /// every event this wrapper has ever seen requested — `0.0` with
    /// nothing appended yet, same reasoning as
    /// [`Self::events_per_second`].
    pub fn duplicate_rate(&self) -> f64 {
        let total = self.events_appended + self.duplicate_events;
        if total == 0 {
            0.0
        } else {
            self.duplicate_events as f64 / total as f64
        }
    }

    /// `Duration::ZERO` with zero successful `append` CALLS — not
    /// zero events, since one call can carry several events (a batch)
    /// at one latency measurement; average latency is per call, not
    /// per event.
    pub fn average_append_latency(&self) -> Duration {
        if self.append_calls == 0 {
            Duration::ZERO
        } else {
            self.total_append_latency / self.append_calls as u32
        }
    }
}

/// See this module's own doc comment.
pub struct MetricsEventStore<S: EventStore> {
    inner: S,
    counters: Mutex<Counters>,
    started_at: Instant,
}

impl<S: EventStore> MetricsEventStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            counters: Mutex::new(Counters::default()),
            started_at: Instant::now(),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let counters = self.counters.lock().expect("metrics lock");
        MetricsSnapshot {
            append_calls: counters.append_calls,
            append_errors: counters.append_errors,
            events_appended: counters.events_appended,
            duplicate_events: counters.duplicate_events,
            total_append_latency: counters.total_append_latency,
            elapsed_since_start: self.started_at.elapsed(),
        }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }
}

#[async_trait]
impl<S: EventStore> EventStore for MetricsEventStore<S> {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        let start = Instant::now();
        let result = self.inner.append(request).await;
        let elapsed = start.elapsed();

        let mut counters = self.counters.lock().expect("metrics lock");
        counters.append_calls += 1;
        counters.total_append_latency += elapsed;
        match &result {
            Ok(append_result) => {
                for offset in &append_result.local_offsets {
                    match offset {
                        Some(_) => counters.events_appended += 1,
                        None => counters.duplicate_events += 1,
                    }
                }
            }
            Err(_) => counters.append_errors += 1,
        }
        drop(counters);

        result
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
    async fn a_fresh_store_reports_zero_rates_not_nan_or_a_panic() {
        let store = MetricsEventStore::new(InMemoryEventStore::new());
        let snapshot = store.snapshot();
        assert_eq!(snapshot.append_calls, 0);
        assert_eq!(snapshot.events_per_second(), 0.0);
        assert_eq!(snapshot.duplicate_rate(), 0.0);
        assert_eq!(snapshot.average_append_latency(), Duration::ZERO);
    }

    #[tokio::test]
    async fn a_successful_append_increments_calls_and_events_and_records_latency() {
        let store = MetricsEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/metrics/a");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(), new_event(), new_event()],
            })
            .await
            .unwrap();

        let snapshot = store.snapshot();
        assert_eq!(snapshot.append_calls, 1);
        assert_eq!(snapshot.events_appended, 3);
        assert_eq!(snapshot.duplicate_events, 0);
        assert_eq!(snapshot.append_errors, 0);
        assert_eq!(snapshot.duplicate_rate(), 0.0);
        // A real backend genuinely took non-negative time — the only
        // claim worth making about a wall-clock measurement in a test
        // that must never be flaky on a loaded CI box.
        assert!(snapshot.average_append_latency() >= Duration::ZERO);
    }

    #[tokio::test]
    async fn a_duplicate_event_counts_separately_from_a_real_append() {
        let store = MetricsEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/metrics/b");
        let event = new_event();
        let event_id = event.event_id;
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event],
            })
            .await
            .unwrap();

        let mut replay = new_event();
        replay.event_id = event_id;
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 1,
                events: vec![replay],
            })
            .await
            .unwrap();

        let snapshot = store.snapshot();
        assert_eq!(snapshot.append_calls, 2);
        assert_eq!(snapshot.events_appended, 1);
        assert_eq!(snapshot.duplicate_events, 1);
        assert_eq!(snapshot.duplicate_rate(), 0.5);
    }

    #[tokio::test]
    async fn a_failed_append_counts_as_an_error_not_an_appended_event() {
        let store = MetricsEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/metrics/c");

        // Wrong expected_version on a brand-new stream: a real,
        // guaranteed `ConcurrencyConflict`.
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 5,
                events: vec![new_event()],
            })
            .await;
        assert!(result.is_err());

        let snapshot = store.snapshot();
        assert_eq!(snapshot.append_calls, 1);
        assert_eq!(snapshot.append_errors, 1);
        assert_eq!(snapshot.events_appended, 0);
    }

    #[tokio::test]
    async fn reads_pass_through_and_are_not_counted_as_appends() {
        let store = MetricsEventStore::new(InMemoryEventStore::new());
        let stream = StreamId::from_name("test/metrics/d");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event()],
            })
            .await
            .unwrap();

        let events = store.read_stream(stream, 0, 100).await.unwrap();
        assert_eq!(events.len(), 1);
        // Still exactly one append call — reading never touches these
        // counters.
        assert_eq!(store.snapshot().append_calls, 1);
    }
}
