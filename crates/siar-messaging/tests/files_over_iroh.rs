//! `files/1` end to end over REAL iroh endpoints (loopback): two devices,
//! each with a full `FilesService` over `TransportFileChannel`, a real
//! blob store, real engine, real QUIC streams. Nothing in memory stands in
//! for the network.

use std::sync::Arc;
use std::time::Duration;

use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_crypto::DeviceIdentity;
use siar_domain::{DeviceId, MediaType};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, InMemoryEventStore};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use siar_file_transfer_service::FileTransferService;
use siar_files::{
    CiphertextSource, Destination, DestinationPolicy, FileSendPolicy, FilesConfig, FilesService,
};
use siar_messaging::{
    fetch_attachment_over_files, FilesInbound, IdentityPairwiseSecrets, PeerBook,
    TransportFileChannel,
};
use siar_transport::{BlobStore, DeferredFilesSink, SiarEndpoint};
use tokio::sync::mpsc;

type Svc = FilesService<TransportFileChannel, InMemoryBlobMetadata, InMemoryReferenceStore>;

struct NullBlobs;
impl BlobStore for NullBlobs {
    fn get(&self, _: &[u8; 32]) -> Option<Vec<u8>> {
        None
    }
}

struct Node {
    identity: Arc<DeviceIdentity>,
    device: DeviceId,
    endpoint: Arc<SiarEndpoint>,
    book: Arc<PeerBook>,
    svc: Arc<Svc>,
    refs: Arc<InMemoryReferenceStore>,
    root: std::path::PathBuf,
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Node {
    async fn spawn() -> Self {
        let device = DeviceId::new();
        let identity = Arc::new(DeviceIdentity::generate());
        let book = PeerBook::new();
        let deferred = DeferredFilesSink::new();
        let (tx, _rx) = mpsc::channel(16);
        let endpoint = Arc::new(
            SiarEndpoint::bind_with_files(
                iroh::SecretKey::generate(),
                tx,
                Arc::new(NullBlobs),
                Some(deferred.clone()),
            )
            .await
            .unwrap(),
        );
        let root = std::env::temp_dir().join(format!("siar-iroh-files-{}", uuid::Uuid::new_v4()));
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
            TransportFileChannel::new(Arc::clone(&endpoint), Arc::clone(&book)),
            {
                let mut cfg = FilesConfig::new(ns, EventOrigin::LocalDevice(device));
                cfg.pairwise_secrets = Some(Arc::new(IdentityPairwiseSecrets::new(
                    Arc::clone(&identity),
                    Arc::clone(&book),
                )));
                cfg
            },
        );
        assert!(deferred.set(Arc::new(FilesInbound::new(
            Arc::clone(&svc),
            Arc::clone(&book)
        ))));
        Self {
            identity,
            device,
            endpoint,
            book,
            svc,
            refs,
            root,
        }
    }

    /// An address another node can dial on loopback.
    fn addr(&self) -> iroh::EndpointAddr {
        let full = self.endpoint.addr();
        let ips = full
            .addrs
            .into_iter()
            .filter(|a| matches!(a, iroh::TransportAddr::Ip(_)));
        iroh::EndpointAddr::from_parts(full.id, ips)
    }
}

/// Pairs two nodes (each learns the other's device id and address).
async fn pair() -> (Node, Node) {
    let (a, b) = (Node::spawn().await, Node::spawn().await);
    a.book
        .register_with_key(b.device, b.addr(), b.identity.x25519_public().to_bytes());
    b.book
        .register_with_key(a.device, a.addr(), a.identity.x25519_public().to_bytes());
    (a, b)
}

