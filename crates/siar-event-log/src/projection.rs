//! `04-offline-event-log-architecture.md` §16 "Projection Architecture",
//! §17 "Projection Checkpoints", §18 "Read-Your-Writes" — §92 Phase 4.
//!
//! §16's own diagram is the whole shape of this module:
//!
//! ```text
//! Event Journal
//!     ↓
//! Projection Runner
//!     ↓
//! Materialized Views
//! ```
//!
//! [`Projection`] is one materialized view's own update logic (§16's
//! examples: `messages`, `conversation_summary`, `transfer_state`,
//! `device_directory`, `group_state`, `dtn_bundle_state`,
//! `emergency_state` — this crate defines none of those; each belongs
//! in the crate that owns that domain, same reasoning §33-35's own
//! event catalogs already follow, per this crate's own top doc
//! comment). [`ProjectionRunner::catch_up`] is the "Projection Runner"
//! arrow: pull events from an [`crate::store::EventStore`] in log
//! order, `apply` each to a [`Projection`], and persist a
//! [`ProjectionCheckpoint`] (§17, verbatim field names) so a restart
//! resumes instead of replaying from the beginning.
//!
//! ## §16's four requirements, and which ones this module can actually
//! enforce
//!
//! - **versioned**: enforced structurally — [`Projection::
//!   projection_version`] is compared against the saved checkpoint's
//!   own `projection_version` on every [`ProjectionRunner::catch_up`]
//!   call; a mismatch means the projection's own `apply` logic changed
//!   since last run (a real schema/logic bump), not merely restarted,
//!   and triggers [`Projection::reset`] + a full replay from offset 0.
//! - **rebuildable**: also enforced structurally, via the same
//!   version-mismatch path — `reset` is a real trait method a
//!   `Projection` implementer must supply, not optional.
//! - **deterministic** and **idempotent**: NOT enforced by this
//!   module — properties of what a `Projection` implementer's own
//!   `apply` does with each event, which nothing here can verify from
//!   the outside. Named honestly rather than silently assumed.
//!
//! ## §18 "Read-Your-Writes": what this module does and doesn't give
//! for free
//!
//! §18's own text: "critical projections update in the same
//! transaction as the event." [`ProjectionRunner::catch_up`] does NOT
//! do that — it is a separate pull, called after `append` returns, not
//! inside the same `stoolap` transaction `stoolap_store::append` uses.
//! Building genuine same-transaction updates would mean either coupling
//! `EventStore::append` itself to a specific set of registered
//! projections (breaking §19/§92's own backend-neutral, domain-
//! agnostic design this crate has kept throughout Phases 1-3) or
//! accepting projection callbacks as a parameter on `append` (a real,
//! bigger trait redesign, not attempted here). What this module DOES
//! give a caller, and what's actually sufficient for §18's own concrete
//! example ("SendMessage succeeds locally → conversation immediately
//! shows message") in a single-process, no-concurrent-writer setting:
//! calling `catch_up` synchronously right after a successful `append`
//! returns, before doing anything else — see `siar_messaging::
//! service`'s own real caller for exactly this pattern. That's "read-
//! your-writes in practice," not "read-your-writes by construction";
//! the difference matters if this crate ever grows a concurrent-writer
//! story that assumption doesn't survive.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use thiserror::Error;

use crate::ids::LocalLogOffset;
use crate::store::{EventStore, EventStoreError, StoredEvent};

/// §17's own field type, verbatim. A `&'static str` rather than an
/// owned `String`: every real projection id (`"conversation_summary"`,
/// `"device_directory"`, ...) is a compile-time constant a projection
/// author writes once, not runtime-generated data — the same "why pay
/// for an allocation this never needs" reasoning `EventTypeId`'s own
/// bare `u32` already follows in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectionId(pub &'static str);

/// §17, verbatim field names and types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionCheckpoint {
    pub projection_id: ProjectionId,
    pub last_log_offset: LocalLogOffset,
    pub projection_version: u16,
}

#[derive(Debug, Error)]
pub enum ProjectionError {
    /// A [`Projection::apply`]/[`Projection::reset`] call failed —
    /// carries whatever that projection's own storage said, same
    /// "don't invent a taxonomy for errors this crate doesn't own"
    /// reasoning `EventStoreError::Backend` already uses for
    /// `stoolap_store`.
    #[error("projection apply/reset failed: {0}")]
    Projection(String),
    /// A [`ProjectionCheckpointStore`] call failed.
    #[error("projection checkpoint store failed: {0}")]
    Checkpoint(String),
    /// The [`crate::store::EventStore::read_log`] call inside
    /// [`ProjectionRunner::catch_up`] failed.
    #[error(transparent)]
    EventStore(#[from] EventStoreError),
}

/// One materialized view's own update logic — §16's examples list nine
/// of these across five-plus domain crates; this crate defines none of
/// them (see this module's own top doc comment).
#[async_trait]
pub trait Projection: Send + Sync {
    fn projection_id(&self) -> ProjectionId;

