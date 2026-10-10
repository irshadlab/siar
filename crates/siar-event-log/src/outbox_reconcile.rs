//! `04-offline-event-log-architecture.md` §48 "Work Queue
//! Reconciliation", and the outbox end of §47 "Startup Recovery"
//! ("resume projections → *reconcile work queues* → load local read
//! models → render UI → start networking") — §92 Phase 5's "recovery".
//!
//! §48, verbatim: "if an event indicates a pending operation but the
//! optimized work table is missing/corrupt, a recovery reconciler can
//! reconstruct it. This is a key advantage of retaining semantic
//! history." [`reconcile_outbox`] is that reconciler for the
//! [`crate::outbox`] table.
//!
//! ## How it differs from `siar-startup-recovery`'s `reconcile`
//!
//! That crate's generic `reconcile` diffs two id sets and knows nothing
//! about events, stores, or outbox rows (deliberately — it has no
//! dependency on this crate). This module is the concrete,
//! event-log-aware version: it replays the real log through a real
//! [`EffectPlanner`], rebuilds *full* [`OutboxEntry`] values (not just
//! ids), and can write the repair. Both read the same spec section; this
//! one is what a node's startup sequence actually calls.
//!
//! ## What "pending according to history" means
//!
//! Replaying the log in order, for every event:
//!
//! 1. [`EffectPlanner::plan`] says which operations it creates →
//!    *opened*.
//! 2. [`EffectPlanner::closes`] says which operations it discharges →
//!    *closed* (and removed from *opened*).
//!
//! What remains *opened* is what history says is still owed. Because
//! [`OperationId`]s are derived from `(source event, kind, slot)`, the
//! entries rebuilt here have exactly the ids the original commit
//! produced — a node that lost its whole outbox rebuilds a table the
//! receivers cannot distinguish from the one that was lost, so their
//! deduplication (§31) keeps working across the loss.
//!
//! ## What it finds, and what it will and won't repair
//!
//! | finding | meaning | [`RepairMode::Repair`] |
//! |---|---|---|
//! | [`OutboxReconciliation::missing`] | history says owed; no row exists | enqueue the rebuilt row |
//! | [`OutboxReconciliation::stale_closed`] | row still open; history says discharged | mark `Done` |
//! | [`OutboxReconciliation::done_but_open`] | row `Done`; history never recorded the discharge (crash between effect and closing event) | **report only** — the owning domain appends its own closing event |
//! | [`OutboxReconciliation::unplanned`] | open row no event in history plans | **report only** |
//!
//! `unplanned` is never auto-repaired on purpose: it is also what a
//! *planner change* looks like (a new planner version no longer plans a
//! kind that old rows still owe). Dead-lettering real pending work
//! because a mapping evolved would turn a code change into data loss.
//!
//! ## Preconditions
//!
//! Run it during recovery, **before** any [`crate::effects::
//! EffectProcessor`] starts (that is §47's order). Events appended
//! while a scan is running are tolerated — they can only appear as
//! `unplanned`, never cause a wrong repair — but a clean report needs a
//! quiescent log.
//!
//! Memory: one [`OutboxEntry`] per *open* operation plus 16 bytes per
//! closed id. Snapshots (§38, still open) will bound the closed set.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::command_flow::{EffectPlanner, EventRef};
use crate::ids::{LocalLogOffset, Timestamp};
use crate::outbox::{EnqueueOutcome, OperationId, Outbox, OutboxEntry, OutboxError, OutboxState};
use crate::store::{EventStore, EventStoreError};

/// Events read per round trip during the scan.
pub const RECONCILE_SCAN_BATCH: usize = 512;

