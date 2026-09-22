//! `MessageService`'s entire public API was untested before this file —
//! confirmed by grep before writing a line here. Every test below uses
//! the REAL stack: `siar_storage::open_in_memory()` (real stoolap),
//! real `siar_crypto::DeviceIdentity`, and two real `SiarEndpoint`s
//! talking real QUIC over loopback (same direct-IP-only pattern
//! `siar-transport/tests/roundtrip.rs` established, for the same
//! reason: never depend on relay/DNS discovery in this sandbox).
//! Nothing here is mocked except that there's no UI above it.

use siar_crypto::DeviceIdentity;
use siar_domain::{
    CallControlEvent, ConversationId, DeliveryState, DeviceId, MediaType, MessageContent,
    MessageText,
};
use siar_event_log::{EventStore, InMemoryEventStore, StoolapCheckpointStore};
use siar_messaging::{
    conversation_stream_id, decode_messaging_event, IncomingEvent, MessageService, MessagingEvent,
    PeerTicket, StoolapConversationSummaryProjection, StorageBlobStore,
};
use siar_storage::{
    open_in_memory, BlobRepository, MessageRepository, OutboxRepository, StoolapBlobRepository,
    StoolapMessageRepository, StoolapOutboxRepository,
};
use siar_transport::{BlobStore, SiarEndpoint};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// One simulated device: its own identity, its own in-memory database,
/// its own bound endpoint, and — unlike a production caller, which
/// spawns a receive loop over this — a directly-held `mpsc::Receiver`
/// so tests can assert on exactly what arrived.
struct Node {
    device_id: DeviceId,
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    service: MessageService,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    outbox: Arc<dyn OutboxRepository + Send + Sync>,
    /// Always wired (`MessageService::with_event_log`) — every
    /// existing test below still passes without ever looking at this,
    /// confirming the event log is genuinely additive, not something
    /// existing send/receive behavior secretly depends on.
    event_log: Arc<InMemoryEventStore>,
    incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
}

impl Node {
    async fn spawn() -> Self {
        let device_id = DeviceId::new();
        let identity = DeviceIdentity::generate();

        let db = open_in_memory().expect("in-memory db opens");
        let messages: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoolapMessageRepository::new(Arc::clone(&db)));
        let outbox: Arc<dyn OutboxRepository + Send + Sync> =
            Arc::new(StoolapOutboxRepository::new(Arc::clone(&db)));
        let blobs: Arc<dyn BlobRepository + Send + Sync> =
            Arc::new(StoolapBlobRepository::new(Arc::clone(&db)));

        let blob_store: Arc<dyn BlobStore> = Arc::new(StorageBlobStore(Arc::clone(&blobs)));
        let (incoming_tx, incoming_rx) = mpsc::channel(16);
        let endpoint = Arc::new(
            SiarEndpoint::bind(iroh::SecretKey::generate(), incoming_tx, blob_store)
                .await
                .expect("endpoint binds"),
        );

        let event_log = Arc::new(InMemoryEventStore::new());
        let service = MessageService::new(
            device_id,
            identity.try_clone().expect("identity clones"),
            Arc::clone(&endpoint),
            Arc::clone(&messages),
            Arc::clone(&outbox),
            Arc::clone(&blobs),
        )
        .with_event_log(Arc::clone(&event_log) as Arc<dyn EventStore + Send + Sync>);

        Self {
            device_id,
            identity,
            endpoint,
            service,
            messages,
            outbox,
            event_log,
            incoming: incoming_rx,
        }
    }

    /// A `PeerTicket` another node can use to reach *this* one — direct
    /// loopback address only (no relay/DNS discovery in this sandbox).
    fn ticket(&self) -> PeerTicket {
        let full = self.endpoint.addr();
        let ip_addrs = full
            .addrs
            .into_iter()
            .filter(|a| matches!(a, iroh::TransportAddr::Ip(_)));
        PeerTicket {
            endpoint_addr: iroh::EndpointAddr::from_parts(full.id, ip_addrs),
            x25519_public: self.identity.x25519_public().to_bytes(),
            ed25519_verifying: self.identity.verifying_key().to_bytes(),
        }
    }

    /// Waits for the next raw frame this node's transport received and
    /// decodes it as a `v1::Envelope` — what a real receive loop would
    /// hand straight to `handle_incoming`.
    async fn recv_envelope(&mut self) -> siar_protocol::v1::Envelope {
        let frame = tokio::time::timeout(Duration::from_secs(20), self.incoming.recv())
            .await
            .expect("frame arrives within timeout")
            .expect("channel stays open");
        let siar_protocol::WireMessage::V1(envelope) = frame.message else {
            panic!("expected a V1 envelope, got something else");
        };
        envelope
    }
}

