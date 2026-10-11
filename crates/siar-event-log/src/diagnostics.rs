//! `04-offline-event-log-architecture.md` §81 "Diagnostics" (and, with
//! [`crate::metrics`], §82).
//!
//! §81: expose "latest local offset, event-store health, projection lag,
//! pending effect count, last successful compaction — **without exposing
//! content**."
//!
//! [`collect_diagnostics`] gathers exactly those five facts into
//! [`EventLogDiagnostics`], and the "without exposing content" half is
//! enforced the same way [`crate::analytics`] enforces its rule: by
//! shape. Every field is a number, an offset, a static projection name,
//! or a fixed enum — there is no `String` built from stored data, no
//! event id, no stream id, no payload, no error *message* (an error is
//! reduced to a [`StoreHealth`] variant; the backend's text, which could
//! quote a path or a row, is dropped). A diagnostics blob is therefore
//! safe to put in a bug report or show on a debug screen.
//!
//! ## Reading it
//!
//! - **`projection_lag`** is `latest_offset − applied_through`, per
//!   projection. `0` = caught up. A projection with *no checkpoint at all*
//!   reports `applied_through = 0` and lag = the whole log, which is
//!   exactly the truth (it has applied nothing) and is how a
//!   never-started projection looks distinct from a healthy one.
//! - **`pending_effects`** is the outbox's own counts. `dead_lettered`
//!   is the number a human should look at.
//! - **`health`** is a cheap probe (read the log head), not a full
//!   integrity scan: it detects a store that is unreachable, read-only,
//!   full or visibly corrupt *at the tail*; [`crate::integrity`] is the
//!   deep check.

use crate::ids::{LocalLogOffset, Timestamp};
use crate::outbox::{Outbox, OutboxCounts};
use crate::projection::{ProjectionCheckpointStore, ProjectionId};
use crate::store::{EventStore, EventStoreError};

/// A deliberately coarse health reading — never the backend's own text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreHealth {
    Healthy,
    ReadOnly,
    StorageFull,
    /// A row at the log head failed its integrity check.
    Corrupt,
    /// Any other backend failure.
    Unavailable,
}

