//! The spec's own end-to-end acceptance scenarios, run against the real
//! durable backend:
//!
//! - §87 **Recovery Acceptance Test** — "a key production gate."
//! - §80 **Emergency Integration** — an SOS is committed before any
//!   transmission attempt and stays pending until delivered, expired or
//!   cancelled.
//! - §88 **Multi-Device Offline Test** — two devices append while
//!   disconnected, reconnect, and converge with no central clock.
//!
//! Each "process death" below is real in the sense that matters: every
//! in-memory object is dropped and the store is reopened from its file.
//! Nothing carried across the restart except what the files hold.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use siar_domain::DeviceId;
use siar_event_log::backup::{restore_outbox, GuardVerdict, RestoreGuard};
use siar_event_log::clock::{HlcTimestamp, HybridClock};
use siar_event_log::command_flow::{CommandFlow, EffectIntent, EffectPlanner, EventRef};
use siar_event_log::effects::{
    BackoffPolicy, EffectHandler, EffectOutcome, EffectProcessor, NoTransitions,
};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::outbox::{EffectKind, OperationId, Outbox, OutboxEntry, OutboxState};
use siar_event_log::outbox_reconcile::{reconcile_outbox, RepairMode};
use siar_event_log::reassembly::StreamReassembler;
use siar_event_log::replication::{
    next_outbound_batch, OutboundRequest, OwnDevicesOnly, ReplicationScope, ScopeTable,
};
use siar_event_log::{
    AppendRequest, EventId, EventStore, EventTypeId, InMemoryCheckpointStore, LocalLogOffset,
    NewEvent, Projection, ProjectionError, ProjectionId, ProjectionRunner, ReplayMode,
    StoolapEventStore, StoredEvent, StreamId, Timestamp,
};

fn temp(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("siar-accept-{tag}-{}", uuid::Uuid::new_v4()))
        .to_str()
        .unwrap()
        .to_string()
}
fn cleanup(p: &str) {
    let _ = std::fs::remove_dir_all(p);
    let _ = std::fs::remove_file(p);
}

fn ev(ty: EventTypeId, payload: &[u8], cause: Option<EventId>) -> NewEvent {
    NewEvent {
        event_id: EventId::new(),
        event_type: ty,
        schema_version: 1,
        created_at: Timestamp(1),
        origin: EventOrigin::LocalDevice(DeviceId::new()),
        correlation_id: None,
        causation_id: cause,
        payload: payload.to_vec(),
    }
}

// ---------------------------------------------------------------- §87

const QUEUED: EventTypeId = EventTypeId(1);
const DELIVERED: EventTypeId = EventTypeId(2);
const SEND: EffectKind = EffectKind(10);

/// `MessageQueued` obliges a send; `MessageDelivered` (caused by it)
/// discharges it.
struct MsgPlanner;
impl EffectPlanner for MsgPlanner {
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
    fn closes(&self, e: &EventRef<'_>) -> Vec<OperationId> {
        match (e.event_type == DELIVERED, e.causation_id) {
            (true, Some(c)) => vec![OperationId::derive(c, SEND, 0)],
            _ => vec![],
        }
    }
}

/// Projection: the set of message ids that have been queued.
#[derive(Default)]
struct QueuedMessages(Mutex<Vec<Vec<u8>>>);
#[async_trait]
impl Projection for QueuedMessages {
    fn projection_id(&self) -> ProjectionId {
        ProjectionId("queued-messages")
    }
    fn projection_version(&self) -> u16 {
        1
    }
    async fn apply(&self, e: &StoredEvent) -> Result<(), ProjectionError> {
        if e.envelope.event_type == QUEUED {
            self.0.lock().unwrap().push(e.envelope.payload.clone());
        }
        Ok(())
    }
    async fn reset(&self) -> Result<(), ProjectionError> {
        self.0.lock().unwrap().clear();
        Ok(())
    }
}

/// A "network": finds a route, sends, and the *recipient* deduplicates
/// on the operation id it is handed.
struct Network {
    route_found: Mutex<u32>,
    sent_ids: Mutex<Vec<OperationId>>,
    recipient_seen: Mutex<HashSet<OperationId>>,
    recipient_accepted: Mutex<u32>,
}
impl Network {
    fn new() -> Self {
        Self {
            route_found: Mutex::new(0),
            sent_ids: Mutex::new(vec![]),
            recipient_seen: Mutex::new(HashSet::new()),
            recipient_accepted: Mutex::new(0),
        }
    }
}
impl EffectHandler for Network {
    async fn handle(&self, e: &OutboxEntry) -> EffectOutcome {
        *self.route_found.lock().unwrap() += 1; // "route found"
        self.sent_ids.lock().unwrap().push(e.operation_id);
        // Recipient-side dedup (§31).
        if self.recipient_seen.lock().unwrap().insert(e.operation_id) {
            *self.recipient_accepted.lock().unwrap() += 1;
        }
        EffectOutcome::Done
    }
}