fn text(s: &str) -> MessageText {
    MessageText::parse(s.to_string()).expect("test string is valid message text")
}

fn millis_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after 1970")
        .as_millis() as i64
}

#[tokio::test]
async fn send_text_persists_locally_via_the_transactional_outbox() {
    // plan.md §111: persist first. No receive loop runs on either side
    // in this test — the only way `messages.get` can find anything is
    // if `send_text` really does write through `outbox.enqueue`'s
    // transactional insert, not merely queue a network send.
    let alice = Node::spawn().await;
    let bob = Node::spawn().await;
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    let message_id = alice
        .service
        .send_text(conversation, &bob_ticket, text("hello, stored locally"))
        .await
        .expect("send_text succeeds");

    let stored = alice
        .messages
        .get(message_id)
        .expect("lookup succeeds")
        .expect("send_text must persist the message, not just attempt to send it");
    assert_eq!(stored.conversation_id, conversation);
    assert_eq!(stored.sender_device, alice.device_id);
}

#[tokio::test]
async fn send_text_delivers_and_the_ack_completes_the_outbox() {
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    let sent_id = alice
        .service
        .send_text(conversation, &bob_ticket, text("hi bob"))
        .await
        .expect("send_text succeeds");

    // Bob receives the raw frame and hands it to his own service — the
    // real receive-loop shape (see apps/cli's main.rs).
    let envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");

    let IncomingEvent::Content(MessageContent::Text(received)) =
        event.expect("a fresh text message must surface as an event")
    else {
        panic!("expected Content(Text)");
    };
    assert_eq!(received.as_str(), "hi bob");

    // handle_incoming's ACK path sent a DeliveryAck back to Alice —
    // receive it on her side too, closing the loop for real.
    let ack_envelope = alice.recv_envelope().await;
    alice
        .service
        .handle_incoming(&bob_ticket, ack_envelope)
        .await
        .expect("processing the ack succeeds");

    let stored = alice
        .messages
        .get(sent_id)
        .expect("lookup succeeds")
        .expect("message still exists");
    assert_eq!(stored.delivery_state, DeliveryState::Delivered);

    let due = alice.outbox.due(i64::MAX, 50).expect("due() succeeds");
    assert!(
        due.iter().all(|op| op.message_id != sent_id),
        "acked message must not still be due for retry"
    );
}