/// Cap on open outbox rows examined for `unplanned`/`stale` detection.
/// When hit, [`OutboxReconciliation::open_scan_truncated`] is set.
pub const RECONCILE_MAX_OPEN_ROWS: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairMode {
    /// Compute and report; write nothing.
    DryRun,
    /// Apply the repairs in the module doc's table.
    Repair,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutboxReconciliation {
    pub events_scanned: u64,
    pub missing: Vec<OperationId>,
    pub stale_closed: Vec<OperationId>,
    pub done_but_open: Vec<OperationId>,
    pub unplanned: Vec<OperationId>,
    pub open_scan_truncated: bool,
    /// `true` only if [`RepairMode::Repair`] ran.
    pub repaired: bool,
}

impl OutboxReconciliation {
    /// Nothing for a human or the domain to look at.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
            && self.stale_closed.is_empty()
            && self.done_but_open.is_empty()
            && self.unplanned.is_empty()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReconcileError {
    #[error(transparent)]
    Store(#[from] EventStoreError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
}

/// What replaying the log says is owed — see the module doc's
/// "pending according to history". Shared with [`crate::backup`]'s
/// restore path.
pub(crate) struct History {
    pub(crate) opened: HashMap<OperationId, OutboxEntry>,
    pub(crate) closed: HashSet<OperationId>,
    pub(crate) events_scanned: u64,
}

pub(crate) async fn scan_history<S, P>(store: &S, planner: &P) -> Result<History, ReconcileError>
where
    S: EventStore + ?Sized,
    P: EffectPlanner + ?Sized,
{
    let mut history = History {
        opened: HashMap::new(),
        closed: HashSet::new(),
        events_scanned: 0,
    };
    let mut from = LocalLogOffset(0);
    loop {
        let batch = store.read_log(from, RECONCILE_SCAN_BATCH).await?;
        let Some(last) = batch.last() else { break };
        from = last.local_offset;
        for stored in &batch {
            history.events_scanned += 1;
            let view = EventRef::from_stored(stored);
            for intent in planner.plan(&view) {
                let entry = OutboxEntry::new(
                    view.event_id,
                    view.stream_id,
                    intent.kind,
                    intent.slot,
                    intent.payload,
                    // The obligation dates from the event, not from
                    // this recovery run.
                    view.created_at,
                )?;
                if !history.closed.contains(&entry.operation_id) {
                    history.opened.insert(entry.operation_id, entry);
                }
            }
            for id in planner.closes(&view) {
                history.opened.remove(&id);
                history.closed.insert(id);
            }
        }
    }
    Ok(history)
}

/// See the module doc.
pub async fn reconcile_outbox<S, O, P>(
    store: &S,
    outbox: &O,
    planner: &P,
    mode: RepairMode,
    now: Timestamp,
) -> Result<OutboxReconciliation, ReconcileError>
where
    S: EventStore + ?Sized,
    O: Outbox + ?Sized,
    P: EffectPlanner + ?Sized,
{
    let mut report = OutboxReconciliation::default();
    let History {
        mut opened,
        closed,
        events_scanned,
    } = scan_history(store, planner).await?;
    report.events_scanned = events_scanned;

    // 2. History says owed, table disagrees.
    let mut open_ids: Vec<OperationId> = opened.keys().copied().collect();
    open_ids.sort_unstable();
    for id in open_ids {
        match outbox.get(id).await? {
            None => report.missing.push(id),
            Some(row) if row.state == OutboxState::Done => report.done_but_open.push(id),
            Some(_) => {}
        }
    }

    // 3. Table says owed, history disagrees.
    let rows = outbox.list_open(RECONCILE_MAX_OPEN_ROWS).await?;
    report.open_scan_truncated = rows.len() >= RECONCILE_MAX_OPEN_ROWS;
    for row in &rows {
        let id = row.operation_id;
        if closed.contains(&id) {
            report.stale_closed.push(id);
        } else if !opened.contains_key(&id) {
            report.unplanned.push(id);
        }
    }
    report.stale_closed.sort_unstable();
    report.unplanned.sort_unstable();

    // 4. Repair.
    if mode == RepairMode::Repair {
        for id in &report.missing {
            if let Some(entry) = opened.remove(id) {
                // Idempotent; a concurrent enqueue is fine.
                let _: EnqueueOutcome = outbox.enqueue(entry).await?;
            }
        }
        for id in &report.stale_closed {
            outbox.complete(*id, now).await?;
        }
        report.repaired = true;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_flow::{tests_support::*, CommandFlow};
    use crate::ids::StreamId;
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::{InMemoryOutbox, TransactionalOutbox};
    use crate::store::NewEvent;

    async fn queue(
        store: &InMemoryEventStore,
        stream: StreamId,
        version: u64,
        payload: &[u8],
    ) -> (NewEvent, u64) {
        let flow = CommandFlow::new(store, &MessagePlanner);
        let e = event(QUEUED, payload);
        let c = flow
            .commit(stream, version, vec![e.clone()], Timestamp(5))
            .await
            .unwrap();
        (e, c.append.new_version)
    }

    async fn sent(
        store: &InMemoryEventStore,
        stream: StreamId,
        version: u64,
        cause: &NewEvent,
    ) -> u64 {
        let mut e = event(SENT, b"");
        e.causation_id = Some(cause.event_id);
        let r = store
            .append_with_outbox(
                crate::store::AppendRequest {
                    stream_id: stream,
                    expected_version: version,
                    events: vec![e],
                },
                vec![],
            )
            .await
            .unwrap();
        r.new_version
    }

    #[tokio::test]
    async fn a_consistent_node_reports_clean_and_changes_nothing() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        queue(&store, stream, 0, b"a").await;

        let before = store.outbox().counts().await.unwrap();
        let r = reconcile_outbox(
            &store,
            &store.outbox(),
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(100),
        )
        .await
        .unwrap();
        assert!(r.is_clean(), "{r:?}");
        assert_eq!(r.events_scanned, 1);
        assert_eq!(store.outbox().counts().await.unwrap(), before);
    }

    #[tokio::test]
    async fn a_lost_outbox_is_rebuilt_with_the_original_operation_ids() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let (e1, v) = queue(&store, stream, 0, b"one").await;
        let (e2, v) = queue(&store, stream, v, b"two").await;
        let (e3, v) = queue(&store, stream, v, b"three").await;
        sent(&store, stream, v, &e2).await; // e2 already delivered

        // Disaster: the work table is gone, the log survives.
        let fresh = InMemoryOutbox::new();
        let r = reconcile_outbox(
            &store,
            &fresh,
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(9),
        )
        .await
        .unwrap();

        let id = |e: &NewEvent| OperationId::derive(e.event_id, SEND_EFFECT, 0);
        let mut want = vec![id(&e1), id(&e3)];
        want.sort_unstable();
        assert_eq!(r.missing, want, "e2 was discharged, so not rebuilt");
        assert!(r.repaired);
        assert_eq!(fresh.counts().await.unwrap().pending, 2);

        // Rebuilt rows match what the original commit enqueued — same
        // id, payload, and source event — so receivers still dedup.
        let original = store.outbox().get(id(&e1)).await.unwrap().unwrap();
        let rebuilt = fresh.get(id(&e1)).await.unwrap().unwrap();
        assert_eq!(rebuilt.operation_id, original.operation_id);
        assert_eq!(rebuilt.payload, original.payload);
        assert_eq!(rebuilt.source_event_id, original.source_event_id);
        assert_eq!(rebuilt.created_at, Timestamp(1), "dated from the event");
        assert!(fresh.get(id(&e2)).await.unwrap().is_none());

        // Idempotent: a second pass finds nothing left to do.
        let again = reconcile_outbox(
            &store,
            &fresh,
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(10),
        )
        .await
        .unwrap();
        assert!(again.is_clean(), "{again:?}");
    }

    #[tokio::test]
    async fn dry_run_reports_but_writes_nothing() {
        let store = InMemoryEventStore::new();
        queue(&store, StreamId::from_name("s"), 0, b"a").await;
        let fresh = InMemoryOutbox::new();
        let r = reconcile_outbox(
            &store,
            &fresh,
            &MessagePlanner,
            RepairMode::DryRun,
            Timestamp(0),
        )
        .await
        .unwrap();
        assert_eq!(r.missing.len(), 1);
        assert!(!r.repaired);
        assert_eq!(fresh.counts().await.unwrap().pending, 0);
    }

    #[tokio::test]
    async fn a_row_history_says_is_discharged_is_closed_not_resent() {
        // The crash window: MessageSent was appended, but the outbox
        // row never reached Done. Without reconciliation the effect
        // would run again after restart.
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let (e, v) = queue(&store, stream, 0, b"a").await;
        sent(&store, stream, v, &e).await;
        let id = OperationId::derive(e.event_id, SEND_EFFECT, 0);
        assert_eq!(
            store.outbox().get(id).await.unwrap().unwrap().state,
            OutboxState::Pending
        );

        let r = reconcile_outbox(
            &store,
            &store.outbox(),
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(7),
        )
        .await
        .unwrap();
        assert_eq!(r.stale_closed, vec![id]);
        assert_eq!(
            store.outbox().get(id).await.unwrap().unwrap().state,
            OutboxState::Done
        );
        assert!(store
            .outbox()
            .claim_due(Timestamp(1_000), 10, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn done_row_with_no_recorded_discharge_is_reported_never_rewritten() {
        let store = InMemoryEventStore::new();
        let (e, _) = queue(&store, StreamId::from_name("s"), 0, b"a").await;
        let id = OperationId::derive(e.event_id, SEND_EFFECT, 0);
        store.outbox().complete(id, Timestamp(3)).await.unwrap();

        let r = reconcile_outbox(
            &store,
            &store.outbox(),
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(9),
        )
        .await
        .unwrap();
        assert_eq!(r.done_but_open, vec![id]);
        assert!(r.missing.is_empty());
        assert_eq!(
            store.outbox().get(id).await.unwrap().unwrap().state,
            OutboxState::Done
        );
    }

    #[tokio::test]
    async fn an_unplanned_row_is_reported_but_never_dead_lettered() {
        let store = InMemoryEventStore::new();
        queue(&store, StreamId::from_name("s"), 0, b"a").await;
        let stray = OutboxEntry::new(
            crate::ids::EventId::new(),
            StreamId::from_name("other"),
            SEND_EFFECT,
            0,
            vec![],
            Timestamp(0),
        )
        .unwrap();
        let stray_id = stray.operation_id;
        store.outbox().enqueue(stray).await.unwrap();

        let r = reconcile_outbox(
            &store,
            &store.outbox(),
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(9),
        )
        .await
        .unwrap();
        assert_eq!(r.unplanned, vec![stray_id]);
        assert_eq!(
            store.outbox().get(stray_id).await.unwrap().unwrap().state,
            OutboxState::Pending,
            "a planner change must not become data loss"
        );
    }

    #[tokio::test]
    async fn scanning_spans_multiple_read_batches() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let n = RECONCILE_SCAN_BATCH + 40;
        let mut v = 0;
        for i in 0..n {
            let (_, nv) = queue(&store, stream, v, &(i as u32).to_le_bytes()).await;
            v = nv;
        }
        let fresh = InMemoryOutbox::new();
        let r = reconcile_outbox(
            &store,
            &fresh,
            &MessagePlanner,
            RepairMode::Repair,
            Timestamp(0),
        )
        .await
        .unwrap();
        assert_eq!(r.events_scanned as usize, n);
        assert_eq!(r.missing.len(), n);
        assert_eq!(fresh.counts().await.unwrap().pending as usize, n);
    }
}