impl StoreHealth {
    fn from_error(e: &EventStoreError) -> Self {
        match e {
            EventStoreError::ReadOnly => Self::ReadOnly,
            EventStoreError::StorageFull => Self::StorageFull,
            EventStoreError::Corrupt(_) => Self::Corrupt,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionLag {
    pub projection: ProjectionId,
    pub applied_through: LocalLogOffset,
    pub lag_events: u64,
}

/// Fixed-shape, content-free. See the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogDiagnostics {
    pub latest_offset: LocalLogOffset,
    pub health: StoreHealth,
    pub projection_lag: Vec<ProjectionLag>,
    pub pending_effects: OutboxCounts,
    /// When the last successful compaction finished; the caller records
    /// it (this crate does not schedule compaction).
    pub last_compaction: Option<Timestamp>,
}

pub async fn collect_diagnostics<S, C, O>(
    store: &S,
    checkpoints: &C,
    projections: &[ProjectionId],
    outbox: &O,
    last_compaction: Option<Timestamp>,
) -> EventLogDiagnostics
where
    S: EventStore + ?Sized,
    C: ProjectionCheckpointStore + ?Sized,
    O: Outbox + ?Sized,
{
    // Probe: the head offset, then read the head event itself so a
    // corrupt tail row is noticed (latest_offset alone wouldn't).
    let (latest_offset, health) = match store.latest_offset().await {
        Ok(latest) => {
            if latest.0 == 0 {
                (latest, StoreHealth::Healthy)
            } else {
                let from = LocalLogOffset(latest.0 - 1);
                match store.read_log(from, 1).await {
                    Ok(_) => (latest, StoreHealth::Healthy),
                    Err(e) => (latest, StoreHealth::from_error(&e)),
                }
            }
        }
        Err(e) => (LocalLogOffset(0), StoreHealth::from_error(&e)),
    };

    let mut projection_lag = Vec::with_capacity(projections.len());
    for id in projections {
        let applied = match checkpoints.load(*id).await {
            Ok(Some(cp)) => cp.last_log_offset,
            _ => LocalLogOffset(0),
        };
        projection_lag.push(ProjectionLag {
            projection: *id,
            applied_through: applied,
            lag_events: latest_offset.0.saturating_sub(applied.0),
        });
    }

    let pending_effects = outbox.counts().await.unwrap_or_default();

    EventLogDiagnostics {
        latest_offset,
        health,
        projection_lag,
        pending_effects,
        last_compaction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, StreamId};
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::{EffectKind, OutboxEntry};
    use crate::projection::{
        InMemoryCheckpointStore, Projection, ProjectionCheckpoint, ProjectionError,
        ProjectionRunner,
    };
    use crate::read_only::ReadOnlyEventStore;
    use crate::store::{AppendRequest, NewEvent, StoredEvent};
    use async_trait::async_trait;

    fn req(n: usize, version: u64) -> AppendRequest {
        AppendRequest {
            stream_id: StreamId::from_name("s"),
            expected_version: version,
            events: (0..n)
                .map(|i| NewEvent {
                    event_id: EventId::new(),
                    event_type: EventTypeId(1),
                    schema_version: 1,
                    created_at: Timestamp(0),
                    origin: EventOrigin::System,
                    correlation_id: None,
                    causation_id: None,
                    payload: format!("SECRET-{i}").into_bytes(),
                })
                .collect(),
        }
    }

    struct Noop(&'static str);
    #[async_trait]
    impl Projection for Noop {
        fn projection_id(&self) -> ProjectionId {
            ProjectionId(self.0)
        }
        fn projection_version(&self) -> u16 {
            1
        }
        async fn apply(&self, _e: &StoredEvent) -> Result<(), ProjectionError> {
            Ok(())
        }
        async fn reset(&self) -> Result<(), ProjectionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn an_empty_store_is_healthy_with_no_lag() {
        let store = InMemoryEventStore::new();
        let cps = InMemoryCheckpointStore::new();
        let d =
            collect_diagnostics(&store, &cps, &[ProjectionId("p")], &store.outbox(), None).await;
        assert_eq!(d.latest_offset, LocalLogOffset(0));
        assert_eq!(d.health, StoreHealth::Healthy);
        assert_eq!(d.projection_lag[0].lag_events, 0);
        assert_eq!(d.pending_effects, OutboxCounts::default());
        assert_eq!(d.last_compaction, None);
    }

    #[tokio::test]
    async fn projection_lag_distinguishes_caught_up_behind_and_never_started() {
        let store = InMemoryEventStore::new();
        store.append(req(10, 0)).await.unwrap();
        let cps = InMemoryCheckpointStore::new();
        let runner = ProjectionRunner::new(&store, &cps);
        runner.catch_up(&Noop("caught-up"), 100).await.unwrap();
        // "behind" applied only the first 4 events of the 10: hand it
        // that checkpoint directly.
        cps.save(ProjectionCheckpoint {
            projection_id: ProjectionId("behind"),
            last_log_offset: LocalLogOffset(4),
            projection_version: 1,
        })
        .await
        .unwrap();

        let d = collect_diagnostics(
            &store,
            &cps,
            &[
                ProjectionId("caught-up"),
                ProjectionId("behind"),
                ProjectionId("never-started"),
            ],
            &store.outbox(),
            Some(Timestamp(123)),
        )
        .await;
        assert_eq!(d.latest_offset, LocalLogOffset(10));
        let lag = |name| {
            d.projection_lag
                .iter()
                .find(|l| l.projection == ProjectionId(name))
                .unwrap()
                .lag_events
        };
        assert_eq!(lag("caught-up"), 0);
        assert_eq!(lag("behind"), 6);
        assert_eq!(
            lag("never-started"),
            10,
            "applied nothing: lag is the whole log"
        );
        assert_eq!(d.last_compaction, Some(Timestamp(123)));
    }

    #[tokio::test]
    async fn pending_effect_counts_come_from_the_outbox() {
        let store = InMemoryEventStore::new();
        let outbox = store.outbox();
        for i in 0..3u8 {
            outbox
                .enqueue(
                    OutboxEntry::new(
                        EventId::new(),
                        StreamId::from_name("s"),
                        EffectKind(1),
                        0,
                        vec![i],
                        Timestamp(0),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let d =
            collect_diagnostics(&store, &InMemoryCheckpointStore::new(), &[], &outbox, None).await;
        assert_eq!(d.pending_effects.pending, 3);
    }

    #[tokio::test]
    async fn health_reflects_read_only_and_never_carries_backend_text() {
        let ro = ReadOnlyEventStore::new(InMemoryEventStore::new());
        let cps = InMemoryCheckpointStore::new();
        let outbox = crate::outbox::InMemoryOutbox::new();
        // Reads still work on a read-only store.
        let d = collect_diagnostics(&ro, &cps, &[], &outbox, None).await;
        assert_eq!(d.health, StoreHealth::Healthy);

        assert_eq!(
            StoreHealth::from_error(&EventStoreError::ReadOnly),
            StoreHealth::ReadOnly
        );
        assert_eq!(
            StoreHealth::from_error(&EventStoreError::StorageFull),
            StoreHealth::StorageFull
        );
        assert_eq!(
            StoreHealth::from_error(&EventStoreError::Corrupt(
                "row 7 at /home/alice/secret.db".into()
            )),
            StoreHealth::Corrupt
        );
        assert_eq!(
            StoreHealth::from_error(&EventStoreError::Backend(
                "disk error at /home/alice".into()
            )),
            StoreHealth::Unavailable
        );
    }

    #[tokio::test]
    async fn the_report_contains_no_stored_content() {
        let store = InMemoryEventStore::new();
        store.append(req(5, 0)).await.unwrap();
        let d = collect_diagnostics(
            &store,
            &InMemoryCheckpointStore::new(),
            &[ProjectionId("p")],
            &store.outbox(),
            None,
        )
        .await;
        let rendered = format!("{d:?}");
        assert!(!rendered.contains("SECRET"), "{rendered}");
        // Only fixed-shape facts appear.
        assert!(rendered.contains("latest_offset"));
    }
}
