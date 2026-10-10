//! Phase 7 on the real `stoolap` backend: redaction is durable and
//! keeps the §22 integrity check intact, deletion/backup/restore work
//! store-to-store, and snapshots survive reopen.

use siar_domain::DeviceId;
use siar_event_log::backup::{export_since, restore_chunks, BACKUP_GENESIS};
use siar_event_log::deletion::{execute_deletion, DeletionRequest};
use siar_event_log::outbox::{
    EffectKind, OperationId, Outbox, OutboxEntry, OutboxState, TransactionalOutbox,
};
use siar_event_log::redaction::RedactableStore;
use siar_event_log::retention::{
    apply_compaction, plan_compaction, RetentionClass, RetentionPolicy, RetentionTable,
};
use siar_event_log::snapshot::{
    load_state, snapshot_if_due, LoadSource, SnapshotStore, StoolapSnapshotStore, StreamFold,
};
use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventTypeId, LocalLogOffset, NewEvent,
    StoolapEventStore, StoredEvent, StreamId, Timestamp,
};

const NOISE: EventTypeId = EventTypeId(2);
const AUDIT: EventTypeId = EventTypeId(1);

fn ev(ty: EventTypeId, created: u64, payload: &[u8]) -> NewEvent {
    NewEvent {
        event_id: EventId::new(),
        event_type: ty,
        schema_version: 1,
        created_at: Timestamp(created),
        origin: EventOrigin::LocalDevice(DeviceId::new()),
        correlation_id: None,
        causation_id: None,
        payload: payload.to_vec(),
    }
}

fn temp(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("siar-{tag}-{}", uuid::Uuid::new_v4()))
        .to_str()
        .unwrap()
        .to_string()
}

fn cleanup(p: &str) {
    let _ = std::fs::remove_dir_all(p);
    let _ = std::fs::remove_file(p);
}

#[tokio::test]
async fn redaction_is_durable_and_the_row_still_verifies_after_reopen() {
    let path = temp("redact");
    let stream = StreamId::from_name("s");
    let secret = ev(NOISE, 0, b"top secret");
    let keep = ev(AUDIT, 0, b"audit detail");
    {
        let store = StoolapEventStore::open(&path).unwrap();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![secret.clone(), keep.clone()],
            })
            .await
            .unwrap();
        let r = store.redact_payloads(&[secret.event_id]).await.unwrap();
        assert_eq!(r.redacted, 1);
        assert_eq!(
            store
                .redact_payloads(&[secret.event_id, EventId::new()])
                .await
                .unwrap()
                .already_redacted,
            1
        );
    }
    {
        // A fresh open re-verifies every row's checksum on read: a
        // redacted row that failed it would surface as `Corrupt`.
        let store = StoolapEventStore::open(&path).unwrap();
        let got = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(got[0].envelope.payload.is_empty());
        assert_eq!(got[0].envelope.stream_version, 1);
        assert_eq!(got[1].envelope.payload, b"audit detail");
        assert!(store.is_redacted(secret.event_id).await.unwrap());
        assert!(!store.is_redacted(keep.event_id).await.unwrap());
        assert_eq!(store.stream_head(stream).await.unwrap(), 2);
        // §24: the tombstoned id still counts as seen.
        let r = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 2,
                events: vec![secret],
            })
            .await
            .unwrap();
        assert_eq!(r.local_offsets, vec![None]);
    }
    cleanup(&path);
}

#[tokio::test]
async fn compaction_and_deletion_work_end_to_end_on_disk() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let outbox = store.outbox().unwrap();
    let snaps = StoolapSnapshotStore::open_in_memory().unwrap();
    let stream = StreamId::from_name("s");
    let old_noise = ev(NOISE, 0, b"operational");
    let msg = ev(AUDIT, 0, b"private message");
    let entry = OutboxEntry::new(
        msg.event_id,
        stream,
        EffectKind(1),
        0,
        b"private message".to_vec(),
        Timestamp(0),
    )
    .unwrap();
    let op = entry.operation_id;
    store
        .append_with_outbox(
            AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![old_noise.clone(), msg.clone()],
            },
            vec![entry],
        )
        .await
        .unwrap();

    // Compaction: only the aged operational event.
    let table = RetentionTable::new().with(NOISE, RetentionClass::OperationalShortTerm);
    let plan = plan_compaction(
        &store,
        &table,
        &RetentionPolicy::default(),
        Timestamp(30 * 24 * 3_600_000),
        LocalLogOffset(0),
        100,
        |_| false,
    )
    .await
    .unwrap();
    assert_eq!(plan.candidates, vec![old_noise.event_id]);
    assert_eq!(apply_compaction(&store, &plan).await.unwrap().redacted, 1);

    // Deletion of the private message across log + snapshot + outbox.
    snaps
        .save(siar_event_log::snapshot::Snapshot::new(
            stream,
            2,
            1,
            b"derived".to_vec(),
        ))
        .await
        .unwrap();
    let report = execute_deletion(
        &store,
        &snaps,
        &outbox,
        &DeletionRequest {
            events: vec![msg.event_id],
            streams: vec![stream],
            operations: vec![op],
        },
        Timestamp(99),
    )
    .await
    .unwrap();
    assert_eq!(
        (
            report.redaction.redacted,
            report.snapshots_purged,
            report.outbox_scrubbed
        ),
        (1, 1, 1)
    );

    let row = outbox.get(op).await.unwrap().unwrap();
    assert!(row.payload.is_empty());
    assert_eq!(row.state, OutboxState::DeadLettered);
    assert!(snaps.load_latest(stream).await.unwrap().is_none());
    let log = store.read_stream(stream, 0, 10).await.unwrap();
    assert!(log.iter().all(|e| e.envelope.payload.is_empty()));
    assert_eq!(log.len(), 2, "history shape preserved");
    let _ = OperationId::derive(msg.event_id, EffectKind(1), 0);
}

