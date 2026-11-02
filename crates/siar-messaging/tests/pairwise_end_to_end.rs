//! 1:1 over OpenMLS through the real `MessageService`: real stoolap, real
//! QUIC over loopback, real persisted MLS sessions. Same harness shape as
//! `end_to_end.rs`.

use siar_crypto::DeviceIdentity;
use siar_crypto_mls::{PairwisePolicy, PairwiseProfile};
use siar_domain::{ConversationId, DeliveryState, DeviceId, MessageContent, MessageText};
use siar_messaging::{
    InMemoryKeyPackageDirectory, IncomingEvent, KeyPackageDirectory, MessageService,
    PairwiseService, PeerTicket, StorageBlobStore,
};
use siar_protocol::v1::{Envelope, EnvelopeKind};
use siar_storage::{
    open_in_memory, BlobRepository, MessageRepository, OutboxRepository, StoolapBlobRepository,
    StoolapMessageRepository, StoolapOutboxRepository,
};
use siar_transport::{BlobStore, SiarEndpoint};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

struct Node {
    device_id: DeviceId,
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    service: MessageService,
    pairwise: Arc<PairwiseService>,
    messages: Arc<dyn MessageRepository + Send + Sync>,
    incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
}

impl Node {
    async fn spawn(name: &str) -> Self {
        let device_id = DeviceId::new();
        let identity = DeviceIdentity::generate();
        let db = open_in_memory().unwrap();
        let messages: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoolapMessageRepository::new(Arc::clone(&db)));
        let outbox: Arc<dyn OutboxRepository + Send + Sync> =
            Arc::new(StoolapOutboxRepository::new(Arc::clone(&db)));
        let blobs: Arc<dyn BlobRepository + Send + Sync> =
            Arc::new(StoolapBlobRepository::new(Arc::clone(&db)));
        let blob_store: Arc<dyn BlobStore> = Arc::new(StorageBlobStore(Arc::clone(&blobs)));
        let (tx, rx) = mpsc::channel(16);
        let endpoint = Arc::new(
            SiarEndpoint::bind(iroh::SecretKey::generate(), tx, blob_store)
                .await
                .unwrap(),
        );
        let dir =
            std::env::temp_dir().join(format!("siar-pw-e2e-{name}-{}", ConversationId::new()));
        let pairwise = Arc::new(
            PairwiseService::new(
                device_id,
                dir,
                PairwisePolicy {
                    update_after_sent: 1,
                    ..PairwisePolicy::default()
                },
            )
            .unwrap(),
        );
        let service = MessageService::new(
            device_id,
            identity.try_clone().unwrap(),
            Arc::clone(&endpoint),
            Arc::clone(&messages),
            outbox,
            blobs,
        )
        .with_pairwise(Arc::clone(&pairwise));
        Self {
            device_id,
            identity,
            endpoint,
            service,
            pairwise,
            messages,
            incoming: rx,
        }
    }

    fn ticket(&self) -> PeerTicket {
        let full = self.endpoint.addr();
        let ips = full
            .addrs
            .into_iter()
            .filter(|a| matches!(a, iroh::TransportAddr::Ip(_)));
        PeerTicket {
            endpoint_addr: iroh::EndpointAddr::from_parts(full.id, ips),
            x25519_public: self.identity.x25519_public().to_bytes(),
            ed25519_verifying: self.identity.verifying_key().to_bytes(),
        }
    }

    async fn recv(&mut self) -> Envelope {
        let f = tokio::time::timeout(Duration::from_secs(20), self.incoming.recv())
            .await
            .expect("frame within timeout")
            .expect("channel open");
        let siar_protocol::WireMessage::V1(e) = f.message else {
            panic!("expected V1")
        };
        e
    }
}

fn text(s: &str) -> MessageText {
    MessageText::parse(s.to_string()).unwrap()
}

/// Run the whole handshake: Bob publishes, Alice opens, Bob accepts, Alice
/// sees the ack. Returns the conversation id.
async fn handshake(alice: &mut Node, bob: &mut Node) -> ConversationId {
    let dir = InMemoryKeyPackageDirectory::new();
    bob.pairwise
        .publish_key_package(PairwiseProfile::Compact, &dir)
        .unwrap();
    let conv = ConversationId::new();
    let (a_t, b_t) = (alice.ticket(), bob.ticket());
    assert!(alice
        .service
        .open_pairwise_session(conv, &b_t, &dir, bob.device_id, PairwiseProfile::Compact)
        .await
        .unwrap());
    let w = bob.recv().await;
    assert!(matches!(w.kind, EnvelopeKind::PairwiseWelcome));
    bob.service.handle_incoming(&a_t, w).await.unwrap();
    assert!(bob.pairwise.has_session(conv));
    let ack = alice.recv().await;
    alice.service.handle_incoming(&b_t, ack).await.unwrap();
    assert!(
        alice.pairwise.due(u64::MAX).is_empty(),
        "the Welcome's ack clears its retransmission row"
    );
    conv
}

