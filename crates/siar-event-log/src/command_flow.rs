//! `04-offline-event-log-architecture.md` §13 "Local-First Command
//! Flow", §29 "Pure Decision Functions", and the "events schedule
//! effects" half of §30 — §92 Phase 5.
//!
//! §13's correct order, verbatim:
//!
//! ```text
//! User action → validate → append event → COMMIT
//!             → update projection → show UI → network effect later
//! ```
//!
//! and its stated anti-pattern: "try network, persist only after
//! success." [`CommandFlow`] makes the right order the *only* order
//! available through this API:
//!
//! 1. **validate / decide** — [`Decider::decide`], a pure function
//!    (§29). It has no store, clock, or network handle; ids and
//!    timestamps arrive inside the command, so the same inputs always
//!    produce the same events and tests need no mocks.
//! 2. **plan** — [`EffectPlanner::plan`] maps each decided event to the
//!    external work it obliges (§30), again purely.
//! 3. **commit** — events and their outbox rows go through
//!    [`TransactionalOutbox::append_with_outbox`] as one transaction
//!    (§14). Nothing external has happened yet; if this step fails,
//!    there is nothing to undo and nothing was sent.
//! 4. **project** (optional) — [`CommandFlow::commit_and_project`]
//!    catches projections up *after* the commit, so the UI reads its own
//!    write (§18's "in practice" form — see `projection`'s own doc
//!    comment for what that does and doesn't guarantee). A projection
//!    failure does **not** fail the command: the events are durable, and
//!    the projection's checkpoint means the next catch-up resumes.
//! 5. **effects, later** — an [`crate::effects::EffectProcessor`] drains
//!    the outbox on its own schedule. Nothing in this module can invoke
//!    a handler; that separation is the whole point.
//!
//! ## `EventRef`: one view over "new" and "stored"
//!
//! Planning happens twice in a system's life: at command time, over
//! [`NewEvent`]s that don't have a stream version or log offset yet, and
//! at reconciliation time (§48, [`crate::outbox_reconcile`]), over
//! [`StoredEvent`]s read back from the log. [`EventRef`] is the common
//! borrowed view, so one [`EffectPlanner`] implementation serves both —
//! and, because [`OperationId`]s are derived from `(event id, kind,
//! slot)`, produces *identical* ids both times. That equality is what
//! lets a rebuilt outbox be indistinguishable from the original.

use thiserror::Error;

use crate::envelope::EventOrigin;
use crate::ids::{EventId, EventTypeId, StreamId, Timestamp};
use crate::outbox::{EffectKind, OperationId, OutboxEntry, OutboxError, TransactionalOutbox};
use crate::projection::{
    Projection, ProjectionError, ProjectionId, ProjectionRunner, DEFAULT_CATCH_UP_BATCH_SIZE,
};
use crate::store::{AppendRequest, AppendResult, EventStoreError, NewEvent, StoredEvent};

/// A borrowed, store-agnostic view of one event — see the module doc.
#[derive(Debug, Clone, Copy)]
pub struct EventRef<'a> {
    pub event_id: EventId,
    pub stream_id: StreamId,
    pub event_type: EventTypeId,
    pub schema_version: u16,
    pub created_at: Timestamp,
    pub origin: EventOrigin,
    pub causation_id: Option<EventId>,
    pub payload: &'a [u8],
}

impl<'a> EventRef<'a> {
    pub fn from_new(stream_id: StreamId, event: &'a NewEvent) -> Self {
        Self {
            event_id: event.event_id,
            stream_id,
            event_type: event.event_type,
            schema_version: event.schema_version,
            created_at: event.created_at,
            origin: event.origin,
            causation_id: event.causation_id,
            payload: &event.payload,
        }
    }

    pub fn from_stored(event: &'a StoredEvent) -> Self {
        let e = &event.envelope;
        Self {
            event_id: e.event_id,
            stream_id: e.stream_id,
            event_type: e.event_type,
            schema_version: e.schema_version,
            created_at: e.created_at,
            origin: e.origin,
            causation_id: e.causation_id,
            payload: &e.payload,
        }
    }
}

