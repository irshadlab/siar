//! `siar-messaging`'s event-log policies (`event_policy`) exercised
//! end to end against real `MessagingEvent`s and both real event-store
//! backends — no `MessageService`, no network: the policies are pure
//! and must be provable without them.

use std::collections::HashSet;

use siar_domain::{ConversationId, DeviceId, MessageId};
use siar_event_log::backup::{export_since, restore_chunks, restore_outbox, BACKUP_GENESIS};
use siar_event_log::command_flow::{CommandFlow, EventRef};
use siar_event_log::effects::{
    BackoffPolicy, EffectHandler, EffectOutcome, EffectProcessor, NoTransitions,
};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{EventId, LocalLogOffset, Timestamp};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::outbox::{OperationId, Outbox, OutboxEntry, OutboxState};
use siar_event_log::outbox_reconcile::{reconcile_outbox, RepairMode};
use siar_event_log::redaction::RedactableStore;
use siar_event_log::replication::{
    next_outbound_batch, OutboundRequest, OwnDevicesOnly, ReplicationScope,
};
use siar_event_log::retention::{plan_compaction, RetentionClass, RetentionPolicy};
use siar_event_log::snapshot::{InMemorySnapshotStore, Snapshot, SnapshotStore};
use siar_event_log::unknown::{triage, Disposition};
use siar_event_log::upcast::UpcastError;
use siar_event_log::{
    AppendRequest, EventStore, EventStoreError, EventTypeId, InMemoryEventStore, NewEvent,
    ReplayMode, StoolapEventStore, StreamId,
};
use siar_messaging::event_policy::{
    delete_message_content, identity_conversation_stream_id, local_origin, messaging_event_limits,
    messaging_retention_table, messaging_scope_table, messaging_unknown_policy,
    messaging_upcasters, replication_stream_id, MessageStatus, MessagingEffectPlanner,
    MessagingRestoreGuard, SendMessageIntent, ALL_MESSAGING_EVENT_TYPES, EFFECT_SEND_MESSAGE,
};
use siar_messaging::{
    conversation_stream_id, MessagingEvent, EVENT_TYPE_MESSAGE_CREATED,
    EVENT_TYPE_MESSAGE_DELIVERED, EVENT_TYPE_MESSAGE_QUEUED, EVENT_TYPE_MESSAGE_READ,
    EVENT_TYPE_REACTION_ADDED,
};

struct Ids {
    conv: ConversationId,
    dev: DeviceId,
}

fn ids() -> Ids {
    Ids {
        conv: ConversationId::new(),
        dev: DeviceId::new(),
    }
}

fn created(i: &Ids, m: MessageId, text: &[u8]) -> NewEvent {
    MessagingEvent::MessageCreated {
        conversation_id: i.conv,
        message_id: m,
        sender_device: i.dev,
        sequence: 1,
        ciphertext: text.to_vec(),
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, None)
}

fn queued(i: &Ids, m: MessageId, cause: EventId) -> NewEvent {
    MessagingEvent::MessageQueued {
        conversation_id: i.conv,
        message_id: m,
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, Some(cause))
}

fn delivered(i: &Ids, m: MessageId, cause: EventId) -> NewEvent {
    MessagingEvent::MessageDelivered {
        conversation_id: i.conv,
        message_id: m,
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, Some(cause))
}

/// Commits Created -> Queued for a fresh message; returns the Queued
/// event id (what a later Delivered names as its cause) and the next
/// stream version.
async fn send<S: siar_event_log::outbox::TransactionalOutbox>(
    store: &S,
    i: &Ids,
    text: &[u8],
    version: u64,
    now: u64,
) -> (MessageId, EventId, u64) {
    let flow = CommandFlow::new(store, &MessagingEffectPlanner);
    let m = MessageId::new();
    let c = created(i, m, text);
    let q = queued(i, m, c.event_id);
    let qid = q.event_id;
    let r = flow
        .commit(
            conversation_stream_id(i.conv),
            version,
            vec![c, q],
            Timestamp(now),
        )
        .await
        .unwrap();
    (m, qid, r.append.new_version)
}

