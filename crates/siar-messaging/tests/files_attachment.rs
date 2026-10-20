//! spec 05 §204 over the REAL stack (two iroh endpoints on loopback): an
//! attachment is imported into the file subsystem's store, its reference
//! rides a normal message, and the recipient's unchanged
//! `fetch_attachment` pulls and decrypts it from the file store — with
//! no copy ever placed in `siar-storage`.

use std::sync::Arc;
use std::time::Duration;

use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_crypto::DeviceIdentity;
use siar_domain::{ConversationId, DeviceId, MediaType, MessageContent};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, InMemoryEventStore};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use siar_file_transfer_service::FileTransferService;
use siar_files::{ChannelError, FileChannel, FilesConfig, FilesService, FrameKind};
use siar_messaging::{FilesBlobStore, IncomingEvent, MessageService, PeerTicket, StorageBlobStore};
use siar_storage::{
    open_in_memory, BlobRepository, MessageRepository, OutboxRepository, StoolapBlobRepository,
    StoolapMessageRepository, StoolapOutboxRepository,
};
use siar_transport::{BlobStore, SiarEndpoint};
use tokio::sync::mpsc;

/// No peer-to-peer file transfer happens in this test.
struct NoChannel;
impl FileChannel for NoChannel {
    async fn send_frame(&self, _: DeviceId, _: FrameKind, _: Vec<u8>) -> Result<(), ChannelError> {
        Err(ChannelError::Closed)
    }
}

type Files = FilesService<NoChannel, InMemoryBlobMetadata, InMemoryReferenceStore>;

struct Node {
    identity: DeviceIdentity,
    endpoint: Arc<SiarEndpoint>,
    service: MessageService,
    blobs: Arc<dyn BlobRepository + Send + Sync>,
    incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
}

impl Node {
    async fn spawn(files: Option<Arc<Files>>) -> Self {
        let device_id = DeviceId::new();
        let identity = DeviceIdentity::generate();
        let db = open_in_memory().unwrap();
        let messages: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoolapMessageRepository::new(Arc::clone(&db)));
        let outbox: Arc<dyn OutboxRepository + Send + Sync> =
            Arc::new(StoolapOutboxRepository::new(Arc::clone(&db)));
        let blobs: Arc<dyn BlobRepository + Send + Sync> =
            Arc::new(StoolapBlobRepository::new(Arc::clone(&db)));
        let blob_store: Arc<dyn BlobStore> = match files {
            Some(f) => Arc::new(FilesBlobStore(f)),
            None => Arc::new(StorageBlobStore(Arc::clone(&blobs))),
        };
        let (tx, rx) = mpsc::channel(16);
        let endpoint = Arc::new(
            SiarEndpoint::bind(iroh::SecretKey::generate(), tx, blob_store)
                .await
                .unwrap(),
        );
        let service = MessageService::new(
            device_id,
            identity.try_clone().unwrap(),
            Arc::clone(&endpoint),
            messages,
            outbox,
            Arc::clone(&blobs),
        );
        Self {
            identity,
            endpoint,
            service,
            blobs,
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

    async fn recv_envelope(&mut self) -> siar_protocol::v1::Envelope {
        let frame = tokio::time::timeout(Duration::from_secs(20), self.incoming.recv())
            .await
            .unwrap()
            .unwrap();
        let siar_protocol::WireMessage::V1(e) = frame.message else {
            panic!("expected V1")
        };
        e
    }
}

fn files_service() -> (Arc<Files>, Arc<InMemoryReferenceStore>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("siar-msg-files-{}", uuid::Uuid::new_v4()));
    let store = FsBlobStore::open(
        &root,
        FsStoreConfig {
            limits: StoreLimits::default(),
            durability: DurabilityPolicy::None,
            ..FsStoreConfig::default()
        },
        InMemoryBlobMetadata::new(),
    )
    .unwrap();
    let refs = Arc::new(InMemoryReferenceStore::new());
    let ns = NamespaceId::new("personal").unwrap();
    let engine = Arc::new(TransferEngine::new(
        FileTransferService::new().with_event_log(Arc::new(InMemoryEventStore::new())),
        store,
        refs.clone(),
        ns.clone(),
    ));
    let svc = FilesService::new(
        engine,
        refs.clone(),
        NoChannel,
        FilesConfig::new(ns, EventOrigin::LocalDevice(DeviceId::new())),
    );
    (svc, refs, root)
}

#[tokio::test]
async fn an_imported_attachment_is_sent_fetched_and_decrypted_without_a_storage_copy() {
    let (files, refs, root) = files_service();
    let mut alice = Node::spawn(Some(Arc::clone(&files))).await;
    let mut bob = Node::spawn(None).await;
    let (alice_ticket, bob_ticket) = (alice.ticket(), bob.ticket());
    let conversation = ConversationId::new();
    let plaintext: Vec<u8> = (0..70_000u32).map(|i| (i % 253) as u8).collect();

    let imported = files
        .import_attachment(&plaintext, MediaType::ImagePng)
        .await
        .unwrap();
    let message_id = alice
        .service
        .send_imported_attachment(conversation, &bob_ticket, &imported)
        .await
        .expect("send succeeds");

    // The blob now belongs to the message, not to the import.
    files
        .bind_to_object(
            &imported,
            ObjectId::new(&message_id.to_string()).unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    assert_eq!(refs.count_for(imported.blob_id).unwrap(), 1);

    // Alice's siar-storage never saw the ciphertext.
    assert!(alice
        .blobs
        .get(imported.blob_id.as_bytes())
        .unwrap()
        .is_none());

    let envelope = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice_ticket, envelope)
        .await
        .unwrap();
    let IncomingEvent::Content(MessageContent::Attachment(reference)) = event.unwrap() else {
        panic!("expected an attachment");
    };
    assert_eq!(reference.blob_hash, *imported.blob_id.as_bytes());

    // Bob's UNCHANGED fetch path pulls it from Alice's file store.
    let fetched = bob
        .service
        .fetch_attachment(&alice_ticket, &reference)
        .await
        .expect("fetch from the file store succeeds");
    assert_eq!(fetched, plaintext);

    // And Alice can open her own copy by the same reference.
    assert_eq!(files.open_attachment(&reference).await.unwrap(), plaintext);

    let _ = alice.recv_envelope().await; // drain the ack
    let _ = std::fs::remove_dir_all(root);
}
