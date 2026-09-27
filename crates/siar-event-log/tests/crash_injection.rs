//! `04-offline-event-log-architecture.md` §84 "Crash Injection Tests"
//! — the spec's own five injection points, each given a real test
//! proving deterministic recovery, using [`FaultInjectingEventStore`]
//! (this crate's own new fault-injection wrapper — see its own doc
//! comment) plus [`siar_startup_recovery::reconcile`] (§48, already
//! built) for the one point that's really about a missing work-table
//! entry rather than the event log itself.
//!
//! A real process can't be killed mid-`await` from inside its own
//! test suite in any portable way — these five tests instead inject
//! the SAME observable failure a real crash at each point would leave
//! behind (nothing written; a write whose response never arrived; a
//! write that landed with no corresponding follow-up work scheduled),
//! then verify a fresh, ordinary recovery path (retry, replay,
//! reconcile) converges to the single correct outcome regardless.

use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventTypeId, FaultInjectingEventStore,
    FaultMode, InMemoryCheckpointStore, InMemoryEventStore, NewEvent, Projection, ProjectionError,
    ProjectionId, ProjectionRunner, StoredEvent, StreamId, Timestamp,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
        ProjectionId("crash_injection_counter")
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

/// §84, point 1: "before append." The simplest of the five — nothing
/// happened at all, so recovery is simply "there is nothing to
/// recover": the stream stays exactly as it was before the attempt.
#[tokio::test]
async fn crash_before_append_leaves_nothing_to_recover() {
    let inner = InMemoryEventStore::new();
    let store = FaultInjectingEventStore::new(inner);
    store.arm(FaultMode::FailBeforeAppend);
    let stream = StreamId::from_name("crash/before-append");

    let result = store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![new_event()],
        })
        .await;
    assert!(result.is_err());
    // The real, wrapped store's own `append` was never even called —
    // this failure happened entirely inside the wrapper.
    assert!(!store.inner_append_was_called());
    assert!(store.read_stream(stream, 0, 10).await.unwrap().is_empty());
}

/// §84, point 2: "inside transaction." A batch with an oversized
/// event partway through fails atomically — this is §83's own
/// invariant 4, proven there as a property across every position; this
/// test names it explicitly under the crash-injection framing §84
/// asks for, using a real (not simulated) mid-batch failure: this
/// isn't `FaultInjectingEventStore` at all, because
/// `InMemoryEventStore` itself already refuses the batch atomically —
/// nothing needed to inject.
#[tokio::test]
async fn crash_inside_a_multi_event_transaction_lands_nothing_partial() {
    let store = InMemoryEventStore::new();
    let stream = StreamId::from_name("crash/inside-transaction");
    let mut events = vec![new_event(), new_event(), new_event()];
    let mut oversized = new_event();
    oversized.payload = vec![0u8; siar_event_log::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];
    events[1] = oversized;

    let result = store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events,
        })
        .await;
    assert!(result.is_err());
    // Not even the first (valid) event in the batch landed.
    assert!(store.read_stream(stream, 0, 10).await.unwrap().is_empty());
}

/// §84, point 3: "after event before projection." The event is
/// genuinely, durably appended — that part already succeeded — but
/// this simulates the process dying before `catch_up` ever ran.
/// "Restart" here is just calling `catch_up` for the first time,
/// fresh: recovery is a normal replay, not a special code path, and it
/// reaches the fully correct count regardless of how long the gap
/// between append and catch-up was.
#[tokio::test]
async fn crash_after_event_before_projection_recovers_by_ordinary_replay() {
    let store = InMemoryEventStore::new();
    let stream = StreamId::from_name("crash/before-projection");
    store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![new_event(), new_event(), new_event()],
        })
        .await
        .unwrap();
    // No catch_up call here at all — standing in for the crash: three
    // real, durable events with zero projection work done on them yet.

    // "Restart": a fresh checkpoint store and projection, exactly as
    // if this were a brand-new process that had never run before.
    let checkpoints = InMemoryCheckpointStore::new();
    let projection = CountingProjection::new();
    let runner = ProjectionRunner::new(&store, &checkpoints);
    runner.catch_up(&projection, 100).await.unwrap();

    assert_eq!(projection.count(), 3);
}

