//! `04-offline-event-log-architecture.md` §83 "Property Tests" —
//! upgrading, for the invariants below, from the single-scenario
//! example tests already scattered across this crate's own unit test
//! modules (real, but each checking exactly one hand-picked case) to
//! arbitrary-case generation via `proptest`. `ROADMAP.md`'s own §83
//! row said as much: "several of the section's own listed invariants
//! ARE covered — but only by targeted example tests... never by a
//! property/fuzz framework generating arbitrary cases."
//!
//! This file covers five of the spec's own six listed invariants,
//! against this crate's real, public API — [`InMemoryEventStore`],
//! [`ProjectionRunner`], [`InMemoryCheckpointStore`] — not a mock of
//! any of them. The sixth, "expired pending work is not resurrected,"
//! isn't an [`EventStore`]/[`ProjectionRunner`] property at all — it
//! belongs to `siar-startup-recovery::reconciliation`'s own pending-
//! work model instead, and has its own property test there.
//!
//! Each property test below builds its own `tokio::runtime::Runtime`
//! per generated case and does all the real async work — appends,
//! catch-ups, reads — inside a `block_on`, returning a plain value out
//! of it; every `prop_assert!`/`prop_assert_eq!` then runs in the
//! ordinary synchronous test body `proptest!` itself expects.
//! `proptest!`'s own macro has no support for an async test body, and
//! keeping this split (real work returns a value; assertions happen
//! outside) sidesteps needing one.

use proptest::prelude::*;
use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventTypeId, InMemoryCheckpointStore,
    InMemoryEventStore, NewEvent, Projection, ProjectionCheckpointStore, ProjectionError,
    ProjectionId, ProjectionRunner, StoredEvent, StreamId, Timestamp,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("tokio runtime")
}

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

/// A minimal real [`Projection`] — counts every event it's given,
/// `reset`-able like any real one must be. Shared via `Arc` so a test
/// can read the count after handing the projection to
/// [`ProjectionRunner`] by reference.
struct CountingProjection {
    count: Arc<AtomicU64>,
}

impl CountingProjection {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn count(&self) -> u64 {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl Projection for CountingProjection {
    fn projection_id(&self) -> ProjectionId {
        ProjectionId("property_test_counter")
    }

    fn projection_version(&self) -> u16 {
        1
    }

