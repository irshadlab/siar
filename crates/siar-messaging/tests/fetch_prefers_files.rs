//! `MessageService::fetch_attachment` with a `files/1` fetcher wired in,
//! over two REAL iroh endpoints: it pulls chunked when the peer speaks
//! `files/1`, falls back to the legacy whole-blob request when it does
//! not, and stops when the peer answers "unavailable".

use std::sync::Arc;
use std::time::Duration;

use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{BlobRole, InMemoryReferenceStore, ObjectId};
use siar_crypto::DeviceIdentity;
use siar_domain::{ConversationId, DeviceId, MediaType, MessageContent};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, InMemoryEventStore};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use siar_file_transfer_service::FileTransferService;
use siar_files::{CiphertextSource, FilesConfig, FilesService};
use siar_messaging::{
    FilesAttachmentFetcher, FilesBlobStore, FilesInbound, IncomingEvent, MessageService,
    MessageServiceError, PeerBook, PeerTicket, StorageBlobStore, TransportFileChannel,
};
use siar_storage::{
    open_in_memory, BlobRepository, MessageRepository, OutboxRepository, StoolapBlobRepository,
    StoolapMessageRepository, StoolapOutboxRepository,
};
use siar_transport::{BlobStore, DeferredFilesSink, FilesFrameSink, SiarEndpoint};
use tokio::sync::mpsc;

type Files = FilesService<TransportFileChannel, InMemoryBlobMetadata, InMemoryReferenceStore>;