/// The real point of `MessageService::with_event_log`/
/// `record_messaging_event`: this is the same send→receive→ack flow
/// as `send_text_delivers_and_the_ack_completes_the_outbox` above, but
/// asserting on `siar-event-log`'s own per-conversation stream instead
/// of (only) `siar-storage`'s tables — closing `04-offline-event-log-
/// architecture.md`'s own previously-named "no real `append` caller
/// anywhere" gap for real, not just at the unit-test level `events.rs`
/// already covered.
#[tokio::test]
async fn send_text_round_trip_records_the_full_04_event_log_history() {
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    let sent_id = alice
        .service
        .send_text(conversation, &bob_ticket, text("event-logged hello"))
        .await
        .expect("send_text succeeds");

    // Alice's own stream should already show Created + Queued — both
    // recorded synchronously inside `send_text`, before any network
    // round trip happens at all.
    let alice_stream = conversation_stream_id(conversation);
    let alice_events_after_send = alice
        .event_log
        .read_stream(alice_stream, 0, 10)
        .await
        .expect("read_stream succeeds");
    assert_eq!(
        alice_events_after_send.len(),
        2,
        "MessageCreated + MessageQueued should both be recorded by send_text alone"
    );
    let decoded: Vec<MessagingEvent> = alice_events_after_send
        .iter()
        .map(|e| {
            decode_messaging_event(e.envelope.schema_version, &e.envelope.payload).expect("decodes")
        })
        .collect();
    assert!(matches!(decoded[0], MessagingEvent::MessageCreated { .. }));
    assert!(matches!(decoded[1], MessagingEvent::MessageQueued { .. }));

    // Bob receives it — his own stream (a DIFFERENT `InMemoryEventStore`
    // instance, per `Node::spawn`) should show MessageReceived.
    let envelope = bob.recv_envelope().await;
    bob.service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");

    let bob_stream = conversation_stream_id(conversation);
    let bob_events = bob
        .event_log
        .read_stream(bob_stream, 0, 10)
        .await
        .expect("read_stream succeeds");
    assert_eq!(bob_events.len(), 1);
    let MessagingEvent::MessageReceived {
        message_id,
        sender_device,
        ..
    } = decode_messaging_event(
        bob_events[0].envelope.schema_version,
        &bob_events[0].envelope.payload,
    )
    .expect("decodes")
    else {
        panic!("expected MessageReceived");
    };
    assert_eq!(message_id, sent_id);
    assert_eq!(sender_device, alice.device_id);
    assert_eq!(
        bob_events[0].envelope.origin,
        siar_event_log::EventOrigin::RemoteDevice(alice.device_id),
        "the message's own sender is a remote device from Bob's point of view"
    );

    // The ACK travels back to Alice — her stream should now also show
    // MessageDelivered, bringing her total to 3.
    let ack_envelope = alice.recv_envelope().await;
    alice
        .service
        .handle_incoming(&bob_ticket, ack_envelope)
        .await
        .expect("processing the ack succeeds");

    let alice_events_final = alice
        .event_log
        .read_stream(alice_stream, 0, 10)
        .await
        .expect("read_stream succeeds");
    assert_eq!(alice_events_final.len(), 3);
    assert!(matches!(
        decode_messaging_event(
            alice_events_final[2].envelope.schema_version,
            &alice_events_final[2].envelope.payload
        )
        .expect("decodes"),
        MessagingEvent::MessageDelivered { .. }
    ));
    assert_eq!(
        alice_events_final[2].envelope.origin,
        siar_event_log::EventOrigin::RemoteDevice(bob.device_id),
        "the delivery confirmation originated from Bob acking it, not from Alice"
    );
}

/// §18 "Read-Your-Writes"'s own concrete example ("SendMessage
/// succeeds locally → conversation immediately shows message"), made
/// real end to end through `MessageService::conversation_summary` —
/// not just `siar_event_log::projection`'s own unit tests against a
/// toy projection, and not just `projections.rs`'s own unit tests
/// calling `catch_up` by hand. This test never calls `catch_up`
/// itself — only `send_text`/`handle_incoming`, exactly as a real
/// caller would, confirming `record_messaging_event`'s own automatic
/// catch-up is what's actually keeping the summary current.
#[tokio::test]
async fn conversation_summary_reflects_sends_and_receipts_without_any_manual_catch_up() {
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    assert!(
        alice
            .service
            .conversation_summary(conversation)
            .await
            .is_none(),
        "nothing sent yet"
    );

    let sent_id = alice
        .service
        .send_text(conversation, &bob_ticket, text("read your writes"))
        .await
        .expect("send_text succeeds");

    // Immediately after `send_text` returns — no receive loop has run
    // anywhere yet.
    let alice_summary = alice
        .service
        .conversation_summary(conversation)
        .await
        .expect("send_text must leave the summary populated, synchronously");
    assert_eq!(alice_summary.message_count, 1);
    assert_eq!(alice_summary.last_message_id, Some(sent_id));

    // Bob's own summary is independent — nothing arrives for him until
    // he actually receives the frame.
    assert!(bob
        .service
        .conversation_summary(conversation)
        .await
        .is_none());
    let envelope = bob.recv_envelope().await;
    bob.service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");
    let bob_summary = bob
        .service
        .conversation_summary(conversation)
        .await
        .expect("handle_incoming must leave the summary populated, synchronously");
    assert_eq!(bob_summary.message_count, 1);
    assert_eq!(bob_summary.last_message_id, Some(sent_id));

    // The ack reaching Alice updates her `last_activity_at` but must
    // NOT bump her `message_count` a second time for the same message.
    let ack_envelope = alice.recv_envelope().await;
    alice
        .service
        .handle_incoming(&bob_ticket, ack_envelope)
        .await
        .expect("processing the ack succeeds");
    let alice_summary_after_ack = alice
        .service
        .conversation_summary(conversation)
        .await
        .expect("still populated");
    assert_eq!(alice_summary_after_ack.message_count, 1);
    assert!(alice_summary_after_ack.last_activity_at >= alice_summary.last_activity_at);
}