    /// Bump this whenever `apply`'s own logic changes in a way that
    /// makes previously-materialized state wrong or incomplete for the
    /// new logic — the version comparison in
    /// [`ProjectionRunner::catch_up`] is the only thing that decides
    /// whether a rebuild happens, so an unbumped version after a real
    /// logic change means catch_up silently keeps applying new events
    /// on top of state built by the old logic.
    fn projection_version(&self) -> u16;

    /// Apply one already-durable event to this projection's own
    /// materialized state. §16 requires this be deterministic and
    /// idempotent — not enforced by the trait signature, see this
    /// module's own doc comment.
    async fn apply(&self, event: &StoredEvent) -> Result<(), ProjectionError>;

    /// §16 "rebuildable": clear all materialized state so a fresh
    /// replay from offset 0 (which [`ProjectionRunner::catch_up`]
    /// triggers on its own after calling this) produces a correct,
    /// current view again.
    async fn reset(&self) -> Result<(), ProjectionError>;
}

/// Where a [`ProjectionCheckpoint`] lives between runs — deliberately
/// its own trait, not folded into [`crate::store::EventStore`]: a
/// checkpoint is per-projection-consumer state, not part of the event
/// log itself, and keeping them separate means a checkpoint store can
/// be swapped (or, for [`InMemoryCheckpointStore`], simply not
/// persisted at all) independently of which [`EventStore`] backend is
/// in use.
#[async_trait]
pub trait ProjectionCheckpointStore: Send + Sync {
    async fn load(
        &self,
        projection_id: ProjectionId,
    ) -> Result<Option<ProjectionCheckpoint>, ProjectionError>;

    async fn save(&self, checkpoint: ProjectionCheckpoint) -> Result<(), ProjectionError>;
}

/// The in-memory [`ProjectionCheckpointStore`] — same role
/// [`crate::memory_store::InMemoryEventStore`] plays for
/// [`EventStore`]: real, fully-tested, not durable across a process
/// restart (a durable one would live in `stoolap_store` alongside
/// `StoolapEventStore`, following the same pattern — not built this
/// round; nothing in `siar-messaging`'s own new `conversation_summary`
/// projection needs cross-restart checkpoint durability yet, since
/// that projection's own materialized state isn't durable either).
#[derive(Default)]
pub struct InMemoryCheckpointStore {
    checkpoints: Mutex<HashMap<ProjectionId, ProjectionCheckpoint>>,
}

impl InMemoryCheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ProjectionCheckpointStore for InMemoryCheckpointStore {
    async fn load(
        &self,
        projection_id: ProjectionId,
    ) -> Result<Option<ProjectionCheckpoint>, ProjectionError> {
        let checkpoints = self
            .checkpoints
            .lock()
            .expect("InMemoryCheckpointStore lock poisoned");
        Ok(checkpoints.get(&projection_id).copied())
    }

    async fn save(&self, checkpoint: ProjectionCheckpoint) -> Result<(), ProjectionError> {
        let mut checkpoints = self
            .checkpoints
            .lock()
            .expect("InMemoryCheckpointStore lock poisoned");
        checkpoints.insert(checkpoint.projection_id, checkpoint);
        Ok(())
    }
}

/// §16's "Projection Runner" arrow — see this module's own doc comment
/// for the full picture, including what §18 "read-your-writes" does
/// and doesn't mean here.
pub struct ProjectionRunner<'a> {
    event_store: &'a dyn EventStore,
    checkpoints: &'a dyn ProjectionCheckpointStore,
}

/// A default batch size a caller can reach for without picking one —
/// large enough that a real conversation's history rebuilds in a
/// handful of round trips, small enough that a `catch_up` call after
/// every single append (see `siar_messaging::service`'s own real
/// caller) is dominated by "usually zero or one new event," not by an
/// oversized read.
pub const DEFAULT_CATCH_UP_BATCH_SIZE: usize = 200;

impl<'a> ProjectionRunner<'a> {
    pub fn new(
        event_store: &'a dyn EventStore,
        checkpoints: &'a dyn ProjectionCheckpointStore,
    ) -> Self {
        Self {
            event_store,
            checkpoints,
        }
    }