#[tokio::test]
async fn queueing_a_message_commits_its_send_obligation_atomically() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let (m, qid, _) = send(&store, &i, b"hello", 0, 10).await;

    let op = OperationId::derive(qid, EFFECT_SEND_MESSAGE, 0);
    let row = store.outbox().get(op).await.unwrap().unwrap();
    assert_eq!(row.state, OutboxState::Pending);
    let intent: SendMessageIntent = postcard::from_bytes(&row.payload).unwrap();
    assert_eq!((intent.conversation_id, intent.message_id), (i.conv, m));
    assert!(
        !row.payload.windows(5).any(|w| w == b"hello"),
        "the outbox carries pointers, never the ciphertext (§15)"
    );
    // MessageCreated planned nothing; only Queued did.
    assert_eq!(store.outbox().counts().await.unwrap().pending, 1);
}

struct Sender(std::sync::atomic::AtomicU32);
impl EffectHandler for Sender {
    async fn handle(&self, _e: &OutboxEntry) -> EffectOutcome {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        EffectOutcome::Done
    }
}

#[tokio::test]
async fn the_processor_sends_each_queued_message_once_and_never_on_replay() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let mut v = 0;
    for n in 0..3u8 {
        let (_, _, nv) = send(&store, &i, &[n], v, 1).await;
        v = nv;
    }
    let handler = Sender(Default::default());
    let outbox = store.outbox();
    let p = EffectProcessor::new(
        &outbox,
        &handler,
        &NoTransitions,
        BackoffPolicy::DEFAULT,
        10_000,
        8,
    );

    p.run_once(ReplayMode::ProjectionOnly, Timestamp(5))
        .await
        .unwrap();
    assert_eq!(handler.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    p.run_once(ReplayMode::Live, Timestamp(5)).await.unwrap();
    assert_eq!(handler.0.load(std::sync::atomic::Ordering::SeqCst), 3);
    p.run_once(ReplayMode::Live, Timestamp(5_000_000))
        .await
        .unwrap();
    assert_eq!(handler.0.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn delivered_discharges_the_send_so_a_lost_outbox_is_not_rebuilt_for_it() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let (m1, q1, v) = send(&store, &i, b"one", 0, 10).await;
    let (_m2, q2, v) = send(&store, &i, b"two", v, 10).await;
    store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: v,
            events: vec![delivered(&i, m1, q1)],
        })
        .await
        .unwrap();

    // Lose the whole work table.
    let fresh = siar_event_log::outbox::InMemoryOutbox::new();
    let r = reconcile_outbox(
        &store,
        &fresh,
        &MessagingEffectPlanner,
        RepairMode::Repair,
        Timestamp(99),
    )
    .await
    .unwrap();
    assert_eq!(
        r.missing,
        vec![OperationId::derive(q2, EFFECT_SEND_MESSAGE, 0)]
    );
    assert_eq!(fresh.counts().await.unwrap().pending, 1);
}

#[tokio::test]
async fn restore_vets_each_send_against_current_message_state() {
    let src = StoolapEventStore::open_in_memory().unwrap();
    let i = ids();
    let mut v = 0;
    let mut ms = Vec::new();
    for n in 0..5u8 {
        let (m, _, nv) = send(&src, &i, &[n], v, 10).await;
        ms.push(m);
        v = nv;
    }
    let chunk = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 100)
        .await
        .unwrap()
        .unwrap();

    // Restore into a brand-new device: log first, then the outbox.
    let dst = StoolapEventStore::open_in_memory().unwrap();
    restore_chunks(&dst, &BACKUP_GENESIS, std::slice::from_ref(&chunk))
        .await
        .unwrap();

    let (delivered_since, deleted_since, revoked): (
        HashSet<MessageId>,
        HashSet<MessageId>,
        HashSet<MessageId>,
    ) = ([ms[0]].into(), [ms[1]].into(), [ms[2]].into());
    let guard = MessagingRestoreGuard::new(move |_c, m| {
        if delivered_since.contains(&m) {
            MessageStatus::Delivered
        } else if deleted_since.contains(&m) {
            MessageStatus::Deleted
        } else if revoked.contains(&m) {
            MessageStatus::RecipientRevoked
        } else {
            MessageStatus::Pending
        }
    });

    let outbox = dst.outbox().unwrap();
    let report = restore_outbox(
        &dst,
        &outbox,
        &MessagingEffectPlanner,
        &guard,
        Timestamp(1_000),
    )
    .await
    .unwrap();
    assert_eq!(
        report.enqueued.len(),
        2,
        "only the two still-pending messages"
    );
    assert_eq!(report.dropped.len(), 3);
    let reasons: HashSet<&str> = report.dropped.iter().map(|(_, r)| *r).collect();
    assert_eq!(
        reasons,
        HashSet::from(["already delivered", "message deleted", "recipient revoked"])
    );
    // What will actually be sent: exactly the pending two.
    let due = outbox
        .claim_due(Timestamp(1_000), 10_000, 100)
        .await
        .unwrap();
    assert_eq!(due.len(), 2);
    let sent_msgs: HashSet<MessageId> = due
        .iter()
        .map(|e| {
            postcard::from_bytes::<SendMessageIntent>(&e.payload)
                .unwrap()
                .message_id
        })
        .collect();
    assert_eq!(sent_msgs, HashSet::from([ms[3], ms[4]]));
}