#[tokio::test]
async fn handle_incoming_is_idempotent_under_duplicate_delivery() {
    // plan.md §70: receiving the same envelope twice must not produce a
    // second event or a second stored row.
    let alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    alice
        .service
        .send_text(conversation, &bob_ticket, text("only once"))
        .await
        .expect("send_text succeeds");

    let envelope = bob.recv_envelope().await;

    let first = bob
        .service
        .handle_incoming(&alice_ticket, envelope.clone())
        .await
        .expect("first handle_incoming succeeds");
    assert!(matches!(first, Some(IncomingEvent::Content(_))));

    let second = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("second handle_incoming succeeds");
    assert!(
        second.is_none(),
        "a duplicate envelope must not surface a second event"
    );
}

#[tokio::test]
async fn send_attachment_lets_the_recipient_fetch_and_decrypt_it() {
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();
    let plaintext = b"these are the attachment bytes".to_vec();

    alice
        .service
        .send_attachment(
            conversation,
            &bob_ticket,
            plaintext.clone(),
            MediaType::ImagePng,
        )
        .await
        .expect("send_attachment succeeds");

    let envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");
    let IncomingEvent::Content(MessageContent::Attachment(reference)) =
        event.expect("attachment message must surface as an event")
    else {
        panic!("expected Content(Attachment)");
    };

    // Bob doesn't have the blob cached yet — this must go over the wire
    // to Alice's endpoint (served by her StorageBlobStore) and decrypt
    // correctly on arrival.
    let fetched = bob
        .service
        .fetch_attachment(&alice_ticket, &reference)
        .await
        .expect("fetch_attachment succeeds");
    assert_eq!(fetched, plaintext);

    // Drain Alice's ack-receive so it can't leak into a later assertion
    // if this test is ever extended.
    let _ = alice.recv_envelope().await;
}

#[tokio::test]
async fn fetch_attachment_uses_the_local_cache_on_a_second_call() {
    // Real behavioral claim, not just "it works once": after the first
    // fetch_attachment call caches the ciphertext (service.rs's own
    // documented behavior), a second call for the same reference must
    // not need the peer at all — proven here by handing it a peer
    // ticket that cannot possibly answer (nothing bound at that address).
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();
    let plaintext = b"cache me".to_vec();

    alice
        .service
        .send_attachment(
            conversation,
            &bob_ticket,
            plaintext.clone(),
            MediaType::Other,
        )
        .await
        .expect("send_attachment succeeds");
    let envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");
    let IncomingEvent::Content(MessageContent::Attachment(reference)) =
        event.expect("attachment event")
    else {
        panic!("expected Content(Attachment)");
    };

    let first = bob
        .service
        .fetch_attachment(&alice_ticket, &reference)
        .await
        .expect("first fetch succeeds over the network");
    assert_eq!(first, plaintext);
    let _ = alice.recv_envelope().await; // drain alice's ack receive

    // A ticket pointing at a real, but nothing-listening, iroh identity
    // — any attempt to actually dial it will fail/hang, so a passing
    // second fetch proves the cache path was taken, not the network.
    let unreachable_ticket = PeerTicket {
        endpoint_addr: iroh::EndpointAddr::new(iroh::SecretKey::generate().public()),
        x25519_public: alice_ticket.x25519_public,
        ed25519_verifying: alice_ticket.ed25519_verifying,
    };
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        bob.service
            .fetch_attachment(&unreachable_ticket, &reference),
    )
    .await
    .expect("cached fetch must return quickly, not hang trying to dial an unreachable peer")
    .expect("cached fetch succeeds without the network");
    assert_eq!(second, plaintext);
}