    /// Applies every event `projection` hasn't seen yet, in log order,
    /// starting from its saved checkpoint (or from the beginning, on
    /// first run or a [`Projection::projection_version`] bump — see
    /// this module's own doc comment on §16's "versioned"/"rebuildable"
    /// requirements). Saves an updated checkpoint after each batch, not
    /// after every single event — real durability granularity is a
    /// batch, not a row, matching how big a re-application on restart
    /// after a crash mid-batch would be: bounded by `batch_size`, not
    /// unbounded.
    ///
    /// Returns how many events were actually applied (0 if the
    /// projection was already fully caught up).
    pub async fn catch_up(
        &self,
        projection: &dyn Projection,
        batch_size: usize,
    ) -> Result<u64, ProjectionError> {
        let projection_id = projection.projection_id();
        let existing = self.checkpoints.load(projection_id).await?;

        let mut from_offset = match existing {
            Some(checkpoint)
                if checkpoint.projection_version == projection.projection_version() =>
            {
                checkpoint.last_log_offset
            }
            Some(_stale_version) => {
                // §16 "versioned"/"rebuildable": the projection's own
                // logic moved on since this checkpoint was saved —
                // whatever it materialized under the old version is
                // not trustworthy input for the new one.
                projection.reset().await?;
                LocalLogOffset(0)
            }
            None => LocalLogOffset(0),
        };

        let mut applied = 0u64;
        loop {
            let events = self.event_store.read_log(from_offset, batch_size).await?;
            if events.is_empty() {
                break;
            }
            let batch_len = events.len();
            for event in &events {
                projection.apply(event).await?;
                from_offset = event.local_offset;
                applied += 1;
            }
            self.checkpoints
                .save(ProjectionCheckpoint {
                    projection_id,
                    last_log_offset: from_offset,
                    projection_version: projection.projection_version(),
                })
                .await?;
            if batch_len < batch_size {
                break;
            }
        }
        Ok(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, StreamId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::{AppendRequest, NewEvent};
    use siar_domain::DeviceId;
    use std::sync::atomic::{AtomicU64, Ordering};

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

    /// A toy projection: counts events applied since the last
    /// [`Projection::reset`]. Enough to prove catch-up, checkpoint
    /// resumption, and version-triggered rebuild all actually work,
    /// without needing a real domain's own event types.
    struct CountingProjection {
        version: u16,
        count: AtomicU64,
    }

    impl CountingProjection {
        fn new(version: u16) -> Self {
            Self {
                version,
                count: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl Projection for CountingProjection {
        fn projection_id(&self) -> ProjectionId {
            ProjectionId("counting_projection_test")
        }

        fn projection_version(&self) -> u16 {
            self.version
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

    #[tokio::test]
    async fn catch_up_applies_every_event_on_a_fresh_projection() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event(), event(), event()],
            })
            .await
            .unwrap();

        let projection = CountingProjection::new(1);
        let runner = ProjectionRunner::new(&store, &checkpoints);
        let applied = runner.catch_up(&projection, 200).await.unwrap();

        assert_eq!(applied, 3);
        assert_eq!(projection.count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn catch_up_is_a_true_no_op_when_already_caught_up() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event()],
            })
            .await
            .unwrap();

        let projection = CountingProjection::new(1);
        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection, 200).await.unwrap();
        let second_run = runner.catch_up(&projection, 200).await.unwrap();

        assert_eq!(second_run, 0, "nothing new since the last catch_up");
        assert_eq!(projection.count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn catch_up_resumes_from_the_saved_checkpoint_not_from_zero() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event(), event()],
            })
            .await
            .unwrap();

        let projection = CountingProjection::new(1);
        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection, 200).await.unwrap();

        // A new event arrives after the first catch_up.
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 2,
                events: vec![event()],
            })
            .await
            .unwrap();

        let applied = runner.catch_up(&projection, 200).await.unwrap();
        assert_eq!(applied, 1, "only the one new event, not a full replay");
        assert_eq!(projection.count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn catch_up_respects_batch_size_across_multiple_reads() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: (0..5).map(|_| event()).collect(),
            })
            .await
            .unwrap();

        let projection = CountingProjection::new(1);
        let runner = ProjectionRunner::new(&store, &checkpoints);
        // Batch size smaller than the total event count forces
        // `catch_up`'s own internal loop to run more than once.
        let applied = runner.catch_up(&projection, 2).await.unwrap();

        assert_eq!(applied, 5);
        assert_eq!(projection.count.load(Ordering::SeqCst), 5);
    }

    #[tokio::test]
    async fn a_projection_version_bump_triggers_a_full_reset_and_replay() {
        let store = InMemoryEventStore::new();
        let checkpoints = InMemoryCheckpointStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![event(), event()],
            })
            .await
            .unwrap();

        let projection_v1 = CountingProjection::new(1);
        let runner = ProjectionRunner::new(&store, &checkpoints);
        runner.catch_up(&projection_v1, 200).await.unwrap();
        assert_eq!(projection_v1.count.load(Ordering::SeqCst), 2);

        // A different `Projection` instance under the SAME
        // `projection_id`, but a bumped version — simulating "this
        // projection's own `apply` logic changed" between runs.
        let projection_v2 = CountingProjection::new(2);
        let applied = runner.catch_up(&projection_v2, 200).await.unwrap();

        assert_eq!(
            applied, 2,
            "version bump must trigger a full replay, not just the new events"
        );
        assert_eq!(projection_v2.count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn checkpoints_for_different_projection_ids_never_cross_over() {
        let checkpoints = InMemoryCheckpointStore::new();
        let a = ProjectionId("projection_a");
        let b = ProjectionId("projection_b");

        checkpoints
            .save(ProjectionCheckpoint {
                projection_id: a,
                last_log_offset: LocalLogOffset(5),
                projection_version: 1,
            })
            .await
            .unwrap();

        assert!(checkpoints.load(b).await.unwrap().is_none());
        assert_eq!(
            checkpoints.load(a).await.unwrap().unwrap().last_log_offset,
            LocalLogOffset(5)
        );
    }
}