#[tokio::test]
async fn deleting_a_message_removes_its_ciphertext_everywhere_the_log_owns_a_copy() {
    let store = StoolapEventStore::open_in_memory().unwrap();
    let snaps = InMemorySnapshotStore::new();
    let i = ids();
    let (secret, q_secret, v) = send(&store, &i, b"private ciphertext", 0, 1).await;
    let (_keep, q_keep, _) = send(&store, &i, b"someone else's message", v, 1).await;
    let stream = conversation_stream_id(i.conv);
    snaps
        .save(Snapshot::new(
            stream,
            4,
            1,
            b"summary containing derived text".to_vec(),
        ))
        .await
        .unwrap();

    let outbox = store.outbox().unwrap();
    let report = delete_message_content(&store, &snaps, &outbox, i.conv, secret, Timestamp(50))
        .await
        .unwrap();
    assert_eq!(report.redaction.redacted, 1, "just the Created ciphertext");
    assert_eq!(report.snapshots_purged, 1);
    assert_eq!(report.outbox_scrubbed, 1);
    assert_eq!(report.affected_streams, vec![stream]);

    let all = store.read_stream(stream, 0, 100).await.unwrap();
    assert_eq!(all.len(), 4, "history shape preserved");
    let texts: Vec<&[u8]> = all.iter().map(|e| e.envelope.payload.as_slice()).collect();
    let has = |needle: &[u8]| {
        texts
            .iter()
            .any(|t| t.windows(needle.len()).any(|w| w == needle))
    };
    assert!(!has(b"private ciphertext"));
    assert!(has(b"someone else's message"), "other messages untouched");

    let secret_op = outbox
        .get(OperationId::derive(q_secret, EFFECT_SEND_MESSAGE, 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(secret_op.state, OutboxState::DeadLettered);
    assert!(secret_op.payload.is_empty());
    let keep_op = outbox
        .get(OperationId::derive(q_keep, EFFECT_SEND_MESSAGE, 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keep_op.state, OutboxState::Pending);
    assert!(snaps.load_latest(stream).await.unwrap().is_none());
}

#[tokio::test]
async fn deletion_leaves_edits_attributed_to_the_right_message_only() {
    let store = InMemoryEventStore::new();
    let snaps = InMemorySnapshotStore::new();
    let i = ids();
    let (m, _, v) = send(&store, &i, b"original", 0, 1).await;
    let (other, _, v) = send(&store, &i, b"other", v, 1).await;
    let edit = |msg, text: &[u8]| {
        MessagingEvent::MessageEdited {
            conversation_id: i.conv,
            message_id: msg,
            new_ciphertext: text.to_vec(),
        }
        .into_new_event(EventId::new(), local_origin(i.dev), None, None)
    };
    store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: v,
            events: vec![edit(m, b"edited secret"), edit(other, b"edited other")],
        })
        .await
        .unwrap();

    let r = delete_message_content(&store, &snaps, &store.outbox(), i.conv, m, Timestamp(9))
        .await
        .unwrap();
    assert_eq!(r.redaction.redacted, 2, "its Created and its Edited");
    let all = store
        .read_stream(conversation_stream_id(i.conv), 0, 100)
        .await
        .unwrap();
    let blob: Vec<u8> = all
        .iter()
        .flat_map(|e| e.envelope.payload.clone())
        .collect();
    let has = |n: &[u8]| blob.windows(n.len()).any(|w| w == n);
    assert!(!has(b"original") && !has(b"edited secret"));
    assert!(has(b"other") && has(b"edited other"));
}

#[tokio::test]
async fn retention_compacts_old_delivery_noise_and_never_user_content() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let (m, qid, v) = send(&store, &i, b"keep me", 0, 0).await;
    store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: v,
            events: vec![
                delivered(&i, m, qid),
                MessagingEvent::MessageRead {
                    conversation_id: i.conv,
                    message_id: m,
                    reader_device: i.dev,
                }
                .into_new_event(EventId::new(), local_origin(i.dev), None, None),
            ],
        })
        .await
        .unwrap();

    let table = messaging_retention_table();
    for t in ALL_MESSAGING_EVENT_TYPES {
        // Every messaging type is explicitly classified — none relies
        // on the keep-by-default fallback by accident.
        let _ = table.class_of(t);
    }
    assert_eq!(
        table.class_of(EVENT_TYPE_MESSAGE_CREATED),
        RetentionClass::UserControlled
    );

    let plan = plan_compaction(
        &store,
        &table,
        &RetentionPolicy::default(),
        // Events are stamped with the real clock (`into_new_event`), so
        // "a year later" must be real-now plus a year.
        Timestamp(Timestamp::now().0 + 365 * 24 * 3_600_000),
        LocalLogOffset(0),
        100,
        |_| false,
    )
    .await
    .unwrap();
    // Created (user content) is never a candidate; Queued, Delivered
    // and Read are operational noise a year later.
    assert_eq!(plan.candidates.len(), 3);
    let created_id = store.read_log(LocalLogOffset(0), 1).await.unwrap()[0]
        .envelope
        .event_id;
    assert!(!plan.candidates.contains(&created_id));
    let _ = (
        EVENT_TYPE_MESSAGE_QUEUED,
        EVENT_TYPE_MESSAGE_DELIVERED,
        EVENT_TYPE_MESSAGE_READ,
    );
    let _ = store.redact_payloads(&plan.candidates).await.unwrap();
}