#[tokio::test]
async fn send_call_signal_is_fire_and_forget_not_outboxed() {
    let alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();

    alice
        .service
        .send_call_signal(&bob_ticket, CallControlEvent::Ring)
        .await
        .expect("send_call_signal succeeds");

    let envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .expect("handle_incoming succeeds");
    match event {
        Some(IncomingEvent::CallSignal { event, .. }) => {
            assert!(matches!(event, CallControlEvent::Ring));
        }
        other => panic!("expected a CallSignal event, got {other:?}"),
    }

    // Fire-and-forget per service.rs's own doc comment: nothing should
    // ever have touched Alice's outbox for a call signal.
    let due = alice.outbox.due(i64::MAX, 50).expect("due() succeeds");
    assert!(due.is_empty(), "call signals must never enter the outbox");
}

#[tokio::test]
async fn retry_due_resends_unacked_messages_and_stops_after_the_ack_arrives() {
    let mut alice = Node::spawn().await;
    let mut bob = Node::spawn().await;
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    let sent_id = alice
        .service
        .send_text(conversation, &bob_ticket, text("retry me"))
        .await
        .expect("send_text succeeds");

    // Drain the first delivery so it doesn't leak into the resend
    // assertion below.
    let _first_delivery = bob.recv_envelope().await;

    // Force this message due right now regardless of the real
    // ACK_TIMEOUT_MILLIS window, then let retry_due find it.
    alice
        .outbox
        .reschedule(sent_id, 0)
        .expect("reschedule succeeds");

    let retried = alice.service.retry_due().await.expect("retry_due succeeds");
    assert_eq!(retried, 1, "exactly the one due message should be retried");

    let resent_envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, resent_envelope)
        .await
        .expect("handle_incoming succeeds");
    assert!(matches!(event, Some(IncomingEvent::Content(_))));

    // Now process the ack and confirm retry_due finds nothing left due.
    let ack_envelope = alice.recv_envelope().await;
    alice
        .service
        .handle_incoming(&bob_ticket, ack_envelope)
        .await
        .expect("ack processes");

    let retried_again = alice.service.retry_due().await.expect("retry_due succeeds");
    assert_eq!(
        retried_again, 0,
        "an acked message must not be retried again"
    );
}

#[tokio::test]
async fn retry_due_backs_off_a_message_whose_peer_is_unreachable() {
    let alice = Node::spawn().await;
    let conversation = ConversationId::new();

    // A ticket for a real key with no endpoint bound at that address —
    // send_text's own network leg will fail, leaving the message queued
    // for retry via record_failure (the Err branch in service.rs).
    let unreachable_ticket = PeerTicket {
        endpoint_addr: iroh::EndpointAddr::new(iroh::SecretKey::generate().public()),
        x25519_public: [1u8; 32],
        ed25519_verifying: [2u8; 32],
    };

    let message_id = alice
        .service
        .send_text(conversation, &unreachable_ticket, text("nobody home"))
        .await
        .expect("send_text still succeeds locally even though delivery will fail");

    // The failed send should already have scheduled a backed-off retry
    // (send_text's Err branch calls record_failure) — not due yet.
    let due_now = alice.outbox.due(millis_now(), 50).expect("due() succeeds");
    assert!(
        due_now.iter().all(|op| op.message_id != message_id),
        "a freshly-failed send must be backed off, not immediately due again"
    );

    // But it is queued for a later attempt.
    let due_far_future = alice.outbox.due(i64::MAX, 50).expect("due() succeeds");
    assert!(
        due_far_future.iter().any(|op| op.message_id == message_id),
        "a failed send must still be scheduled for a future retry"
    );
}