/// One obligation an event creates (§30).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectIntent {
    pub kind: EffectKind,
    /// Distinguishes several same-kind effects from one event; almost
    /// always `0`.
    pub slot: u16,
    /// A pointer to the work (ids, blob refs) — not bulk data (§15).
    pub payload: Vec<u8>,
}

/// §30's "events may schedule external work", as a pure mapping.
///
/// Must be **deterministic**: the same event must always yield the same
/// intents, or a reconciliation rebuild would disagree with what the
/// original command enqueued.
pub trait EffectPlanner: Send + Sync {
    /// External work `event` obliges. Most events oblige none.
    fn plan(&self, event: &EventRef<'_>) -> Vec<EffectIntent>;

    /// Operations whose obligation `event` *discharges* — e.g. a
    /// `MessageSent` event closes the send effect planned by the
    /// `MessageQueued` it was caused by. Used only by reconciliation
    /// (§48): history, not the work table, is the source of truth for
    /// "is this still owed?"
    fn closes(&self, _event: &EventRef<'_>) -> Vec<OperationId> {
        Vec::new()
    }
}

/// A planner for flows that schedule no external work.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEffects;

impl EffectPlanner for NoEffects {
    fn plan(&self, _event: &EventRef<'_>) -> Vec<EffectIntent> {
        Vec::new()
    }
}

/// §29, verbatim shape: `decide(state, command) -> Result<events, error>`.
///
/// Associated function, not a method: there is deliberately no `self`
/// to hang a store, clock, or RNG on. Ids and timestamps are carried in
/// the command.
pub trait Decider {
    type State;
    type Command;
    type Error;