#[tokio::test]
async fn backup_from_one_stoolap_store_restores_into_another() {
    let src = StoolapEventStore::open_in_memory().unwrap();
    let a = StreamId::from_name("a");
    let b = StreamId::from_name("b");
    let mut va = 0;
    let mut vb = 0;
    for i in 0..6u8 {
        let (s, v) = if i % 2 == 0 {
            (a, &mut va)
        } else {
            (b, &mut vb)
        };
        src.append(AppendRequest {
            stream_id: s,
            expected_version: *v,
            events: vec![ev(AUDIT, 0, &[i])],
        })
        .await
        .unwrap();
        *v += 1;
    }

    let c1 = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 4)
        .await
        .unwrap()
        .unwrap();
    let c2 = export_since(&src, c1.last_offset, c1.digest, 4)
        .await
        .unwrap()
        .unwrap();
    assert!(export_since(&src, c2.last_offset, c2.digest, 4)
        .await
        .unwrap()
        .is_none());

    let dst = StoolapEventStore::open_in_memory().unwrap();
    let r = restore_chunks(&dst, &BACKUP_GENESIS, &[c1, c2])
        .await
        .unwrap();
    assert_eq!(r.restored, 6);
    for s in [a, b] {
        let x: Vec<StoredEvent> = src.read_stream(s, 0, 10).await.unwrap();
        let y: Vec<StoredEvent> = dst.read_stream(s, 0, 10).await.unwrap();
        assert_eq!(x.len(), y.len());
        for (p, q) in x.iter().zip(&y) {
            assert_eq!(p.envelope, q.envelope);
        }
    }
}

struct Count;
impl StreamFold for Count {
    type State = u64;
    const PROJECTION_VERSION: u16 = 1;
    fn initial(&self) -> u64 {
        0
    }
    fn apply(&self, s: &mut u64, _e: &StoredEvent) {
        *s += 1;
    }
    fn encode(&self, s: &u64) -> Vec<u8> {
        s.to_le_bytes().to_vec()
    }
    fn decode(&self, b: &[u8]) -> Option<u64> {
        Some(u64::from_le_bytes(b.try_into().ok()?))
    }
}

#[tokio::test]
async fn snapshots_survive_reopen_and_only_the_tail_is_replayed() {
    let log_path = temp("snaplog");
    let snap_path = temp("snapdb");
    let stream = StreamId::from_name("s");
    {
        let store = StoolapEventStore::open(&log_path).unwrap();
        let snaps = StoolapSnapshotStore::open(&snap_path).unwrap();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: (0..20).map(|i| ev(AUDIT, 0, &[i])).collect(),
            })
            .await
            .unwrap();
        let l = load_state(&store, &snaps, &Count, stream).await.unwrap();
        assert_eq!((l.state, l.replayed), (20, 20));
        assert!(snapshot_if_due(&snaps, &Count, stream, &l, 0, 10)
            .await
            .unwrap());
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 20,
                events: vec![ev(AUDIT, 0, b"x"), ev(AUDIT, 0, b"y")],
            })
            .await
            .unwrap();
    }
    {
        let store = StoolapEventStore::open(&log_path).unwrap();
        let snaps = StoolapSnapshotStore::open(&snap_path).unwrap();
        let l = load_state(&store, &snaps, &Count, stream).await.unwrap();
        assert_eq!(l.state, 22);
        assert_eq!(l.source, LoadSource::Snapshot { at: 20 });
        assert_eq!(l.replayed, 2);
    }
    cleanup(&log_path);
    cleanup(&snap_path);
}