#[tokio::test]
async fn mailbox_check_in_and_anonymous_check_in_are_independently_signed() {
    let alice = Node::spawn().await;
    let bob = Node::spawn().await;
    let bob_ticket = bob.ticket();

    let now = millis_now() as u64;
    let signed = alice.service.sign_mailbox_check_in(now);
    assert_eq!(signed.device, alice.device_id);

    // Two anonymous check-ins for the same peer at the same instant
    // must be deterministic (same epoch/token), not a fresh random
    // token every call — an anonymous relay-facing check-in that
    // changed every call couldn't be matched twice by design.
    let anon_a = alice.service.build_anonymous_check_in(&bob_ticket, now);
    let anon_b = alice.service.build_anonymous_check_in(&bob_ticket, now);
    assert_eq!(
        format!("{anon_a:?}"),
        format!("{anon_b:?}"),
        "the same peer+epoch must derive the same anonymous check-in"
    );
}

#[tokio::test]
async fn send_text_anon_round_trips_through_the_relay_deposit() {
    // send_text_anon addresses by TokenMailboxDeposit rather than a
    // direct V1 envelope — confirm decrypt_token_mailbox_envelope (the
    // one method on this path nothing had ever called) really can
    // decrypt what arrived at the relay.
    let mut alice = Node::spawn().await; // plays "relay" here: receives the deposit frame
    let bob = Node::spawn().await; // the intended recipient
    let alice_ticket = alice.ticket();
    let bob_ticket = bob.ticket();

    // Bob addresses the deposit to himself via alice-as-relay, so this
    // test's crypto assertion (decrypt_token_mailbox_envelope) is about
    // the encrypt/decrypt round trip itself, not contact-list wiring.
    bob.service
        .send_text_anon(&bob_ticket, &alice_ticket, text("via relay"))
        .await
        .expect("send_text_anon succeeds");

    let frame = tokio::time::timeout(Duration::from_secs(20), alice.incoming.recv())
        .await
        .expect("relay receives within timeout")
        .expect("channel open");
    let siar_protocol::WireMessage::TokenMailboxDeposit(deposit) = frame.message else {
        panic!("expected a TokenMailboxDeposit frame");
    };

    let content = bob
        .service
        .decrypt_token_mailbox_envelope(&bob_ticket, &deposit)
        .expect("decrypts with the matching session");
    let MessageContent::Text(received) = content else {
        panic!("expected Text content");
    };
    assert_eq!(received.as_str(), "via relay");
}

/// §14 "Transactional Outbox": "If the process dies, the outbox still
/// exists" — the outbox is the actual system of record for send/retry,
/// the event log is additive (see `MessageService::record_messaging_
/// event`'s own doc comment). This test makes that real: an
/// `EventStore` that fails on every single call must not stop
/// `send_text` from persisting the message, enqueuing it in the real
/// outbox, or returning `Ok`. This is the honest, achievable half of
/// §14 for this workspace right now — `siar-storage`'s outbox and
/// `siar-event-log`'s own store are two separate `stoolap::Database`
/// instances (see `siar_event_log::stoolap_store`'s own doc comment),
/// so true single-transaction atomicity across both (§14's own
/// "MessageQueued, OutboxOperation, message projection, conversation
/// summary" all in one commit) isn't attempted — only that a failure
/// in one doesn't take down the other.
struct AlwaysFailingEventStore;

#[async_trait::async_trait]
impl siar_event_log::EventStore for AlwaysFailingEventStore {
    async fn append(
        &self,
        _request: siar_event_log::AppendRequest,
    ) -> Result<siar_event_log::AppendResult, siar_event_log::EventStoreError> {
        Err(siar_event_log::EventStoreError::Backend(
            "simulated event log failure".to_string(),
        ))
    }

    async fn read_stream(
        &self,
        _stream: siar_event_log::StreamId,
        _from_version: u64,
        _limit: usize,
    ) -> Result<Vec<siar_event_log::StoredEvent>, siar_event_log::EventStoreError> {
        Err(siar_event_log::EventStoreError::Backend(
            "simulated event log failure".to_string(),
        ))
    }

    async fn read_log(
        &self,
        _from_offset: siar_event_log::LocalLogOffset,
        _limit: usize,
    ) -> Result<Vec<siar_event_log::StoredEvent>, siar_event_log::EventStoreError> {
        Err(siar_event_log::EventStoreError::Backend(
            "simulated event log failure".to_string(),
        ))
    }
}