#[tokio::test]
async fn every_messaging_event_replicates_to_own_devices_with_no_version_holes() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let phone = DeviceId::new();
    let scopes = messaging_scope_table();
    for t in ALL_MESSAGING_EVENT_TYPES {
        assert_eq!(scopes.scope_of(t), ReplicationScope::OwnDevices, "{t:?}");
    }

    // The single-writer layout: this device's own stream for the conversation.
    let stream = replication_stream_id(i.conv, i.dev);
    let m = MessageId::new();
    let c = created(&i, m, b"hi");
    let q = queued(&i, m, c.event_id);
    let d = delivered(&i, m, q.event_id);
    let reaction = MessagingEvent::ReactionAdded {
        conversation_id: i.conv,
        message_id: m,
        reactor_device: i.dev,
        emoji: "👍".into(),
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, None);
    store
        .append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![c, q, d, reaction],
        })
        .await
        .unwrap();

    let auth = OwnDevicesOnly(move |p| p == phone);
    let batch = next_outbound_batch(
        &store,
        &scopes,
        &auth,
        &OutboundRequest {
            local_device: i.dev,
            peer: phone,
            stream,
            acknowledged_version: 0,
            limit: 100,
        },
    )
    .await
    .unwrap();
    assert_eq!(batch.events.len(), 4);
    assert!(!batch.has_version_holes);
    assert_eq!(batch.through_version, 4);
    // An unrelated device gets nothing.
    let stranger = next_outbound_batch(
        &store,
        &scopes,
        &auth,
        &OutboundRequest {
            local_device: i.dev,
            peer: DeviceId::new(),
            stream,
            acknowledged_version: 0,
            limit: 100,
        },
    )
    .await
    .unwrap();
    assert!(stranger.events.is_empty());
}

#[test]
fn the_shared_conversation_stream_and_the_per_writer_stream_are_different_streams() {
    // The layout finding in event_policy's module doc, pinned: two
    // devices of one account writing into `conversation_stream_id`
    // share a stream (multi-writer); `replication_stream_id` gives each
    // its own.
    let conv = ConversationId::new();
    let (a, b) = (DeviceId::new(), DeviceId::new());
    assert_ne!(
        replication_stream_id(conv, a),
        replication_stream_id(conv, b)
    );
    assert_ne!(replication_stream_id(conv, a), conversation_stream_id(conv));
}

