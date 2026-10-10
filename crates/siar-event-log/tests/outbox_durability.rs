//! §14/§31/§48 against the real `stoolap` backend: the outbox commits
//! atomically with events, survives process death, behaves identically
//! to the in-memory reference table, and a lost table is rebuilt from
//! the log.

use proptest::prelude::*;
use siar_domain::DeviceId;
use siar_event_log::command_flow::{EffectIntent, EffectPlanner, EventRef};
use siar_event_log::effects::{
    BackoffPolicy, EffectHandler, EffectOutcome, EffectProcessor, NoTransitions,
};
use siar_event_log::outbox::{
    EffectKind, InMemoryOutbox, OperationId, Outbox, OutboxEntry, OutboxState, TransactionalOutbox,
    MAX_OUTBOX_PAYLOAD_BYTES,
};
use siar_event_log::outbox_reconcile::{reconcile_outbox, RepairMode};
use siar_event_log::stoolap_outbox::StoolapOutbox;
use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventStoreError, EventTypeId,
    InMemoryEventStore, LocalLogOffset, NewEvent, ReplayMode, StoolapEventStore, StreamId,
    Timestamp,
};
use std::sync::atomic::{AtomicU64, Ordering};

const QUEUED: EventTypeId = EventTypeId(1);
const SEND: EffectKind = EffectKind(10);

struct Planner;
impl EffectPlanner for Planner {
    fn plan(&self, e: &EventRef<'_>) -> Vec<EffectIntent> {
        if e.event_type == QUEUED {
            vec![EffectIntent {
                kind: SEND,
                slot: 0,
                payload: e.payload.to_vec(),
            }]
        } else {
            vec![]
        }
    }
}

fn event(payload: &[u8]) -> NewEvent {
    NewEvent {
        event_id: EventId::new(),
        event_type: QUEUED,
        schema_version: 1,
        created_at: Timestamp(1),
        origin: EventOrigin::LocalDevice(DeviceId::new()),
        correlation_id: None,
        causation_id: None,
        payload: payload.to_vec(),
    }
}

fn entry_for(e: &NewEvent, stream: StreamId) -> OutboxEntry {
    OutboxEntry::new(e.event_id, stream, SEND, 0, e.payload.clone(), Timestamp(2)).unwrap()
}

fn temp_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("siar-outbox-{tag}-{}", uuid::Uuid::new_v4()))
        .to_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn event_and_outbox_row_commit_together() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    let e = event(b"hello");
    let entry = entry_for(&e, stream);
    let id = entry.operation_id;

    store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![e],
            },
            vec![entry],
        )
        .await
        .unwrap();

    let outbox = store.outbox().unwrap();
    let row = outbox.get(id).await.unwrap().unwrap();
    assert_eq!((row.state, row.attempts), (OutboxState::Pending, 0));
    assert_eq!(row.payload, b"hello");
    assert_eq!(
        store.read_log(LocalLogOffset(0), 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_conflict_rolls_back_the_outbox_row_too() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    let first = event(b"1");
    store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![first.clone()],
            },
            vec![entry_for(&first, stream)],
        )
        .await
        .unwrap();

    let stale = event(b"2");
    let stale_entry = entry_for(&stale, stream);
    let stale_id = stale_entry.operation_id;
    let err = store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0, // stale
                events: vec![stale],
            },
            vec![stale_entry],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EventStoreError::ConcurrencyConflict { .. }));

    let outbox = store.outbox().unwrap();
    assert!(outbox.get(stale_id).await.unwrap().is_none());
    assert_eq!(outbox.counts().await.unwrap().pending, 1);
    assert_eq!(
        store.read_log(LocalLogOffset(0), 10).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn an_invalid_outbox_row_leaves_no_event_behind() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    let e = event(b"x");
    let mut bad = entry_for(&e, stream);
    bad.payload = vec![0; MAX_OUTBOX_PAYLOAD_BYTES + 1];

    assert!(store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![e],
            },
            vec![bad],
        )
        .await
        .is_err());
    assert!(store
        .read_log(LocalLogOffset(0), 10)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn resubmitting_the_same_command_is_idempotent_on_disk() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    let e = event(b"x");
    for version in [0u64, 1] {
        store
            .append_with_outbox(
                AppendRequest {
                    stream_id: stream,
                    expected_version: version,
                    events: vec![e.clone()],
                },
                vec![entry_for(&e, stream)],
            )
            .await
            .unwrap();
    }
    assert_eq!(
        store.read_log(LocalLogOffset(0), 10).await.unwrap().len(),
        1
    );
    assert_eq!(store.outbox().unwrap().counts().await.unwrap().pending, 1);
}