#[tokio::test]
async fn recovery_acceptance_test() {
    let log_path = temp("recovery");
    let stream = StreamId::from_name("conversation/alice-bob");
    let message_id_payload = b"message-id-7f3a".to_vec();
    let net = Network::new();

    // -- MessageQueued committed ------------------------------------
    let queued = ev(QUEUED, &message_id_payload, None);
    let queued_id = queued.event_id;
    let expected_op = OperationId::derive(queued_id, SEND, 0);
    {
        let store = StoolapEventStore::open(&log_path).unwrap();
        let flow = CommandFlow::new(&store, &MsgPlanner);
        flow.commit(stream, 0, vec![queued], Timestamp(5))
            .await
            .unwrap();
        // The user has been told "accepted". Nothing has been sent.
        assert_eq!(net.sent_ids.lock().unwrap().len(), 0);
        // -- process killed: everything dropped here --
    }

    // -- restart ------------------------------------------------------
    let store = StoolapEventStore::open(&log_path).unwrap();

    // projection restored
    let projection = QueuedMessages::default();
    let cps = InMemoryCheckpointStore::new();
    ProjectionRunner::new(&store, &cps)
        .catch_up(&projection, 100)
        .await
        .unwrap();
    assert_eq!(
        projection.0.lock().unwrap().as_slice(),
        std::slice::from_ref(&message_id_payload)
    );

    // outbox reconstructed — prove it by destroying the work table
    // first (the harsher case: the log alone must suffice).
    {
        let db = stoolap::Database::open(&format!("file://{log_path}")).unwrap();
        db.execute("DELETE FROM outbox", ()).unwrap();
    }
    let outbox = store.outbox().unwrap();
    assert_eq!(outbox.counts().await.unwrap().pending, 0);
    let rec = reconcile_outbox(
        &store,
        &outbox,
        &MsgPlanner,
        RepairMode::Repair,
        Timestamp(100),
    )
    .await
    .unwrap();
    assert_eq!(rec.missing, vec![expected_op]);

    // route found, same MessageId sent
    let processor = EffectProcessor::new(
        &outbox,
        &net,
        &NoTransitions,
        BackoffPolicy::DEFAULT,
        10_000,
        8,
    );
    processor
        .run_once(ReplayMode::Recovery, Timestamp(200))
        .await
        .unwrap();
    assert_eq!(*net.route_found.lock().unwrap(), 1);
    assert_eq!(
        net.sent_ids.lock().unwrap().as_slice(),
        &[expected_op],
        "the SAME operation id as before the crash"
    );
    assert_eq!(
        outbox.get(expected_op).await.unwrap().unwrap().payload,
        message_id_payload
    );

    // recipient deduplicates — a lost ack means a second delivery of
    // the same operation (simulated by resending it directly).
    net.handle(&outbox.get(expected_op).await.unwrap().unwrap())
        .await;
    assert_eq!(
        net.sent_ids.lock().unwrap().len(),
        2,
        "sent twice on the wire"
    );
    assert_eq!(*net.recipient_accepted.lock().unwrap(), 1, "accepted once");

    // MessageDelivered committed
    let delivered = ev(DELIVERED, b"", Some(queued_id));
    CommandFlow::new(&store, &MsgPlanner)
        .commit(stream, 1, vec![delivered], Timestamp(300))
        .await
        .unwrap();
    // History now says nothing is owed, and the outbox agrees.
    let clean = reconcile_outbox(
        &store,
        &outbox,
        &MsgPlanner,
        RepairMode::DryRun,
        Timestamp(301),
    )
    .await
    .unwrap();
    assert!(clean.is_clean(), "{clean:?}");
    assert_eq!(
        outbox.get(expected_op).await.unwrap().unwrap().state,
        OutboxState::Done
    );

    drop((store, outbox));
    cleanup(&log_path);
}

// ---------------------------------------------------------------- §80