    async fn apply(&self, _event: &StoredEvent) -> Result<(), ProjectionError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn reset(&self) -> Result<(), ProjectionError> {
        self.count.store(0, Ordering::SeqCst);
        Ok(())
    }
}

proptest! {
    /// §83, invariant 1: "duplicate event does not duplicate
    /// projection state." However many times (1 to 8) the exact same
    /// `NewEvent` (same `event_id`) is appended, only the first ever
    /// lands — every replay after that is a §24 idempotent no-op — so
    /// a projection built from this stream only ever sees it once.
    #[test]
    fn duplicate_appends_never_duplicate_projection_state(repeat_count in 1u32..8) {
        let applied = rt().block_on(async {
            let store = InMemoryEventStore::new();
            let checkpoints = InMemoryCheckpointStore::new();
            let stream = StreamId::from_name("property/duplicate");
            let event = new_event();

            for attempt in 0..repeat_count {
                // Every attempt after the first hits a real §24
                // duplicate; `expected_version` must match whatever
                // the store's real, current version is, which is 1
                // after the very first successful append and stays 1
                // for every duplicate after that (a duplicate-only
                // batch never advances the version).
                let expected_version = if attempt == 0 { 0 } else { 1 };
                store
                    .append(AppendRequest {
                        stream_id: stream,
                        expected_version,
                        events: vec![event.clone()],
                    })
                    .await
                    .unwrap();
            }

            let projection = CountingProjection::new();
            let runner = ProjectionRunner::new(&store, &checkpoints);
            runner.catch_up(&projection, 100).await.unwrap();
            projection.count()
        });

        prop_assert_eq!(applied, 1);
    }

    /// §83, invariant 2: "projection rebuild equals live projection."
    /// For any way of splitting events into per-append batches
    /// (`batch_sizes`), applying them incrementally (a `catch_up`
    /// after every append, the "live" path) ends at the same count as
    /// tearing that projection down and rebuilding it from scratch in
    /// one pass over the same stream (the "rebuild" path).
    #[test]
    fn projection_rebuild_equals_live_projection(
        batch_sizes in prop::collection::vec(1usize..5, 1..6)
    ) {
        let (live_count, rebuild_count, total_events) = rt().block_on(async {
            let store = InMemoryEventStore::new();
            let live_checkpoints = InMemoryCheckpointStore::new();
            let live_projection = CountingProjection::new();
            let live_runner = ProjectionRunner::new(&store, &live_checkpoints);

            let stream = StreamId::from_name("property/rebuild");
            let mut expected_version = 0u64;
            let mut total_events = 0u64;
            for batch_size in &batch_sizes {
                let events: Vec<_> = (0..*batch_size).map(|_| new_event()).collect();
                total_events += events.len() as u64;
                store
                    .append(AppendRequest {
                        stream_id: stream,
                        expected_version,
                        events,
                    })
                    .await
                    .unwrap();
                expected_version += *batch_size as u64;
                // The "live" path: catch up after every single append,
                // the way a real running process would.
                live_runner.catch_up(&live_projection, 100).await.unwrap();
            }

            // The "rebuild" path: a fresh projection, fresh checkpoint
            // store, one single catch_up over everything already in
            // the log.
            let rebuild_checkpoints = InMemoryCheckpointStore::new();
            let rebuild_projection = CountingProjection::new();
            let rebuild_runner = ProjectionRunner::new(&store, &rebuild_checkpoints);
            rebuild_runner.catch_up(&rebuild_projection, 1000).await.unwrap();

            (live_projection.count(), rebuild_projection.count(), total_events)
        });

        prop_assert_eq!(live_count, total_events);
        prop_assert_eq!(rebuild_count, total_events);
        prop_assert_eq!(live_count, rebuild_count);
    }

    /// §83, invariant 3: "stream versions strictly increase." For any
    /// way of splitting events into append batches, the
    /// `stream_version` recorded on every one of them, read back in
    /// order, is exactly `1, 2, 3, ..., N` — no gaps, no repeats,
    /// regardless of batch boundaries.
    #[test]
    fn stream_versions_strictly_increase_regardless_of_batching(
        batch_sizes in prop::collection::vec(1usize..5, 1..6)
    ) {
        let (versions, expected) = rt().block_on(async {
            let store = InMemoryEventStore::new();
            let stream = StreamId::from_name("property/versions");
            let mut expected_version = 0u64;
            for batch_size in &batch_sizes {
                let events: Vec<_> = (0..*batch_size).map(|_| new_event()).collect();
                store
                    .append(AppendRequest {
                        stream_id: stream,
                        expected_version,
                        events,
                    })
                    .await
                    .unwrap();
                expected_version += *batch_size as u64;
            }

            let stored = store.read_stream(stream, 0, 10_000).await.unwrap();
            let versions: Vec<u64> = stored.iter().map(|e| e.envelope.stream_version).collect();
            let expected: Vec<u64> = (1..=expected_version).collect();
            (versions, expected)
        });

        prop_assert_eq!(versions, expected);
    }

    /// §83, invariant 4: "failed transaction creates no partial
    /// event." A batch with an oversized event planted at ANY
    /// position (not just the end) fails as a whole — nothing before
    /// it in the same batch lands either, whatever `position` is.
    #[test]
    fn a_batch_with_an_oversized_event_anywhere_lands_nothing(position in 0usize..4) {
        let (append_failed, stored_is_empty) = rt().block_on(async {
            let store = InMemoryEventStore::new();
            let stream = StreamId::from_name("property/atomicity");
            let mut events: Vec<_> = (0..4).map(|_| new_event()).collect();
            let mut oversized = new_event();
            oversized.payload = vec![0u8; siar_event_log::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];
            events[position] = oversized;

            let result = store
                .append(AppendRequest {
                    stream_id: stream,
                    expected_version: 0,
                    events,
                })
                .await;
            let append_failed = result.is_err();

            let stored = store.read_stream(stream, 0, 10_000).await.unwrap();
            (append_failed, stored.is_empty())
        });

        prop_assert!(append_failed);
        prop_assert!(stored_is_empty);
    }

    /// §83, invariant 5: "checkpoint never advances past committed
    /// projection." For any batch size used to drive `catch_up`
    /// (forcing it to stop and save a checkpoint partway through a
    /// longer stream), the saved checkpoint's own `last_log_offset`
    /// never exceeds the number of events the projection has actually
    /// been given via `apply` at that same point.
    #[test]
    fn checkpoint_never_advances_past_what_the_projection_actually_applied(
        total_events in 1u64..20,
        catch_up_batch_size in 1usize..6,
    ) {
        let (checkpoint_offset, applied) = rt().block_on(async {
            let store = InMemoryEventStore::new();
            let checkpoints = InMemoryCheckpointStore::new();
            let stream = StreamId::from_name("property/checkpoint");
            let events: Vec<_> = (0..total_events).map(|_| new_event()).collect();
            store
                .append(AppendRequest {
                    stream_id: stream,
                    expected_version: 0,
                    events,
                })
                .await
                .unwrap();

            let projection = CountingProjection::new();
            let runner = ProjectionRunner::new(&store, &checkpoints);
            runner.catch_up(&projection, catch_up_batch_size).await.unwrap();

            let checkpoint = checkpoints
                .load(ProjectionId("property_test_counter"))
                .await
                .unwrap();
            let applied = projection.count();
            let checkpoint_offset = checkpoint.map(|c| c.last_log_offset.0);
            (checkpoint_offset, applied)
        });

        // The log in this test is a single global stream, so
        // `last_log_offset` can't legitimately exceed how many events
        // this projection has actually processed.
        if let Some(offset) = checkpoint_offset {
            prop_assert!(offset <= applied);
        }
        prop_assert_eq!(applied, total_events);
    }
}