#[tokio::test]
async fn the_outbox_survives_process_death_and_a_claimed_entry_is_retaken() {
    // §14: "if the process dies, the outbox still exists." And §31:
    // an effect claimed but never completed comes back with the SAME
    // operation id once its lease lapses.
    let path = temp_path("durable");
    let stream = StreamId::from_name("s");
    let e = event(b"survive");
    let id = OperationId::derive(e.event_id, SEND, 0);
    {
        let store = StoolapEventStore::open(&path).unwrap();
        store
            .append_with_outbox(
                AppendRequest {
                    stream_id: stream,
                    expected_version: 0,
                    events: vec![e.clone()],
                },
                vec![entry_for(&e, stream)],
            )
            .await
            .unwrap();
        // A worker claims it, then the "process" dies mid-effect.
        let claimed = store
            .outbox()
            .unwrap()
            .claim_due(Timestamp(100), 1_000, 8)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
    }
    {
        let store = StoolapEventStore::open(&path).unwrap();
        let outbox = store.outbox().unwrap();
        let row = outbox.get(id).await.unwrap().unwrap();
        assert_eq!((row.state, row.attempts), (OutboxState::InFlight, 1));
        assert_eq!(row.lease_until, Some(Timestamp(1_100)));

        // Lease still live: not handed out.
        assert!(outbox
            .claim_due(Timestamp(1_099), 1_000, 8)
            .await
            .unwrap()
            .is_empty());
        // Lapsed: same id, attempt 2.
        let again = outbox.claim_due(Timestamp(1_100), 1_000, 8).await.unwrap();
        assert_eq!(again[0].operation_id, id);
        assert_eq!(again[0].attempts, 2);

        // Or, at startup, release without waiting out leases (§47).
        assert_eq!(
            outbox
                .release_all_in_flight(Timestamp(1_200))
                .await
                .unwrap(),
            1
        );
        assert_eq!(outbox.counts().await.unwrap().pending, 1);
    }
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_file(&path);
}

struct Counting(AtomicU64);
impl EffectHandler for Counting {
    async fn handle(&self, _e: &OutboxEntry) -> EffectOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        EffectOutcome::Done
    }
}

#[tokio::test]
async fn processor_drains_the_durable_outbox_and_work_is_not_repeated() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    for i in 0..5u8 {
        let e = event(&[i]);
        let v = i as u64;
        store
            .append_with_outbox(
                AppendRequest {
                    stream_id: stream,
                    expected_version: v,
                    events: vec![e.clone()],
                },
                vec![entry_for(&e, stream)],
            )
            .await
            .unwrap();
    }
    let outbox = store.outbox().unwrap();
    let handler = Counting(AtomicU64::new(0));
    let p = EffectProcessor::new(
        &outbox,
        &handler,
        &NoTransitions,
        BackoffPolicy::DEFAULT,
        10_000,
        3,
    );

    assert_eq!(
        p.run_once(ReplayMode::Live, Timestamp(10))
            .await
            .unwrap()
            .completed,
        3
    );
    assert_eq!(
        p.run_once(ReplayMode::Live, Timestamp(10))
            .await
            .unwrap()
            .completed,
        2
    );
    assert_eq!(
        p.run_once(ReplayMode::Live, Timestamp(10))
            .await
            .unwrap()
            .claimed,
        0
    );
    assert_eq!(handler.0.load(Ordering::SeqCst), 5);
    assert_eq!(outbox.counts().await.unwrap().done, 5);

    // Retention for finished work.
    assert_eq!(outbox.purge_done(Timestamp(11)).await.unwrap(), 5);
    assert_eq!(outbox.counts().await.unwrap().done, 0);
}

#[tokio::test]
async fn a_deleted_outbox_table_is_rebuilt_from_the_durable_log() {
    let path = temp_path("rebuild");
    let stream = StreamId::from_name("s");
    let events: Vec<NewEvent> = (0..4u8).map(|i| event(&[i])).collect();
    {
        let store = StoolapEventStore::open(&path).unwrap();
        for (i, e) in events.iter().enumerate() {
            store
                .append_with_outbox(
                    AppendRequest {
                        stream_id: stream,
                        expected_version: i as u64,
                        events: vec![e.clone()],
                    },
                    vec![entry_for(e, stream)],
                )
                .await
                .unwrap();
        }
    }
    // Corruption: the work table (only) is lost; the log is intact.
    {
        let db = stoolap::Database::open(&format!("file://{path}")).unwrap();
        db.execute("DELETE FROM outbox", ()).unwrap();
    }
    {
        let store = StoolapEventStore::open(&path).unwrap();
        let outbox = store.outbox().unwrap();
        assert_eq!(outbox.counts().await.unwrap().pending, 0);

        let report = reconcile_outbox(&store, &outbox, &Planner, RepairMode::Repair, Timestamp(50))
            .await
            .unwrap();
        assert_eq!(report.missing.len(), 4);
        assert_eq!(outbox.counts().await.unwrap().pending, 4);
        for e in &events {
            let id = OperationId::derive(e.event_id, SEND, 0);
            let row = outbox.get(id).await.unwrap().unwrap();
            assert_eq!(row.payload, e.payload, "rebuilt payload matches original");
        }
        assert!(
            reconcile_outbox(&store, &outbox, &Planner, RepairMode::Repair, Timestamp(51))
                .await
                .unwrap()
                .is_clean()
        );
    }
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------
// Parity: the durable table must be observably identical to the
// reference `OutboxTable` for every sequence of operations.
// ---------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Enqueue { n: u8, not_before: u64 },
    Claim { now: u64, lease: u64, limit: usize },
    Complete { n: u8 },
    Reschedule { n: u8, not_before: u64 },
    DeadLetter { n: u8 },
    Release,
    Purge { before: u64 },
    Scrub { ns: Vec<u8> },
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..6, 0u64..200).prop_map(|(n, not_before)| Op::Enqueue { n, not_before }),
        (0u64..400, 1u64..150, 1usize..5).prop_map(|(now, lease, limit)| Op::Claim {
            now,
            lease,
            limit
        }),
        (0u8..6).prop_map(|n| Op::Complete { n }),
        (0u8..6, 0u64..400).prop_map(|(n, not_before)| Op::Reschedule { n, not_before }),
        (0u8..6).prop_map(|n| Op::DeadLetter { n }),
        Just(Op::Release),
        (0u64..400).prop_map(|before| Op::Purge { before }),
        proptest::collection::vec(0u8..6, 0..4).prop_map(|ns| Op::Scrub { ns }),
    ]
}

