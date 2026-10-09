//! §49-53 end to end on the real `stoolap` backend: device A replicates
//! a single-writer stream to its own device B — scope-filtered,
//! authorizer-gated, delivered out of order and with lost acks — and B
//! converges on exactly the replicable subset, with cursors advancing
//! only on acknowledgement.

use siar_domain::DeviceId;
use siar_event_log::reassembly::{ReceiveOutcome, StreamReassembler};
use siar_event_log::replication::{
    next_outbound_batch, OutboundRequest, OwnDevicesOnly, ReplicationScope, ScopeTable,
};
use siar_event_log::sync_cursor::{CursorStore, StoolapCursorStore};
use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventTypeId, NewEvent, StoolapEventStore,
    StreamId, Timestamp,
};

const MSG: EventTypeId = EventTypeId(1);
const DIAG: EventTypeId = EventTypeId(2);

fn ev(ty: EventTypeId, tag: u8, author: DeviceId) -> NewEvent {
    NewEvent {
        event_id: EventId::new(),
        event_type: ty,
        schema_version: 1,
        created_at: Timestamp(tag as u64),
        origin: EventOrigin::LocalDevice(author),
        correlation_id: None,
        causation_id: None,
        payload: vec![tag],
    }
}

#[tokio::test]
async fn own_device_sync_converges_despite_reordering_and_lost_acks() {
    let a = DeviceId::new();
    let b = DeviceId::new();
    // Layout rule: replicated events and local-only diagnostics live in
    // different streams.
    let state = StreamId::from_name("device/a/state");
    let diag = StreamId::from_name("device/a/diagnostics");

    let store_a = StoolapEventStore::open_in_memory().unwrap();
    store_a
        .append(AppendRequest {
            stream_id: state,
            expected_version: 0,
            events: (0..10).map(|i| ev(MSG, i, a)).collect(),
        })
        .await
        .unwrap();
    store_a
        .append(AppendRequest {
            stream_id: diag,
            expected_version: 0,
            events: (0..5).map(|i| ev(DIAG, i, a)).collect(),
        })
        .await
        .unwrap();

    let scopes = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
    let auth = OwnDevicesOnly(move |d| d == b);
    let cursors = StoolapCursorStore::open_in_memory().unwrap();
    let store_b = StoolapEventStore::open_in_memory().unwrap();
    let mut rx = StreamReassembler::default();

    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 20, "replication must terminate");
        let acked = cursors.get(b, state).await.unwrap().acknowledged_version;
        let batch = next_outbound_batch(
            &store_a,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: a,
                peer: b,
                stream: state,
                acknowledged_version: acked,
                limit: 3,
            },
        )
        .await
        .unwrap();
        assert!(!batch.has_version_holes);
        if batch.events.is_empty() {
            break;
        }
        // Network reorders each batch.
        for w in batch.events.iter().rev() {
            rx.receive(&store_b, w.clone()).await.unwrap();
        }
        // Round 2's acknowledgement is lost: the next round resends.
        if rounds != 2 {
            let head = store_b.stream_head(state).await.unwrap();
            cursors.acknowledge(b, state, head).await.unwrap();
        }
    }

    assert_eq!(
        cursors.get(b, state).await.unwrap().acknowledged_version,
        10
    );
    let on_b = store_b.read_stream(state, 0, 100).await.unwrap();
    let on_a = store_a.read_stream(state, 0, 100).await.unwrap();
    assert_eq!(on_b.len(), 10);
    for (x, y) in on_a.iter().zip(&on_b) {
        assert_eq!(x.envelope.event_id, y.envelope.event_id);
        assert_eq!(x.envelope.payload, y.envelope.payload);
        assert_eq!(y.envelope.origin, EventOrigin::RemoteDevice(a));
    }
    // Diagnostics never left A.
    assert_eq!(store_b.stream_head(diag).await.unwrap(), 0);
}

#[tokio::test]
async fn mixing_local_only_events_into_a_replicated_stream_is_flagged_and_stalls_safely() {
    let a = DeviceId::new();
    let b = DeviceId::new();
    let stream = StreamId::from_name("mixed");
    let store_a = StoolapEventStore::open_in_memory().unwrap();
    store_a
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![ev(MSG, 0, a), ev(DIAG, 1, a), ev(MSG, 2, a)],
        })
        .await
        .unwrap();
    let scopes = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
    let auth = OwnDevicesOnly(move |d| d == b);

    let batch = next_outbound_batch(
        &store_a,
        &scopes,
        &auth,
        &OutboundRequest {
            local_device: a,
            peer: b,
            stream,
            acknowledged_version: 0,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(batch.events.len(), 2);
    assert!(batch.has_version_holes, "version 2 was filtered out");

    // The receiver applies v1, then holds v3 waiting for a v2 that will
    // never come — stalled, but nothing wrong is stored.
    let store_b = StoolapEventStore::open_in_memory().unwrap();
    let mut rx = StreamReassembler::default();
    for w in &batch.events {
        rx.receive(&store_b, w.clone()).await.unwrap();
    }
    assert_eq!(store_b.stream_head(stream).await.unwrap(), 1);
    assert_eq!(rx.pending_gaps().len(), 1);
}

#[tokio::test]
async fn the_cursor_only_moves_on_acknowledgement_so_a_lost_ack_resends() {
    let a = DeviceId::new();
    let b = DeviceId::new();
    let stream = StreamId::from_name("s");
    let store_a = StoolapEventStore::open_in_memory().unwrap();
    store_a
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: (0..4).map(|i| ev(MSG, i, a)).collect(),
        })
        .await
        .unwrap();
    let scopes = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
    let auth = OwnDevicesOnly(move |d| d == b);
    let cursors = StoolapCursorStore::open_in_memory().unwrap();
    let store_b = StoolapEventStore::open_in_memory().unwrap();
    let mut rx = StreamReassembler::default();

    let req = |ack| OutboundRequest {
        local_device: a,
        peer: b,
        stream,
        acknowledged_version: ack,
        limit: 10,
    };

    let first = next_outbound_batch(&store_a, &scopes, &auth, &req(0))
        .await
        .unwrap();
    for w in &first.events {
        rx.receive(&store_b, w.clone()).await.unwrap();
    }
    // ...the ack is lost; A's cursor still says 0, so it resends all 4.
    let again = next_outbound_batch(
        &store_a,
        &scopes,
        &auth,
        &req(cursors.get(b, stream).await.unwrap().acknowledged_version),
    )
    .await
    .unwrap();
    assert_eq!(again.events.len(), 4);
    let mut dupes = 0;
    for w in &again.events {
        if let ReceiveOutcome::Duplicate { .. } = rx.receive(&store_b, w.clone()).await.unwrap() {
            dupes += 1;
        }
    }
    assert_eq!(dupes, 4, "§24 absorbs the retransmission");
    assert_eq!(store_b.stream_head(stream).await.unwrap(), 4);

    cursors
        .acknowledge(b, stream, again.through_version)
        .await
        .unwrap();
    let done = next_outbound_batch(&store_a, &scopes, &auth, &req(4))
        .await
        .unwrap();
    assert!(done.events.is_empty());
}