struct Peer {
    identity: DeviceIdentity,
    device: DeviceId,
    endpoint: Arc<SiarEndpoint>,
    service: MessageService,
    blobs: Arc<dyn BlobRepository + Send + Sync>,
    book: Arc<PeerBook>,
    files: Option<Arc<Files>>,
    incoming: mpsc::Receiver<siar_transport::IncomingFrame>,
    root: std::path::PathBuf,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Peer {
    /// `speak_files`: whether this peer implements `files/1` at all.
    /// `serve_from_files`: whether its LEGACY blob protocol serves from
    /// the file store (instead of `siar-storage`).
    async fn spawn(speak_files: bool, prefer_files: bool, serve_from_files: bool) -> Self {
        let device = DeviceId::new();
        let identity = DeviceIdentity::generate();
        let db = open_in_memory().unwrap();
        let messages: Arc<dyn MessageRepository + Send + Sync> =
            Arc::new(StoolapMessageRepository::new(Arc::clone(&db)));
        let outbox: Arc<dyn OutboxRepository + Send + Sync> =
            Arc::new(StoolapOutboxRepository::new(Arc::clone(&db)));
        let blobs: Arc<dyn BlobRepository + Send + Sync> =
            Arc::new(StoolapBlobRepository::new(Arc::clone(&db)));
        let book = PeerBook::new();
        let root = std::env::temp_dir().join(format!("siar-pref-{}", uuid::Uuid::new_v4()));

        // The file service exists only for peers that speak files/1.
        let deferred = DeferredFilesSink::new();
        let files_sink: Option<Arc<dyn FilesFrameSink>> =
            speak_files.then(|| deferred.clone() as Arc<dyn FilesFrameSink>);
        // The legacy server is bound before the file service exists, so a
        // peer that serves its legacy blobs from the file store needs the
        // service first; build the store pieces up front.
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

        // Late-bound legacy server: needs the FilesService (built after the
        // endpoint). A small indirection keeps the order workable.
        struct LateBlobs(std::sync::OnceLock<Arc<dyn BlobStore>>);
        impl BlobStore for LateBlobs {
            fn get(&self, h: &[u8; 32]) -> Option<Vec<u8>> {
                self.0.get().and_then(|s| s.get(h))
            }
        }
        let late = Arc::new(LateBlobs(std::sync::OnceLock::new()));
        let (tx, rx) = mpsc::channel(16);
        let endpoint = Arc::new(
            SiarEndpoint::bind_with_files(
                iroh::SecretKey::generate(),
                tx,
                Arc::clone(&late) as Arc<dyn BlobStore>,
                files_sink,
            )
            .await
            .unwrap(),
        );
        let files: Option<Arc<Files>> = speak_files.then(|| {
            let svc = FilesService::new(
                engine.clone(),
                refs.clone(),
                TransportFileChannel::new(Arc::clone(&endpoint), Arc::clone(&book)),
                FilesConfig::new(ns.clone(), EventOrigin::LocalDevice(device)),
            );
            assert!(deferred.set(Arc::new(FilesInbound::new(
                Arc::clone(&svc),
                Arc::clone(&book)
            ))));
            svc
        });
        let legacy: Arc<dyn BlobStore> = match (&files, serve_from_files) {
            (Some(f), true) => Arc::new(FilesBlobStore(Arc::clone(f))),
            _ => Arc::new(StorageBlobStore(Arc::clone(&blobs))),
        };
        let _ = late.0.set(legacy);

        let mut service = MessageService::new(
            device,
            identity.try_clone().unwrap(),
            Arc::clone(&endpoint),
            messages,
            outbox,
            Arc::clone(&blobs),
        );
        if prefer_files {
            let f = files.clone().expect("a fetcher needs a file service");
            service = service.with_attachment_fetcher(Arc::new(FilesAttachmentFetcher::new(
                f,
                Arc::clone(&book),
            )));
        }
        Self {
            identity,
            device,
            endpoint,
            service,
            blobs,
            book,
            files,
            incoming: rx,
            root,
        }
    }

    fn addr(&self) -> iroh::EndpointAddr {
        let full = self.endpoint.addr();
        let ips = full
            .addrs
            .into_iter()
            .filter(|a| matches!(a, iroh::TransportAddr::Ip(_)));
        iroh::EndpointAddr::from_parts(full.id, ips)
    }

    fn ticket(&self) -> PeerTicket {
        PeerTicket {
            endpoint_addr: self.addr(),
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

fn plaintext() -> Vec<u8> {
    (0..1_300_000u32).map(|i| (i % 249) as u8).collect()
}

/// Alice sends Bob an attachment reference in a real message, imported
/// into her file store. Returns what Bob received.
async fn deliver_reference(
    alice: &mut Peer,
    bob: &mut Peer,
    data: &[u8],
) -> siar_domain::AttachmentReference {
    let files = alice.files.clone().expect("alice speaks files/1");
    let imported = files
        .import_attachment(&data.to_vec(), MediaType::ImagePng)
        .await
        .unwrap();
    let msg = alice
        .service
        .send_imported_attachment(ConversationId::new(), &bob.ticket(), &imported)
        .await
        .unwrap();
    files
        .bind_to_object(
            &imported,
            ObjectId::new(&msg.to_string()).unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    let env = bob.recv_envelope().await;
    let event = bob
        .service
        .handle_incoming(&alice.ticket(), env)
        .await
        .unwrap();
    let IncomingEvent::Content(MessageContent::Attachment(reference)) = event.unwrap() else {
        panic!("expected an attachment")
    };
    reference
}

#[tokio::test]
async fn fetch_attachment_uses_files_1_when_the_peer_speaks_it() {
    let mut alice = Peer::spawn(true, false, false).await; // legacy server: siar-storage (EMPTY)
    let mut bob = Peer::spawn(true, true, false).await;
    alice.book.register(bob.device, bob.addr());
    bob.book.register(alice.device, alice.addr());
    let data = plaintext();
    let reference = deliver_reference(&mut alice, &mut bob, &data).await;

    // The legacy path could not succeed (Alice's legacy store is empty):
    // success proves the chunked path ran.
    let got = bob
        .service
        .fetch_attachment(&alice.ticket(), &reference)
        .await
        .expect("pulled over files/1");
    assert_eq!(got, data);
    // …and it landed in Bob's file store, not in siar-storage.
    let bob_files = bob.files.as_ref().unwrap();
    assert!(bob_files.ciphertext(&reference.blob_hash).is_some());
    assert!(bob.blobs.get(&reference.blob_hash).unwrap().is_none());
}

#[tokio::test]
async fn it_falls_back_to_the_legacy_path_when_the_peer_cannot_speak_files_1() {
    // Alice does not implement files/1 at all; her LEGACY server holds the
    // attachment in siar-storage (plain send_attachment).
    let alice = Peer::spawn(false, false, false).await;
    let mut bob = Peer::spawn(true, true, false).await;
    alice.book.register(bob.device, bob.addr());
    bob.book.register(alice.device, alice.addr());
    let data = plaintext();
    alice
        .service
        .send_attachment(
            ConversationId::new(),
            &bob.ticket(),
            data.clone(),
            MediaType::ImagePng,
        )
        .await
        .unwrap();
    let env = bob.recv_envelope().await;
    let IncomingEvent::Content(MessageContent::Attachment(reference)) = bob
        .service
        .handle_incoming(&alice.ticket(), env)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected an attachment")
    };
    let got = bob
        .service
        .fetch_attachment(&alice.ticket(), &reference)
        .await
        .expect("legacy fallback succeeds");
    assert_eq!(got, data);
    // It came through the legacy path, which caches in siar-storage.
    assert!(bob.blobs.get(&reference.blob_hash).unwrap().is_some());
}

#[tokio::test]
async fn an_unavailable_answer_ends_the_attempt_without_trying_the_legacy_path() {
    // Alice speaks files/1 but her gate-less default serves only
    // referenced blobs; Bob asks for a blob she never had.
    let mut alice = Peer::spawn(true, false, true).await;
    let mut bob = Peer::spawn(true, true, false).await;
    alice.book.register(bob.device, bob.addr());
    bob.book.register(alice.device, alice.addr());
    let data = plaintext();
    let mut reference = deliver_reference(&mut alice, &mut bob, &data).await;
    reference.blob_hash = [0x5A; 32]; // a blob nobody has

    let started = std::time::Instant::now();
    let r = bob
        .service
        .fetch_attachment(&alice.ticket(), &reference)
        .await;
    assert!(matches!(r, Err(MessageServiceError::BlobNotFound)), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn without_a_fetcher_behaviour_is_exactly_what_it_was() {
    let mut alice = Peer::spawn(true, false, true).await; // legacy serves from the file store
    let mut bob = Peer::spawn(true, false, false).await; // no fetcher
    alice.book.register(bob.device, bob.addr());
    bob.book.register(alice.device, alice.addr());
    let data = plaintext();
    let reference = deliver_reference(&mut alice, &mut bob, &data).await;
    let got = bob
        .service
        .fetch_attachment(&alice.ticket(), &reference)
        .await
        .unwrap();
    assert_eq!(got, data);
    assert!(
        bob.blobs.get(&reference.blob_hash).unwrap().is_some(),
        "legacy caches"
    );
}