#[test]
fn identities_get_distinct_streams_for_the_same_conversation() {
    let conv = ConversationId::new();
    let personal = NamespaceId::new("personal").unwrap();
    let work = NamespaceId::new("work").unwrap();
    assert_ne!(
        identity_conversation_stream_id(&personal, conv),
        identity_conversation_stream_id(&work, conv)
    );
    assert_ne!(
        identity_conversation_stream_id(&personal, conv),
        conversation_stream_id(conv)
    );
}

#[tokio::test]
async fn an_event_from_a_newer_build_blocks_instead_of_being_misread() {
    let store = InMemoryEventStore::new();
    let i = ids();
    let (_, _, v) = send(&store, &i, b"known", 0, 1).await;
    // A newer peer's event type this build has never seen.
    let future = NewEvent {
        event_id: EventId::new(),
        event_type: EventTypeId(109),
        schema_version: 1,
        created_at: Timestamp(2),
        origin: EventOrigin::RemoteDevice(DeviceId::new()),
        correlation_id: None,
        causation_id: None,
        payload: vec![1, 2, 3],
    };
    store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: v,
            events: vec![future],
        })
        .await
        .unwrap();

    let policy = messaging_unknown_policy();
    assert_eq!(
        policy.disposition(EVENT_TYPE_REACTION_ADDED),
        Disposition::Apply
    );
    assert_eq!(policy.disposition(EventTypeId(109)), Disposition::Block);

    let log = store.read_log(LocalLogOffset(0), 100).await.unwrap();
    let t = triage(&policy, &log);
    assert_eq!(t.apply.len(), 2, "Created + Queued are applied");
    assert_eq!(t.blocked.unwrap().event_type, EventTypeId(109));
    assert_eq!(t.safe_through, Some(LocalLogOffset(2)));
}

#[test]
fn schema_history_is_registered_and_a_future_version_is_refused() {
    let r = messaging_upcasters();
    for t in ALL_MESSAGING_EVENT_TYPES {
        r.verify_chain(t, 1).unwrap();
        assert_eq!(r.upcast(t, 1, b"x").unwrap().steps, 0);
        assert!(matches!(
            r.upcast(t, 2, b"x"),
            Err(UpcastError::FromFuture {
                found: 2,
                latest: 1,
                ..
            })
        ));
    }
}

#[tokio::test]
async fn size_limits_keep_bookkeeping_events_tiny_and_content_events_roomy() {
    let store = InMemoryEventStore::new().with_limits(messaging_event_limits());
    let i = ids();
    let m = MessageId::new();
    // Content event: a 100 KiB ciphertext is fine.
    let big = created(&i, m, &vec![7u8; 100 * 1024]);
    store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: 0,
            events: vec![big],
        })
        .await
        .unwrap();
    // A reaction carrying kilobytes of "emoji" is rejected.
    let abusive = MessagingEvent::ReactionAdded {
        conversation_id: i.conv,
        message_id: m,
        reactor_device: i.dev,
        emoji: "x".repeat(4096),
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, None);
    let err = store
        .append(AppendRequest {
            stream_id: conversation_stream_id(i.conv),
            expected_version: 1,
            events: vec![abusive],
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        EventStoreError::PayloadTooLarge { limit: 1024, .. }
    ));
}

#[test]
fn the_planner_ignores_everything_except_queued_and_closes_only_via_causation() {
    use siar_event_log::command_flow::EffectPlanner;
    let i = ids();
    let m = MessageId::new();
    let c = created(&i, m, b"x");
    fn view(e: &NewEvent) -> EventRef<'_> {
        EventRef::from_new(StreamId::from_name("s"), e)
    }
    assert!(MessagingEffectPlanner.plan(&view(&c)).is_empty());
    let q = queued(&i, m, c.event_id);
    assert_eq!(MessagingEffectPlanner.plan(&view(&q)).len(), 1);

    // A Delivered with no causation (registry lost on restart)
    // discharges nothing — the guard is what covers that case.
    let orphan_delivered = MessagingEvent::MessageDelivered {
        conversation_id: i.conv,
        message_id: m,
    }
    .into_new_event(EventId::new(), local_origin(i.dev), None, None);
    assert!(MessagingEffectPlanner
        .closes(&view(&orphan_delivered))
        .is_empty());
    let d = delivered(&i, m, q.event_id);
    assert_eq!(
        MessagingEffectPlanner.closes(&view(&d)),
        vec![OperationId::derive(q.event_id, EFFECT_SEND_MESSAGE, 0)]
    );
}