    fn decide(state: &Self::State, command: Self::Command) -> Result<Vec<NewEvent>, Self::Error>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommitError {
    #[error(transparent)]
    Store(#[from] EventStoreError),
    /// An [`EffectIntent`] was invalid (oversized payload) — raised
    /// before anything was written.
    #[error(transparent)]
    Effect(#[from] OutboxError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError<E> {
    /// The pure decision rejected the command; nothing was written.
    #[error("command rejected: {0}")]
    Decide(E),
    #[error(transparent)]
    Commit(#[from] CommitError),
}

/// What a successful commit did.
#[derive(Debug)]
pub struct Committed {
    pub append: AppendResult,
    /// Outbox rows submitted with the events. Idempotent: rows already
    /// present (a resubmitted command) are counted here too but add
    /// nothing.
    pub effects_submitted: usize,
    /// Projections whose post-commit catch-up failed. The command still
    /// succeeded — see the module doc.
    pub projection_failures: Vec<(ProjectionId, ProjectionError)>,
}

pub struct CommandFlow<'a, S, P>
where
    S: TransactionalOutbox + ?Sized,
    P: EffectPlanner + ?Sized,
{
    store: &'a S,
    planner: &'a P,
}

impl<'a, S, P> CommandFlow<'a, S, P>
where
    S: TransactionalOutbox + ?Sized,
    P: EffectPlanner + ?Sized,
{
    pub fn new(store: &'a S, planner: &'a P) -> Self {
        Self { store, planner }
    }

    /// Steps 2-3: plan, then commit events + outbox rows atomically.
    pub async fn commit(
        &self,
        stream_id: StreamId,
        expected_version: u64,
        events: Vec<NewEvent>,
        now: Timestamp,
    ) -> Result<Committed, CommitError> {
        let mut entries = Vec::new();
        for event in &events {
            let view = EventRef::from_new(stream_id, event);
            for intent in self.planner.plan(&view) {
                entries.push(OutboxEntry::new(
                    event.event_id,
                    stream_id,
                    intent.kind,
                    intent.slot,
                    intent.payload,
                    now,
                )?);
            }
        }
        let effects_submitted = entries.len();
        let append = self
            .store
            .append_with_outbox(
                AppendRequest {
                    stream_id,
                    expected_version,
                    events,
                },
                entries,
            )
            .await?;
        Ok(Committed {
            append,
            effects_submitted,
            projection_failures: Vec::new(),
        })
    }

    /// Steps 2-4: [`Self::commit`], then catch every projection up.
    pub async fn commit_and_project(
        &self,
        stream_id: StreamId,
        expected_version: u64,
        events: Vec<NewEvent>,
        now: Timestamp,
        runner: &ProjectionRunner<'_>,
        projections: &[&dyn Projection],
    ) -> Result<Committed, CommitError> {
        let mut committed = self
            .commit(stream_id, expected_version, events, now)
            .await?;
        for projection in projections {
            if let Err(e) = runner
                .catch_up(*projection, DEFAULT_CATCH_UP_BATCH_SIZE)
                .await
            {
                committed
                    .projection_failures
                    .push((projection.projection_id(), e));
            }
        }
        Ok(committed)
    }

    /// Steps 1-3 for a [`Decider`]: decide purely against `state`,
    /// then commit. On a [`EventStoreError::ConcurrencyConflict`] the
    /// caller must reload state and decide *again* — retrying the same
    /// decision against stale state would be wrong (§12), which is why
    /// there is no retry loop here.
    pub async fn decide_and_commit<D: Decider>(
        &self,
        stream_id: StreamId,
        state: &D::State,
        expected_version: u64,
        command: D::Command,
        now: Timestamp,
    ) -> Result<Committed, CommandError<D::Error>> {
        let events = D::decide(state, command).map_err(CommandError::Decide)?;
        Ok(self
            .commit(stream_id, expected_version, events, now)
            .await?)
    }
}

/// Shared fixtures for this crate's own tests ([`crate::outbox_reconcile`]
/// reuses them): a two-event message flow where `MessageQueued`
/// obliges a send and `MessageSent` discharges it.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use crate::ids::CorrelationId;
    use siar_domain::DeviceId;

    pub(crate) const QUEUED: EventTypeId = EventTypeId(1);
    pub(crate) const SENT: EventTypeId = EventTypeId(2);
    pub(crate) const SEND_EFFECT: EffectKind = EffectKind(10);

    /// `MessageQueued` obliges a send; `MessageSent` (caused by it)
    /// discharges it.
    pub(crate) struct MessagePlanner;
    impl EffectPlanner for MessagePlanner {
        fn plan(&self, e: &EventRef<'_>) -> Vec<EffectIntent> {
            if e.event_type == QUEUED {
                vec![EffectIntent {
                    kind: SEND_EFFECT,
                    slot: 0,
                    payload: e.payload.to_vec(),
                }]
            } else {
                vec![]
            }
        }
        fn closes(&self, e: &EventRef<'_>) -> Vec<OperationId> {
            match (e.event_type == SENT, e.causation_id) {
                (true, Some(cause)) => vec![OperationId::derive(cause, SEND_EFFECT, 0)],
                _ => vec![],
            }
        }
    }

    pub(crate) fn event(ty: EventTypeId, payload: &[u8]) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(1),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: Some(CorrelationId::new()),
            causation_id: None,
            payload: payload.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::effects::{
        BackoffPolicy, EffectHandler, EffectOutcome, EffectProcessor, NoTransitions,
    };
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::{Outbox, OutboxState, MAX_OUTBOX_PAYLOAD_BYTES};
    use crate::projection::{InMemoryCheckpointStore, ProjectionRunner, ReplayMode};
    use crate::store::EventStore;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct CountingHandler(AtomicU64);
    impl EffectHandler for CountingHandler {
        async fn handle(&self, _e: &OutboxEntry) -> EffectOutcome {
            self.0.fetch_add(1, Ordering::SeqCst);
            EffectOutcome::Done
        }
    }

    #[tokio::test]
    async fn commit_lands_event_and_outbox_row_together_and_sends_nothing() {
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("conversation/a");
        let queued = event(QUEUED, b"hi");
        let id = OperationId::derive(queued.event_id, SEND_EFFECT, 0);

        let c = flow
            .commit(stream, 0, vec![queued], Timestamp(5))
            .await
            .unwrap();
        assert_eq!(c.append.new_version, 1);
        assert_eq!(c.effects_submitted, 1);

        let row = store.outbox().get(id).await.unwrap().unwrap();
        assert_eq!(row.state, OutboxState::Pending);
        assert_eq!(
            row.attempts, 0,
            "committed, but nothing attempted yet (§13)"
        );
        assert_eq!(row.payload, b"hi");
    }

    #[tokio::test]
    async fn a_failed_append_leaves_no_outbox_row() {
        // §13's anti-pattern in reverse: if the durable append is
        // refused, no effect may exist that could ever be sent.
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("conversation/a");
        flow.commit(stream, 0, vec![event(QUEUED, b"first")], Timestamp(0))
            .await
            .unwrap();

        let stale = event(QUEUED, b"second");
        let stale_id = OperationId::derive(stale.event_id, SEND_EFFECT, 0);
        let err = flow
            .commit(stream, 0, vec![stale], Timestamp(0))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CommitError::Store(EventStoreError::ConcurrencyConflict { .. })
        ));
        assert!(store.outbox().get(stale_id).await.unwrap().is_none());
        assert_eq!(store.outbox().counts().await.unwrap().pending, 1);
    }

    #[tokio::test]
    async fn an_invalid_effect_writes_no_events() {
        struct HugePlanner;
        impl EffectPlanner for HugePlanner {
            fn plan(&self, _e: &EventRef<'_>) -> Vec<EffectIntent> {
                vec![EffectIntent {
                    kind: SEND_EFFECT,
                    slot: 0,
                    payload: vec![0; MAX_OUTBOX_PAYLOAD_BYTES + 1],
                }]
            }
        }
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &HugePlanner);
        let stream = StreamId::from_name("s");
        let err = flow
            .commit(stream, 0, vec![event(QUEUED, b"x")], Timestamp(0))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            CommitError::Effect(OutboxError::PayloadTooLarge { .. })
        ));
        assert!(store
            .read_log(crate::ids::LocalLogOffset(0), 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn resubmitting_the_same_command_is_idempotent_for_log_and_outbox() {
        // Crash-and-retry: the same NewEvent (same EventId) goes in
        // again. §24 dedups the event; the derived OperationId dedups
        // the outbox row (§31).
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("s");
        let queued = event(QUEUED, b"hi");

        flow.commit(stream, 0, vec![queued.clone()], Timestamp(0))
            .await
            .unwrap();
        // Same stream version guess is now stale; a real retry reloads it.
        let again = flow
            .commit(stream, 1, vec![queued], Timestamp(99))
            .await
            .unwrap();
        assert_eq!(again.append.local_offsets, vec![None], "§24 duplicate");
        assert_eq!(
            store
                .read_log(crate::ids::LocalLogOffset(0), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(store.outbox().counts().await.unwrap().pending, 1);
    }

    struct Rename;
    impl Decider for Rename {
        type State = bool; // "already renamed"
        type Command = NewEvent;
        type Error = &'static str;
        fn decide(state: &bool, command: NewEvent) -> Result<Vec<NewEvent>, Self::Error> {
            if *state {
                Err("already renamed")
            } else {
                Ok(vec![command])
            }
        }
    }

    #[tokio::test]
    async fn a_rejected_decision_writes_nothing() {
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("s");

        let err = flow
            .decide_and_commit::<Rename>(stream, &true, 0, event(QUEUED, b"x"), Timestamp(0))
            .await
            .unwrap_err();
        assert_eq!(err, CommandError::Decide("already renamed"));
        assert!(store
            .read_log(crate::ids::LocalLogOffset(0), 10)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(store.outbox().counts().await.unwrap().pending, 0);

        flow.decide_and_commit::<Rename>(stream, &false, 0, event(QUEUED, b"x"), Timestamp(0))
            .await
            .unwrap();
        assert_eq!(store.outbox().counts().await.unwrap().pending, 1);
    }

    struct CountProjection(AtomicU64);
    #[async_trait]
    impl Projection for CountProjection {
        fn projection_id(&self) -> ProjectionId {
            ProjectionId("count")
        }
        fn projection_version(&self) -> u16 {
            1
        }
        async fn apply(&self, _e: &StoredEvent) -> Result<(), ProjectionError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn reset(&self) -> Result<(), ProjectionError> {
            self.0.store(0, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingProjection;
    #[async_trait]
    impl Projection for FailingProjection {
        fn projection_id(&self) -> ProjectionId {
            ProjectionId("failing")
        }
        fn projection_version(&self) -> u16 {
            1
        }
        async fn apply(&self, _e: &StoredEvent) -> Result<(), ProjectionError> {
            Err(ProjectionError::Projection("boom".into()))
        }
        async fn reset(&self) -> Result<(), ProjectionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn projections_see_the_write_and_a_projection_failure_does_not_undo_it() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let runner = ProjectionRunner::new(&store, &checkpoints);
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let good = CountProjection(AtomicU64::new(0));
        let bad = FailingProjection;
        let stream = StreamId::from_name("s");

        let c = flow
            .commit_and_project(
                stream,
                0,
                vec![event(QUEUED, b"x")],
                Timestamp(0),
                &runner,
                &[&good, &bad],
            )
            .await
            .unwrap();
        assert_eq!(
            good.0.load(Ordering::SeqCst),
            1,
            "read-your-writes in practice"
        );
        assert_eq!(c.projection_failures.len(), 1);
        assert_eq!(c.projection_failures[0].0, ProjectionId("failing"));
        // The command itself is durable regardless.
        assert_eq!(c.append.new_version, 1);
        assert_eq!(store.outbox().counts().await.unwrap().pending, 1);
    }

    #[tokio::test]
    async fn projection_replay_never_runs_effects_but_live_processing_does() {
        // §45 end to end: commit a message, rebuild its projection
        // from scratch (a version bump forces a full replay), and show
        // the send handler was never called. Only the processor, in
        // Live mode, ever calls it — and exactly once.
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let runner = ProjectionRunner::new(&store, &checkpoints);
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("s");
        let projection = CountProjection(AtomicU64::new(0));
        flow.commit_and_project(
            stream,
            0,
            vec![event(QUEUED, b"x")],
            Timestamp(0),
            &runner,
            &[&projection],
        )
        .await
        .unwrap();

        let handler = CountingHandler(AtomicU64::new(0));
        let outbox = store.outbox();
        let processor = EffectProcessor::new(
            &outbox,
            &handler,
            &NoTransitions,
            BackoffPolicy::DEFAULT,
            10_000,
            16,
        );

        // Full rebuild, in the mode that forbids external work.
        projection.reset().await.unwrap();
        let fresh_checkpoints = InMemoryCheckpointStore::new();
        let rebuild = ProjectionRunner::new(&store, &fresh_checkpoints);
        let outcome = rebuild
            .catch_up_with_mode(&projection, 50, ReplayMode::ProjectionOnly)
            .await
            .unwrap();
        assert_eq!(outcome.applied, 1);
        assert!(!outcome.may_schedule_external_work);
        processor
            .run_once(ReplayMode::ProjectionOnly, Timestamp(0))
            .await
            .unwrap();
        assert_eq!(handler.0.load(Ordering::SeqCst), 0, "replay sent nothing");

        processor
            .run_once(ReplayMode::Live, Timestamp(0))
            .await
            .unwrap();
        assert_eq!(handler.0.load(Ordering::SeqCst), 1);
        // And a second full rebuild after that still sends nothing more.
        let again_checkpoints = InMemoryCheckpointStore::new();
        let again = ProjectionRunner::new(&store, &again_checkpoints);
        projection.reset().await.unwrap();
        again
            .catch_up_with_mode(&projection, 50, ReplayMode::ProjectionOnly)
            .await
            .unwrap();
        processor
            .run_once(ReplayMode::Live, Timestamp(0))
            .await
            .unwrap();
        assert_eq!(handler.0.load(Ordering::SeqCst), 1);
    }
}