/// §84, point 4: "after projection before network effect." The event
/// is appended AND the projection caught up on it — both real,
/// durable facts — but the follow-up work a real caller would
/// schedule as a consequence (§30 "Effect Processing": `MessageQueued
/// → send effect`, in this test's own stand-in vocabulary "work
/// needed → work done") never happened, and the optimized work table
/// that would normally track that is empty/lost, exactly the §48
/// scenario. Recovery is [`siar_startup_recovery::reconcile`] — not a
/// special crash-handling path, the same reconciliation this crate
/// already has for an ordinary missing/corrupt work table.
#[tokio::test]
async fn crash_after_projection_before_network_effect_is_caught_by_reconciliation() {
    const WORK_NEEDED: EventTypeId = EventTypeId(100);
    const WORK_DONE: EventTypeId = EventTypeId(101);

    let store = InMemoryEventStore::new();
    let stream = StreamId::from_name("crash/before-network-effect");
    let mut work_needed = new_event();
    work_needed.event_type = WORK_NEEDED;
    let work_item_id = work_needed.event_id;
    store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![work_needed],
        })
        .await
        .unwrap();
    // No corresponding WORK_DONE event — standing in for the crash:
    // the network effect this event called for never happened, and
    // (also simulating loss/corruption) the real work table that
    // would normally still remember this is empty below.

    let history = store
        .read_log(siar_event_log::LocalLogOffset(0), 100)
        .await
        .unwrap();
    let opens = |e: &siar_event_log::StoredEvent| {
        (e.envelope.event_type == WORK_NEEDED).then_some(e.envelope.event_id)
    };
    let closes = |e: &siar_event_log::StoredEvent| {
        (e.envelope.event_type == WORK_DONE)
            .then_some(())
            .and_then(|_| e.envelope.causation_id)
    };
    let current_work_table = std::collections::HashSet::new(); // lost/corrupt
    let report = siar_startup_recovery::reconcile(history, opens, closes, &current_work_table);

    assert_eq!(report.missing, vec![work_item_id]);
    assert!(report.stale.is_empty());
}

/// §84, point 5: "after network effect before success marker." The
/// most subtle of the five: [`FaultInjectingEventStore::arm`]'s
/// [`FaultMode::SucceedInnerButReportFailure`] makes the real,
/// wrapped store genuinely, durably append the event — standing in
/// for the network effect having ALSO already fully happened — but
/// the caller is told the whole operation failed, standing in for the
/// process dying before any separate "it's done" marker got written.
/// A real caller in that position has no way to know it actually
/// succeeded, and — not knowing any better — retries with the SAME
/// event, the standard safe behavior for anything that might not have
/// gone through. §24's own idempotency is what makes that retry safe:
/// the retried append reports the same version, nothing duplicated.
#[tokio::test]
async fn crash_after_the_network_effect_before_a_success_marker_makes_retry_safe() {
    let inner = InMemoryEventStore::new();
    let store = FaultInjectingEventStore::new(inner);
    let stream = StreamId::from_name("crash/before-success-marker");
    let event = new_event();
    let event_id = event.event_id;

    store.arm(FaultMode::SucceedInnerButReportFailure);
    let first_attempt = store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![event.clone()],
        })
        .await;
    // The caller is told this failed...
    assert!(first_attempt.is_err());
    // ...even though the real write actually happened.
    assert!(store.inner_append_was_called());
    assert_eq!(store.read_stream(stream, 0, 10).await.unwrap().len(), 1);

    // Recovery: a real caller, not knowing better, retries the exact
    // same event.
    let retry = store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 1,
            events: vec![event],
        })
        .await
        .unwrap();

    // §24: the retry is a real, observable no-op — nothing duplicated,
    // and the caller can tell (`local_offsets` is `None`) that this
    // was already done rather than newly done.
    assert_eq!(retry.local_offsets, vec![None]);
    assert_eq!(retry.new_version, 1);
    let stored = store.read_stream(stream, 0, 10).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].envelope.event_id, event_id);
}