fn id_of(n: u8) -> OperationId {
    OperationId::derive(EventId::from_uuid(uuid::Uuid::from_bytes([n; 16])), SEND, 0)
}

fn entry_n(n: u8, not_before: u64) -> OutboxEntry {
    let mut e = OutboxEntry::new(
        EventId::from_uuid(uuid::Uuid::from_bytes([n; 16])),
        StreamId::from_name("p"),
        SEND,
        0,
        vec![n],
        Timestamp(not_before),
    )
    .unwrap();
    e.not_before = Timestamp(not_before);
    e
}

async fn apply<O: Outbox>(o: &O, op: &Op, clock: u64) -> String {
    let now = Timestamp(clock);
    match op {
        Op::Enqueue { n, not_before } => format!("{:?}", o.enqueue(entry_n(*n, *not_before)).await),
        Op::Claim { now, lease, limit } => {
            format!("{:?}", o.claim_due(Timestamp(*now), *lease, *limit).await)
        }
        Op::Complete { n } => format!("{:?}", o.complete(id_of(*n), now).await),
        Op::Reschedule { n, not_before } => format!(
            "{:?}",
            o.reschedule(id_of(*n), Timestamp(*not_before), Some("err".into()), now)
                .await
        ),
        Op::DeadLetter { n } => format!("{:?}", o.dead_letter(id_of(*n), "dead".into(), now).await),
        Op::Release => format!("{:?}", o.release_all_in_flight(now).await),
        Op::Purge { before } => format!("{:?}", o.purge_done(Timestamp(*before)).await),
        Op::Scrub { ns } => {
            let ids: Vec<OperationId> = ns.iter().map(|n| id_of(*n)).collect();
            format!("{:?}", o.scrub(&ids, "scrubbed", now).await)
        }
    }
}

async fn run_parity(ops: Vec<Op>) -> Result<(), TestCaseError> {
    let mem = InMemoryOutbox::new();
    let disk = StoolapOutbox::open_in_memory().unwrap();
    for (i, op) in ops.iter().enumerate() {
        let clock = i as u64;
        let a = apply(&mem, op, clock).await;
        let b = apply(&disk, op, clock).await;
        prop_assert_eq!(&a, &b, "diverged on op #{} {:?}", i, op);
    }
    prop_assert_eq!(mem.counts().await.unwrap(), disk.counts().await.unwrap());
    prop_assert_eq!(
        format!("{:?}", mem.list_open(100).await),
        format!("{:?}", disk.list_open(100).await)
    );
    for n in 0..6u8 {
        prop_assert_eq!(
            format!("{:?}", mem.get(id_of(n)).await),
            format!("{:?}", disk.get(id_of(n)).await)
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn durable_outbox_matches_the_reference_table(ops in proptest::collection::vec(op_strategy(), 1..40)) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(run_parity(ops))?;
    }
}

#[tokio::test]
async fn embedded_in_memory_outbox_commits_atomically_too() {
    // Same §14 guarantees on the in-memory store, for parity with the
    // durable one above.
    let store = InMemoryEventStore::new();
    let stream = StreamId::from_name("s");
    let e = event(b"m");
    let mut bad = entry_for(&e, stream);
    bad.payload = vec![0; MAX_OUTBOX_PAYLOAD_BYTES + 1];
    assert!(store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![e.clone()],
            },
            vec![bad],
        )
        .await
        .is_err());
    assert!(store
        .read_log(LocalLogOffset(0), 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(store.outbox().counts().await.unwrap().pending, 0);
}