fn payload(len: usize) -> Vec<u8> {
    let mut x = 0xBEEFu32;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

fn policy(name: &str) -> DestinationPolicy {
    DestinationPolicy {
        object: ObjectId::new(name).unwrap(),
        role: BlobRole::UserSave,
        expires: TransferExpiry(u64::MAX),
    }
}

async fn eventually<F: Fn() -> bool>(what: &str, f: F) {
    for _ in 0..1500 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn a_file_crosses_real_quic_between_two_devices_and_arrives_byte_exact() {
    let (a, b) = pair().await;
    let data = payload(1_500_000);
    a.svc
        .send_file(
            Destination::Device(b.device),
            &data,
            FileSendPolicy {
                media_type: MediaType::ImagePng,
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();

    let offer = tokio::time::timeout(Duration::from_secs(20), b.svc.next_offer())
        .await
        .expect("the offer arrived over QUIC")
        .unwrap();
    assert_eq!(offer.from, a.device);
    let (blob, ciphertext_len) = (offer.offer.blob.blob_id, offer.offer.blob.size.bytes());
    offer.accept(policy("saved")).await.unwrap();

    eventually("the blob lands on the receiver", || {
        b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    let theirs = b.svc.ciphertext(blob.as_bytes()).unwrap();
    assert_eq!(theirs.len() as u64, ciphertext_len);
    assert_eq!(a.svc.ciphertext(blob.as_bytes()).unwrap(), theirs);
    assert_eq!(b.refs.count_for(blob).unwrap(), 1);
}

#[tokio::test]
async fn a_streamed_v2_file_with_many_chunks_crosses_in_order_dependent_phases() {
    // The protocol's phases (capabilities → offer → manifest → chunk
    // request → chunks → complete) only work if the transport delivers a
    // peer's frames in order. A multi-chunk v2 file exercises every phase
    // with many frames in flight.
    let (a, b) = pair().await;
    let data = payload(6_000_000);
    a.svc
        .send_file_streaming(
            Destination::Device(b.device),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("v2-saved")).await.unwrap();
    eventually("v2 blob lands", || {
        b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    assert_eq!(
        a.svc.ciphertext(blob.as_bytes()),
        b.svc.ciphertext(blob.as_bytes())
    );
}

#[tokio::test]
async fn files_flow_both_ways_and_concurrently_over_the_same_connection() {
    let (a, b) = pair().await;
    let (d1, d2) = (payload(900_000), payload(1_100_000));
    a.svc
        .send_file(
            Destination::Device(b.device),
            &d1,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    b.svc
        .send_file(
            Destination::Device(a.device),
            &d2,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let at_b = b.svc.next_offer().await.unwrap();
    let at_a = a.svc.next_offer().await.unwrap();
    let (blob_b, blob_a) = (at_b.offer.blob.blob_id, at_a.offer.blob.blob_id);
    at_b.accept(policy("from-a")).await.unwrap();
    at_a.accept(policy("from-b")).await.unwrap();
    eventually("both directions complete", || {
        b.svc.ciphertext(blob_b.as_bytes()).is_some()
            && a.svc.ciphertext(blob_a.as_bytes()).is_some()
    })
    .await;
}

#[tokio::test]
async fn a_rejected_offer_over_the_wire_frees_the_senders_blob() {
    let (a, b) = pair().await;
    a.svc
        .send_file(
            Destination::Device(b.device),
            &payload(50_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.reject().await.unwrap();
    eventually("the sender releases the declined blob", || {
        a.refs.count_for(blob).unwrap() == 0
    })
    .await;
    assert!(b.svc.ciphertext(blob.as_bytes()).is_none());
}

#[tokio::test]
async fn a_device_that_is_not_paired_cannot_start_a_transfer() {
    let (a, b) = pair().await;
    let stranger = Node::spawn().await;
    // The stranger knows how to reach `b`, but `b` has never paired it.
    stranger.book.register(b.device, b.addr());
    let _ = stranger
        .svc
        .send_file(
            Destination::Device(b.device),
            &payload(10_000),
            FileSendPolicy::default(),
        )
        .await;
    let nothing = tokio::time::timeout(Duration::from_millis(800), b.svc.next_offer()).await;
    assert!(
        nothing.is_err(),
        "an unpaired endpoint's frames are dropped"
    );
    drop(a);
}

#[tokio::test]
async fn sending_to_a_device_with_no_known_address_fails_cleanly_and_releases_the_blob() {
    let a = Node::spawn().await;
    let nobody = DeviceId::new();
    let err = a
        .svc
        .send_file(
            Destination::Device(nobody),
            &payload(10_000),
            FileSendPolicy::default(),
        )
        .await;
    assert!(err.is_err());
    // The failed send must not leave the blob pinned by a dangling
    // reference: nothing in the reference model, so GC can reclaim it.
    assert!(
        a.refs.referenced_blob_ids(None, 16).unwrap().is_empty(),
        "a failed send left a reference behind"
    );
}

#[tokio::test]
async fn a_peer_pulls_an_attachment_over_real_quic_by_its_message_reference() {
    let (a, b) = pair().await;
    let data = payload(2_200_000);
    let imported = a
        .svc
        .import_attachment(&data, MediaType::ImageJpeg)
        .await
        .unwrap();
    a.svc
        .bind_to_object(
            &imported,
            ObjectId::new("msg-quic").unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    // What the message carried to B.
    let reference = imported.reference().unwrap();

    let plaintext = fetch_attachment_over_files(
        &b.svc,
        &b.book,
        a.endpoint.id(),
        &reference,
        policy("pulled-over-quic"),
    )
    .await
    .expect("pulled over files/1");
    assert_eq!(plaintext, data);
    assert_eq!(b.refs.count_for(imported.blob_id).unwrap(), 1);

    // A peer that is not in the book is never dialed (asked about a blob
    // this device does not already hold, so the network would be needed).
    let stranger = iroh::SecretKey::generate().public();
    let other = {
        let imp = a
            .svc
            .import_attachment(&payload(5_000), MediaType::Other)
            .await
            .unwrap();
        imp.reference().unwrap()
    };
    let r = fetch_attachment_over_files(&b.svc, &b.book, stranger, &other, policy("y")).await;
    assert!(
        matches!(r, Err(siar_files::FilesError::Channel(_))),
        "{r:?}"
    );
}

#[tokio::test]
async fn pulling_something_the_peer_never_had_answers_unavailable_over_quic() {
    let (a, b) = pair().await;
    let reference = {
        // A reference to a blob nobody stored.
        let mut r = a
            .svc
            .import_attachment(&payload(1_000), MediaType::Other)
            .await
            .unwrap()
            .reference()
            .unwrap();
        r.blob_hash = [0xAB; 32];
        r
    };
    let started = std::time::Instant::now();
    let r = fetch_attachment_over_files(
        &b.svc,
        &b.book,
        a.endpoint.id(),
        &reference,
        policy("missing"),
    )
    .await;
    assert_eq!(r, Err(siar_files::FilesError::BlobUnavailable));
    assert!(started.elapsed() < Duration::from_secs(10));
}

// ---- wrapped key delivery with REAL identities over REAL QUIC ----------

fn wrapped_policy(name: &str) -> FileSendPolicy {
    FileSendPolicy {
        key_delivery: siar_files::KeyDelivery::Wrapped,
        file_name: Some(name.to_owned()),
        ..FileSendPolicy::default()
    }
}

#[tokio::test]
async fn a_key_wrapped_with_real_x25519_secrets_opens_on_the_other_device_over_quic() {
    let (a, b) = pair().await;
    let data = payload(1_400_000);
    a.svc
        .send_file(
            Destination::Device(b.device),
            &data,
            wrapped_policy("wrapped.png"),
        )
        .await
        .unwrap();
    let offer = b.svc.next_offer().await.unwrap();
    assert_eq!(offer.key_status(), siar_files::KeyStatus::Present);
    assert_eq!(offer.display_name(), Some("wrapped.png"));
    offer.accept(policy("wrapped-quic")).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(20), b.svc.next_received())
        .await
        .expect("received over QUIC")
        .unwrap();
    assert_eq!(
        b.svc
            .open_attachment(&got.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_key_wrapped_against_the_wrong_public_key_cannot_be_opened() {
    let (a, b) = pair().await;
    // A believes B's key is someone else's: it wraps for the wrong secret.
    let impostor = DeviceIdentity::generate();
    a.book
        .register_with_key(b.device, b.addr(), impostor.x25519_public().to_bytes());
    a.svc
        .send_file(
            Destination::Device(b.device),
            &payload(80_000),
            wrapped_policy("secret.bin"),
        )
        .await
        .unwrap();
    let offer = b.svc.next_offer().await.unwrap();
    assert_eq!(offer.key_status(), siar_files::KeyStatus::WrapUnopenable);
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("wrong-key")).await.unwrap();
    eventually("the ciphertext still verifies and lands", || {
        b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), b.svc.next_received())
            .await
            .is_err(),
        "no key, so nothing to announce"
    );
}

#[tokio::test]
async fn a_peer_with_no_recorded_x25519_key_gets_no_wrap_at_all() {
    let (a, b) = pair().await;
    // Re-registered for transport only (an address update keeps a known
    // key, so forget the peer first): no key, so no secret, so no guess.
    a.book.forget(b.device);
    a.book.register(b.device, b.addr());
    let r = a
        .svc
        .send_file(
            Destination::Device(b.device),
            &payload(5_000),
            wrapped_policy("x"),
        )
        .await;
    assert_eq!(
        r.err(),
        Some(siar_files::FilesError::NoPairwiseSecret(b.device))
    );
}