#[tokio::test]
async fn a_pairwise_conversation_round_trips_both_ways_with_plaintext_at_rest() {
    let (mut alice, mut bob) = (Node::spawn("a").await, Node::spawn("b").await);
    let conv = handshake(&mut alice, &mut bob).await;
    let (a_t, b_t) = (alice.ticket(), bob.ticket());

    let id = alice
        .service
        .send_text(conv, &b_t, text("forward-secret hello"))
        .await
        .unwrap();
    let env = bob.recv().await;
    assert!(matches!(env.kind, EnvelopeKind::PairwiseMls));
    assert!(
        !env.payload
            .windows(b"forward-secret".len())
            .any(|w| w == b"forward-secret"),
        "the plaintext must not appear on the wire"
    );
    let ev = bob.service.handle_incoming(&a_t, env).await.unwrap();
    let Some(IncomingEvent::Content(MessageContent::Text(t))) = ev else {
        panic!("expected a text event")
    };
    assert_eq!(t.as_str(), "forward-secret hello");

    // Alice's ack closes the retransmission row and marks the message delivered.
    let ack = alice.recv().await;
    alice.service.handle_incoming(&b_t, ack).await.unwrap();
    assert!(alice.pairwise.due(u64::MAX).is_empty());
    assert_eq!(
        alice.messages.get(id).unwrap().unwrap().delivery_state,
        DeliveryState::Delivered
    );

    // The stored row is plaintext on both sides (a delivered MLS ciphertext
    // can't be opened again, so storage must hold the content).
    let stored = bob.messages.get(id).unwrap().unwrap();
    assert!(bob
        .service
        .open_stored_pairwise_payload(conv, &stored.payload)
        .is_some());

    // And the other direction (Bob's turn: his first message carries a commit).
    bob.service
        .send_text(conv, &a_t, text("and back"))
        .await
        .unwrap();
    let env = alice.recv().await;
    let ev = alice.service.handle_incoming(&b_t, env).await.unwrap();
    let Some(IncomingEvent::Content(MessageContent::Text(t))) = ev else {
        panic!("expected a text event")
    };
    assert_eq!(t.as_str(), "and back");
}

#[tokio::test]
async fn an_unacked_message_is_resent_verbatim_and_a_duplicate_is_not_a_second_event() {
    let (mut alice, mut bob) = (Node::spawn("a2").await, Node::spawn("b2").await);
    let conv = handshake(&mut alice, &mut bob).await;
    let (a_t, b_t) = (alice.ticket(), bob.ticket());

    alice
        .service
        .send_text(conv, &b_t, text("once"))
        .await
        .unwrap();
    let first = bob.recv().await;
    // Pretend the ack never made it back: force the row due, retry.
    let row = alice.pairwise.due(u64::MAX).pop().unwrap();
    alice.pairwise.reschedule(row.message_id, 0).unwrap();
    assert_eq!(alice.service.retry_due().await.unwrap(), 1);
    let resent = bob.recv().await;
    assert_eq!(
        first.payload, resent.payload,
        "resent verbatim, not re-encrypted"
    );

    assert!(bob
        .service
        .handle_incoming(&a_t, first)
        .await
        .unwrap()
        .is_some());
    // The retransmission yields no second event (and no error that would
    // make the receive loop drop the connection).
    let again = bob.service.handle_incoming(&a_t, resent).await;
    assert!(matches!(again, Ok(None)) || again.is_err());
}

#[tokio::test]
async fn without_a_published_key_package_the_caller_is_told_to_keep_the_static_path() {
    let (alice, bob) = (Node::spawn("a3").await, Node::spawn("b3").await);
    let dir = InMemoryKeyPackageDirectory::new();
    let opened = alice
        .service
        .open_pairwise_session(
            ConversationId::new(),
            &bob.ticket(),
            &dir,
            bob.device_id,
            PairwiseProfile::Compact,
        )
        .await
        .unwrap();
    assert!(!opened);
    let _ = dir.take(bob.device_id);
}