const SOS_CREATED: EventTypeId = EventTypeId(10);
const SOS_CANCELLED: EventTypeId = EventTypeId(11);
const RADIO: EffectKind = EffectKind(20);

struct SosPlanner;
impl EffectPlanner for SosPlanner {
    fn plan(&self, e: &EventRef<'_>) -> Vec<EffectIntent> {
        if e.event_type == SOS_CREATED {
            vec![EffectIntent {
                kind: RADIO,
                slot: 0,
                payload: e.payload.to_vec(),
            }]
        } else {
            vec![]
        }
    }
    fn closes(&self, e: &EventRef<'_>) -> Vec<OperationId> {
        match (e.event_type == SOS_CANCELLED, e.causation_id) {
            (true, Some(c)) => vec![OperationId::derive(c, RADIO, 0)],
            _ => vec![],
        }
    }
}

/// A radio with no connectivity until switched on.
struct Radio {
    up: Mutex<bool>,
    attempts: Mutex<u32>,
}
impl EffectHandler for Radio {
    async fn handle(&self, _e: &OutboxEntry) -> EffectOutcome {
        *self.attempts.lock().unwrap() += 1;
        if *self.up.lock().unwrap() {
            EffectOutcome::Done
        } else {
            EffectOutcome::Retry {
                reason: "no link".into(),
            }
        }
    }
}

async fn sos_scenario(ending: &str) {
    let path = temp("sos");
    let stream = StreamId::from_name("emergency");
    let radio = Radio {
        up: Mutex::new(false),
        attempts: Mutex::new(0),
    };
    let sos = ev(SOS_CREATED, b"SOS: injured at grid 12", None);
    let sos_id = sos.event_id;
    let op = OperationId::derive(sos_id, RADIO, 0);

    {
        let store = StoolapEventStore::open(&path).unwrap();
        CommandFlow::new(&store, &SosPlanner)
            .commit(stream, 0, vec![sos], Timestamp(10))
            .await
            .unwrap();
        // COMMITTED before any radio attempt: the work is durable and
        // the radio has not been touched.
        let outbox = store.outbox().unwrap();
        let row = outbox.get(op).await.unwrap().unwrap();
        assert_eq!((row.state, row.attempts), (OutboxState::Pending, 0));
        assert_eq!(*radio.attempts.lock().unwrap(), 0);

        // The device loses connectivity: every attempt fails.
        let p = EffectProcessor::new(
            &outbox,
            &radio,
            &NoTransitions,
            BackoffPolicy::DEFAULT,
            1_000,
            8,
        );
        p.run_once(ReplayMode::Live, Timestamp(20)).await.unwrap();
        assert!(*radio.attempts.lock().unwrap() >= 1);
        assert_eq!(
            outbox.get(op).await.unwrap().unwrap().state,
            OutboxState::Pending
        );
        // -- restart --
    }

    let store = StoolapEventStore::open(&path).unwrap();
    let outbox = store.outbox().unwrap();
    // Still pending after the restart: nothing lost, nothing marked done.
    let row = outbox.get(op).await.unwrap().unwrap();
    assert!(row.state.is_open(), "SOS must survive restart");
    assert_eq!(row.payload, b"SOS: injured at grid 12");
    // Startup: release any claim the dead process held (§47).
    outbox.release_all_in_flight(Timestamp(30)).await.unwrap();

    match ending {
        "delivered" => {
            *radio.up.lock().unwrap() = true;
            let p = EffectProcessor::new(
                &outbox,
                &radio,
                &NoTransitions,
                BackoffPolicy::DEFAULT,
                1_000,
                8,
            );
            // Far enough in the future to clear the backoff.
            p.run_once(ReplayMode::Recovery, Timestamp(10_000_000))
                .await
                .unwrap();
            assert_eq!(
                outbox.get(op).await.unwrap().unwrap().state,
                OutboxState::Done
            );
        }
        "cancelled" => {
            // The user cancels: a domain event whose cause is the SOS.
            CommandFlow::new(&store, &SosPlanner)
                .commit(
                    stream,
                    1,
                    vec![ev(SOS_CANCELLED, b"", Some(sos_id))],
                    Timestamp(40),
                )
                .await
                .unwrap();
            // Reconciliation closes the stale obligation.
            let r = reconcile_outbox(
                &store,
                &outbox,
                &SosPlanner,
                RepairMode::Repair,
                Timestamp(41),
            )
            .await
            .unwrap();
            assert_eq!(r.stale_closed, vec![op]);
            assert_eq!(
                outbox.get(op).await.unwrap().unwrap().state,
                OutboxState::Done
            );
            // And it will never be transmitted afterwards.
            *radio.up.lock().unwrap() = true;
            let before = *radio.attempts.lock().unwrap();
            let p = EffectProcessor::new(
                &outbox,
                &radio,
                &NoTransitions,
                BackoffPolicy::DEFAULT,
                1_000,
                8,
            );
            p.run_once(ReplayMode::Live, Timestamp(10_000_000))
                .await
                .unwrap();
            assert_eq!(*radio.attempts.lock().unwrap(), before);
        }
        "expired" => {
            // A restore-time guard (§71) expires stale reports: here an
            // SOS older than its validity window is dropped, not sent.
            struct Expiry;
            impl RestoreGuard for Expiry {
                fn assess(&self, _e: &OutboxEntry, _now: Timestamp) -> GuardVerdict {
                    GuardVerdict::Drop { reason: "expired" }
                }
            }
            // Same node loses its outbox (e.g. restored device)...
            {
                let db = stoolap::Database::open(&format!("file://{path}")).unwrap();
                db.execute("DELETE FROM outbox", ()).unwrap();
            }
            let r = restore_outbox(&store, &outbox, &SosPlanner, &Expiry, Timestamp(99_999_999))
                .await
                .unwrap();
            assert_eq!(r.dropped.len(), 1);
            let row = outbox.get(op).await.unwrap().unwrap();
            assert_eq!(row.state, OutboxState::DeadLettered);
            assert_eq!(row.last_error.as_deref(), Some("expired"));
        }
        _ => unreachable!(),
    }
    drop((store, outbox));
    cleanup(&path);
}