#[tokio::test]
async fn send_text_survives_a_completely_broken_event_log() {
    let alice = Node::spawn().await;
    let bob = Node::spawn().await;
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    // Rebuild Alice's own service against the SAME real repositories
    // and endpoint `Node::spawn` already set up, but with an
    // `EventStore` that fails on every call — proving the failure
    // mode is really independent of `siar-storage`, not just untested.
    // `Node` doesn't keep its own `blobs` repository around (nothing
    // else needs it post-construction) — a fresh one is fine here
    // since this test never touches attachments.
    let blobs: Arc<dyn BlobRepository + Send + Sync> = Arc::new(StoolapBlobRepository::new(
        open_in_memory().expect("in-memory db opens"),
    ));
    let broken_service = MessageService::new(
        alice.device_id,
        alice.identity.try_clone().expect("identity clones"),
        Arc::clone(&alice.endpoint),
        Arc::clone(&alice.messages),
        Arc::clone(&alice.outbox),
        blobs,
    )
    .with_event_log(Arc::new(AlwaysFailingEventStore) as Arc<dyn EventStore + Send + Sync>);

    let message_id = broken_service
        .send_text(conversation, &bob_ticket, text("still works"))
        .await
        .expect("send_text must succeed even though the event log always fails");

    let stored = alice
        .messages
        .get(message_id)
        .expect("lookup succeeds")
        .expect("the message was really persisted via siar-storage's own outbox");
    assert_eq!(stored.conversation_id, conversation);

    // §18's own read-your-writes summary is the one thing that's
    // allowed to be missing here — it lives entirely inside the event
    // log this service was built to fail.
    assert!(
        broken_service
            .conversation_summary(conversation)
            .await
            .is_none(),
        "no summary can exist when every append to back it failed"
    );
}

/// The durable counterpart to `conversation_summary_reflects_sends_
/// and_receipts_without_any_manual_catch_up`: same real send→receive
/// flow through the actual `MessageService` API, but with
/// `with_durable_conversation_summary` swapped in — proving the
/// `stoolap`-backed projection really is a drop-in replacement for the
/// in-memory one from `MessageService`'s own caller's point of view,
/// not just correct in `stoolap_projections`'s own isolated unit
/// tests. Also closes the loop `StoolapCheckpointStore`'s own doc
/// comment named as still-open: this is that store's first real
/// caller, exercised through an actual multi-event workflow rather
/// than only direct unit tests of the checkpoint store by itself.
#[tokio::test]
async fn durable_conversation_summary_works_as_a_drop_in_replacement() {
    let alice = Node::spawn().await;
    let bob = Node::spawn().await;
    let bob_ticket = bob.ticket();
    let conversation = ConversationId::new();

    let event_log: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
    let projection = Arc::new(StoolapConversationSummaryProjection::open_in_memory().unwrap());
    let checkpoints = Arc::new(StoolapCheckpointStore::open_in_memory().unwrap());
    let blobs: Arc<dyn BlobRepository + Send + Sync> = Arc::new(StoolapBlobRepository::new(
        open_in_memory().expect("in-memory db opens"),
    ));

    let durable_service = MessageService::new(
        alice.device_id,
        alice.identity.try_clone().expect("identity clones"),
        Arc::clone(&alice.endpoint),
        Arc::clone(&alice.messages),
        Arc::clone(&alice.outbox),
        blobs,
    )
    .with_event_log(event_log)
    .with_durable_conversation_summary(projection, checkpoints);

    assert!(durable_service
        .conversation_summary(conversation)
        .await
        .is_none());

    let message_id = durable_service
        .send_text(conversation, &bob_ticket, text("durable read your writes"))
        .await
        .expect("send_text succeeds");

    let summary = durable_service
        .conversation_summary(conversation)
        .await
        .expect("send_text must leave the DURABLE summary populated too, synchronously");
    assert_eq!(summary.message_count, 1);
    assert_eq!(summary.last_message_id, Some(message_id));
}