#[tokio::test]
async fn sos_is_persisted_before_transmission_and_survives_to_delivery() {
    sos_scenario("delivered").await;
}

#[tokio::test]
async fn a_cancelled_sos_is_closed_by_history_and_never_sent() {
    sos_scenario("cancelled").await;
}

#[tokio::test]
async fn an_expired_sos_is_recorded_as_dropped_not_silently_lost_or_resent() {
    sos_scenario("expired").await;
}

// ---------------------------------------------------------------- §88

const SETTING: EventTypeId = EventTypeId(30);

/// payload = klen(1) | key | value-len(1) | value | hlc(28)
fn setting(key: &str, value: &str, at: &HlcTimestamp) -> Vec<u8> {
    let mut p = vec![key.len() as u8];
    p.extend(key.as_bytes());
    p.push(value.len() as u8);
    p.extend(value.as_bytes());
    p.extend(at.to_bytes());
    p
}

fn parse_setting(p: &[u8]) -> (String, String, HlcTimestamp) {
    let k = p[0] as usize;
    let key = String::from_utf8(p[1..1 + k].to_vec()).unwrap();
    let v = p[1 + k] as usize;
    let value = String::from_utf8(p[2 + k..2 + k + v].to_vec()).unwrap();
    let hlc = HlcTimestamp::from_bytes(&p[2 + k + v..]).unwrap();
    (key, value, hlc)
}

async fn append_setting(
    store: &StoolapEventStore,
    dev: DeviceId,
    stream: StreamId,
    version: u64,
    payload: Vec<u8>,
) {
    let mut e = ev(SETTING, &payload, None);
    e.origin = EventOrigin::LocalDevice(dev);
    store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: version,
            events: vec![e],
        })
        .await
        .unwrap();
}

/// The DOMAIN conflict rule (§54): last-writer-wins by hybrid logical
/// clock. The event store merely records both facts.
async fn resolved_state(
    store: &StoolapEventStore,
    streams: &[StreamId],
) -> BTreeMap<String, String> {
    let mut best: BTreeMap<String, (HlcTimestamp, String)> = BTreeMap::new();
    for s in streams {
        for e in store.read_stream(*s, 0, 1000).await.unwrap() {
            let (k, v, t) = parse_setting(&e.envelope.payload);
            match best.get(&k) {
                Some((cur, _)) if *cur >= t => {}
                _ => {
                    best.insert(k, (t, v));
                }
            }
        }
    }
    best.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

/// Replicates `from_store`'s `stream` to `to_store`, delivering each
/// batch reversed and with every event duplicated (a hostile network).
async fn sync(
    from: (&StoolapEventStore, DeviceId),
    to: (&StoolapEventStore, DeviceId),
    stream: StreamId,
    rx: &mut StreamReassembler,
) {
    let scopes = ScopeTable::new().with(SETTING, ReplicationScope::OwnDevices);
    let target = to.1;
    let auth = OwnDevicesOnly(move |d| d == target);
    let batch = next_outbound_batch(
        from.0,
        &scopes,
        &auth,
        &OutboundRequest {
            local_device: from.1,
            peer: to.1,
            stream,
            acknowledged_version: 0,
            limit: 100,
        },
    )
    .await
    .unwrap();
    assert!(!batch.has_version_holes);
    for w in batch.events.iter().rev() {
        rx.receive(to.0, w.clone()).await.unwrap();
        rx.receive(to.0, w.clone()).await.unwrap(); // duplicate delivery
    }
}

#[tokio::test]
async fn two_offline_devices_converge_with_no_central_clock() {
    let phone = DeviceId::new();
    let laptop = DeviceId::new();
    let phone_stream = StreamId::from_name("settings/writer/phone");
    let laptop_stream = StreamId::from_name("settings/writer/laptop");
    let phone_store = StoolapEventStore::open_in_memory().unwrap();
    let laptop_store = StoolapEventStore::open_in_memory().unwrap();

    // Each device has its OWN clock — and the laptop's is 5 seconds
    // behind. No shared time source exists anywhere in this test.
    let phone_clock = HybridClock::new(phone);
    let laptop_clock = HybridClock::new(laptop);

    // -- both offline, appending independently ------------------------
    let t1 = phone_clock.now(100_000).unwrap();
    append_setting(
        &phone_store,
        phone,
        phone_stream,
        0,
        setting("theme", "dark", &t1),
    )
    .await;
    let t2 = phone_clock.now(100_050).unwrap();
    append_setting(
        &phone_store,
        phone,
        phone_stream,
        1,
        setting("language", "fr", &t2),
    )
    .await;

    let t3 = laptop_clock.now(95_000).unwrap(); // laptop clock is behind
    append_setting(
        &laptop_store,
        laptop,
        laptop_stream,
        0,
        setting("theme", "light", &t3),
    )
    .await;
    let t4 = laptop_clock.now(95_020).unwrap();
    append_setting(
        &laptop_store,
        laptop,
        laptop_stream,
        1,
        setting("font", "large", &t4),
    )
    .await;

    // Before reconnecting, they disagree.
    let both = [phone_stream, laptop_stream];
    let phone_before = resolved_state(&phone_store, &both).await;
    let laptop_before = resolved_state(&laptop_store, &both).await;
    assert_ne!(phone_before, laptop_before);
    assert_eq!(phone_before["theme"], "dark");
    assert_eq!(laptop_before["theme"], "light");

    // -- reconnect: authorized events replicate both ways -------------
    let mut rx_on_laptop = StreamReassembler::default();
    let mut rx_on_phone = StreamReassembler::default();
    sync(
        (&phone_store, phone),
        (&laptop_store, laptop),
        phone_stream,
        &mut rx_on_laptop,
    )
    .await;
    sync(
        (&laptop_store, laptop),
        (&phone_store, phone),
        laptop_stream,
        &mut rx_on_phone,
    )
    .await;

    // -- domain conflict rules apply; both converge --------------------
    let phone_after = resolved_state(&phone_store, &both).await;
    let laptop_after = resolved_state(&laptop_store, &both).await;
    assert_eq!(phone_after, laptop_after, "converged");
    assert_eq!(phone_after.len(), 3, "union of independent settings");
    assert_eq!(phone_after["language"], "fr");
    assert_eq!(phone_after["font"], "large");
    // The conflict on `theme` is decided by the HLC alone: the phone's
    // timestamp is the greater one, so BOTH devices land on "dark".
    // (Not "whichever was really last" — the laptop's clock was behind;
    // what matters is that every replica reaches the same answer.)
    assert!(t1 > t3);
    assert_eq!(phone_after["theme"], "dark");

    // Nothing was lost or duplicated by the hostile delivery.
    for (store, stream, n) in [
        (&laptop_store, phone_stream, 2u64),
        (&phone_store, laptop_stream, 2u64),
    ] {
        assert_eq!(store.stream_head(stream).await.unwrap(), n);
    }
    // Replicated events are labelled as remote on the receiving side.
    let remote = laptop_store.read_stream(phone_stream, 0, 10).await.unwrap();
    assert!(remote
        .iter()
        .all(|e| e.envelope.origin == EventOrigin::RemoteDevice(phone)));
    let _ = LocalLogOffset(0);
}
