//! End to end: a real sender store → `files/1` frames → a real receiver
//! engine, with the event log and reference model behind it.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::Bytes;
use siar_blob_manifest::blob_io::{BlobStore, BlobWriter};
use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::file_offer::{FileOffer, FileOfferDecision};
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, TransferId};
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::{build_manifest, BlobManifest};
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::transfer_state::TransferState;
use siar_blob_manifest::wire::{
    decode_control, encode_control, FileControlV1, FileOfferV1, FilesCapabilities, FilesFeatures,
    WireLimits,
};
use siar_domain::{BlobSize, DeviceId};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, InMemoryEventStore};
use siar_file_transfer_service::gc_schedule::GcScheduler;
use siar_file_transfer_service::protocol::{
    ChunkOutcome, FileReceiver, FileSender, ProtocolError, ReceiveContext, SendStatus,
};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use siar_file_transfer_service::FileTransferService;

struct Root(std::path::PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn root() -> Root {
    Root(std::env::temp_dir().join(format!("siar-proto-{}", uuid::Uuid::new_v4())))
}

type Meta = InMemoryBlobMetadata;
type Engine = TransferEngine<Meta, InMemoryReferenceStore>;

fn open_store(r: &Root) -> FsBlobStore<Meta> {
    let cfg = FsStoreConfig {
        limits: StoreLimits::default(),
        durability: DurabilityPolicy::None,
        ..FsStoreConfig::default()
    };
    FsBlobStore::open(&r.0, cfg, Meta::new()).unwrap()
}

fn engine(r: &Root, refs: &Arc<InMemoryReferenceStore>) -> Engine {
    let service = FileTransferService::new().with_event_log(Arc::new(InMemoryEventStore::new()));
    TransferEngine::new(
        service,
        open_store(r),
        refs.clone(),
        NamespaceId::new("personal").unwrap(),
    )
}

struct Fx {
    bytes: Vec<u8>,
    descriptor: BlobDescriptor,
    manifest: Arc<BlobManifest>,
}

fn fixture(len: usize) -> Fx {
    let mut x = 0x1234_5678u32;
    let bytes: Vec<u8> = (0..len)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect();
    let manifest = build_manifest(&bytes, 1_000, &ManifestLimits::default()).unwrap();
    Fx {
        descriptor: BlobDescriptor {
            blob_id: BlobId::from_ciphertext(&bytes),
            size: BlobSize::parse(len as u64).unwrap(),
            chunking: ChunkingDescriptor { chunk_size: 1_000 },
            encryption: EncryptionDescriptor {
                algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                nonce: [0; 12],
            },
            media_type: None,
            manifest_id: None,
        },
        manifest: Arc::new(manifest),
        bytes,
    }
}

/// Publishes the fixture into a sender-side store, returning its reader.
async fn seed(store: &FsBlobStore<Meta>, f: &Fx) -> <FsBlobStore<Meta> as BlobStore>::Reader {
    let mut w = store.begin_write(f.descriptor).await.unwrap();
    for c in &f.manifest.chunks {
        let s = c.offset as usize;
        w.write_chunk(
            c.index,
            Bytes::copy_from_slice(&f.bytes[s..s + c.size as usize]),
        )
        .await
        .unwrap();
    }
    w.finalize().await.unwrap();
    store.open(f.descriptor.blob_id).await.unwrap()
}

fn ctx(name: &str) -> ReceiveContext {
    ReceiveContext {
        object: ObjectId::new(name).unwrap(),
        role: BlobRole::MessageAttachment,
        expires: TransferExpiry(u64::MAX),
        origin: EventOrigin::LocalDevice(DeviceId::new()),
        logical_attachment_id: LogicalAttachmentId::new(),
    }
}

fn offer_for(f: &Fx, t: TransferId) -> FileOffer {
    FileOffer {
        transfer_id: t,
        blob: f.descriptor,
        encrypted_metadata: vec![],
        expiry_millis: None,
    }
}

/// Everything crosses as bytes: encode then decode, as a transport would.
fn over_wire(msg: &FileControlV1) -> FileControlV1 {
    decode_control(&encode_control(msg).unwrap(), &WireLimits::default()).unwrap()
}

struct Pair {
    _s: Root,
    _r: Root,
    f: Fx,
    refs: Arc<InMemoryReferenceStore>,
    recv: Arc<Engine>,
    sender_store: FsBlobStore<Meta>,
}

async fn pair(len: usize) -> Pair {
    let (s, r) = (root(), root());
    let refs = Arc::new(InMemoryReferenceStore::new());
    let f = fixture(len);
    let sender_store = open_store(&s);
    Pair {
        recv: Arc::new(engine(&r, &refs)),
        _s: s,
        _r: r,
        f,
        refs,
        sender_store,
    }
}

#[tokio::test]
async fn a_file_crosses_the_wire_and_lands_verified() {
    let p = pair(7_500).await;
    let reader = seed(&p.sender_store, &p.f).await;
    let limits = WireLimits::default();
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), limits, caps);
    let negotiated = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    let tx = FileSender::new(t, p.f.manifest.clone(), reader, &negotiated);

    // Offer, answered by policy.
    let wire_offer = over_wire(&FileControlV1::Offer(FileOfferV1::from(&offer_for(
        &p.f, t,
    ))));
    let FileControlV1::Offer(o) = wire_offer else {
        panic!()
    };
    rx.on_offer(o, negotiated, 0).unwrap();
    let reply = over_wire(&rx.decide(t, FileOfferDecision::Accept).unwrap());
    assert_eq!(reply, FileControlV1::ManifestRequest { transfer_id: t });

    // Manifest, then the first request for every chunk.
    let request = rx
        .on_manifest(&tx.manifest_frame().unwrap(), ctx("m1"))
        .await
        .unwrap();
    let FileControlV1::ChunkRequest(req) = over_wire(&request) else {
        panic!("expected a chunk request")
    };
    assert_eq!(req.ranges, vec![(0, 8)]);

    // Chunks.
    let mut wire = Vec::new();
    assert_eq!(tx.serve(&req, &mut wire).await.unwrap(), 8);
    let mut cursor = wire.as_slice();
    let mut done = None;
    for _ in 0..8 {
        match rx.on_chunk(t, &mut cursor).await.unwrap() {
            ChunkOutcome::Progress(_) => {}
            ChunkOutcome::Complete(m) => done = Some(m),
        }
    }
    assert!(cursor.is_empty());
    let done = over_wire(&done.expect("last chunk completes"));
    assert_eq!(
        tx.on_receiver_message(&done).unwrap(),
        SendStatus::Completed
    );

    assert_eq!(
        p.recv.service().current_state(t),
        Some(TransferState::Completed)
    );
    assert!(p.recv.store().contains_sync(p.f.descriptor.blob_id));
}

#[tokio::test]
async fn resume_after_pause_only_requests_what_is_missing() {
    let p = pair(7_500).await;
    let reader = seed(&p.sender_store, &p.f).await;
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    let tx = FileSender::new(t, p.f.manifest.clone(), reader, &n);
    rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0)
        .unwrap();
    rx.decide(t, FileOfferDecision::Accept).unwrap();
    rx.on_manifest(&tx.manifest_frame().unwrap(), ctx("m"))
        .await
        .unwrap();

    // Chunks 0..3 arrive, then the user pauses.
    let mut wire = Vec::new();
    tx.serve(
        &siar_blob_manifest::wire::ChunkRequestV1 {
            transfer_id: t,
            ranges: vec![(0, 3)],
        },
        &mut wire,
    )
    .await
    .unwrap();
    let mut cur = wire.as_slice();
    for _ in 0..3 {
        rx.on_chunk(t, &mut cur).await.unwrap();
    }
    let pause = over_wire(&rx.pause(t).await.unwrap());
    assert_eq!(tx.on_receiver_message(&pause).unwrap(), SendStatus::Paused);
    let blocked = tx
        .serve(
            &siar_blob_manifest::wire::ChunkRequestV1 {
                transfer_id: t,
                ranges: vec![(3, 8)],
            },
            &mut Vec::new(),
        )
        .await;
    assert_eq!(blocked, Err(ProtocolError::Paused));

    let resume = over_wire(&rx.resume(t).await.unwrap());
    let FileControlV1::Resume(req) = &resume else {
        panic!()
    };
    assert_eq!(
        req.ranges,
        vec![(3, 8)],
        "nothing already received is re-requested"
    );
    assert_eq!(
        tx.on_receiver_message(&resume).unwrap(),
        SendStatus::InFlight
    );
    let mut wire = Vec::new();
    assert_eq!(tx.serve(req, &mut wire).await.unwrap(), 5);
    let mut cur = wire.as_slice();
    let mut complete = false;
    for _ in 0..5 {
        complete |= matches!(
            rx.on_chunk(t, &mut cur).await.unwrap(),
            ChunkOutcome::Complete(_)
        );
    }
    assert!(complete);
}

#[tokio::test]
async fn a_manifest_for_a_different_blob_is_refused_before_anything_starts() {
    let p = pair(3_000).await;
    let other = fixture(3_500);
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0)
        .unwrap();
    rx.decide(t, FileOfferDecision::Accept).unwrap();

    let reader = seed(&p.sender_store, &other).await;
    let liar = FileSender::new(t, other.manifest.clone(), reader, &n);
    assert_eq!(
        rx.on_manifest(&liar.manifest_frame().unwrap(), ctx("m"))
            .await,
        Err(ProtocolError::ManifestNotOffered)
    );
    assert_eq!(
        p.recv.service().current_state(t),
        None,
        "engine never started"
    );
}

#[tokio::test]
async fn offers_outside_the_negotiated_envelope_or_expired_or_repeated_are_refused() {
    let p = pair(3_000).await;
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();

    let mut tiny = n;
    tiny.max_object_size = 10;
    let t = TransferId::new();
    assert_eq!(
        rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), tiny, 0),
        Err(ProtocolError::OutsideNegotiated)
    );

    let mut expired = offer_for(&p.f, TransferId::new());
    expired.expiry_millis = Some(100);
    let id = expired.transfer_id;
    assert_eq!(
        rx.on_offer(FileOfferV1::from(&expired), n, 100),
        Err(ProtocolError::OfferExpired(id))
    );

    rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0)
        .unwrap();
    assert_eq!(
        rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0),
        Err(ProtocolError::Duplicate(t))
    );

    // Declining drops the offer, and a manifest for it is then unknown.
    let reply = rx.decide(t, FileOfferDecision::Reject).unwrap();
    assert!(matches!(reply, FileControlV1::OfferReply { .. }));
    assert_eq!(
        rx.decide(t, FileOfferDecision::Accept),
        Err(ProtocolError::UnknownTransfer(t))
    );
}

#[tokio::test]
async fn a_forged_chunk_is_caught_and_the_transfer_stays_resumable() {
    let p = pair(3_000).await;
    let reader = seed(&p.sender_store, &p.f).await;
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    let tx = FileSender::new(t, p.f.manifest.clone(), reader, &n);
    rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0)
        .unwrap();
    rx.decide(t, FileOfferDecision::Accept).unwrap();
    rx.on_manifest(&tx.manifest_frame().unwrap(), ctx("m"))
        .await
        .unwrap();

    let mut wire = Vec::new();
    tx.serve(
        &siar_blob_manifest::wire::ChunkRequestV1 {
            transfer_id: t,
            ranges: vec![(0, 1)],
        },
        &mut wire,
    )
    .await
    .unwrap();
    let last = wire.len() - 1;
    wire[last] ^= 0xFF; // body corrupted in transit; header intact
    assert!(rx.on_chunk(t, &mut wire.as_slice()).await.is_err());

    // The honest retransmission still works.
    let mut clean = Vec::new();
    tx.serve(
        &siar_blob_manifest::wire::ChunkRequestV1 {
            transfer_id: t,
            ranges: vec![(0, 3)],
        },
        &mut clean,
    )
    .await
    .unwrap();
    let mut cur = clean.as_slice();
    let mut complete = false;
    for _ in 0..3 {
        complete |= matches!(
            rx.on_chunk(t, &mut cur).await.unwrap(),
            ChunkOutcome::Complete(_)
        );
    }
    assert!(complete);
}

#[tokio::test]
async fn sender_side_cancel_and_bad_requests_are_handled() {
    let p = pair(3_000).await;
    let reader = seed(&p.sender_store, &p.f).await;
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    let tx = FileSender::new(t, p.f.manifest.clone(), reader, &n);

    // A range past the manifest, and a request for another transfer.
    let bad = siar_blob_manifest::wire::ChunkRequestV1 {
        transfer_id: t,
        ranges: vec![(0, 99)],
    };
    assert!(tx.serve(&bad, &mut Vec::new()).await.is_err());
    let foreign = siar_blob_manifest::wire::ChunkRequestV1 {
        transfer_id: TransferId::new(),
        ranges: vec![(0, 1)],
    };
    assert!(matches!(
        tx.serve(&foreign, &mut Vec::new()).await,
        Err(ProtocolError::UnknownTransfer(_))
    ));

    // Completion is only believed for BlobVerified on this very blob.
    let weak = FileControlV1::Complete(siar_blob_manifest::wire::TransferCompleteV1 {
        transfer_id: t,
        blob_id: p.f.descriptor.blob_id,
        ack: siar_blob_manifest::transfer_control::TransferAckKind::TransportAck,
    });
    assert_eq!(tx.on_receiver_message(&weak).unwrap(), SendStatus::InFlight);

    let cancel = FileControlV1::Cancel(siar_blob_manifest::wire::CancelV1 {
        transfer_id: t,
        keep_partial: false,
        reason: None,
    });
    assert_eq!(
        tx.on_receiver_message(&cancel).unwrap(),
        SendStatus::Cancelled
    );
}

#[tokio::test]
async fn a_negotiated_ceiling_below_the_chunk_size_blocks_the_offer() {
    let p = pair(3_000).await;
    let mut weak = FilesCapabilities::desktop_default();
    weak.max_chunk_size = 500;
    weak.features = FilesFeatures::BASELINE;
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), weak);
    let n = rx
        .negotiate_with(&FilesCapabilities::desktop_default())
        .unwrap();
    assert_eq!(
        rx.on_offer(FileOfferV1::from(&offer_for(&p.f, TransferId::new())), n, 0),
        Err(ProtocolError::OutsideNegotiated)
    );
}

#[tokio::test]
async fn the_gc_scheduler_sweeps_when_due_and_spares_in_flight_blobs() {
    let p = pair(3_000).await;
    let caps = FilesCapabilities::desktop_default();
    let rx = FileReceiver::new(p.recv.clone(), WireLimits::default(), caps);
    let n = rx.negotiate_with(&caps).unwrap();
    let t = TransferId::new();
    rx.on_offer(FileOfferV1::from(&offer_for(&p.f, t)), n, 0)
        .unwrap();
    rx.decide(t, FileOfferDecision::Accept).unwrap();
    let reader = seed(&p.sender_store, &p.f).await;
    let tx = FileSender::new(t, p.f.manifest.clone(), reader, &n);
    rx.on_manifest(&tx.manifest_frame().unwrap(), ctx("m"))
        .await
        .unwrap();
    assert!(p.recv.active_blob_ids().contains(&p.f.descriptor.blob_id));

    let gc = GcScheduler::new(60_000, 64);
    let policy = GcPolicy {
        grace_period_millis: 0,
    };
    let pinned = HashSet::new();
    let first = gc.tick(&p.recv, &pinned, 1_000, &policy).unwrap();
    assert!(first.is_some(), "never run → due");
    assert!(gc.tick(&p.recv, &pinned, 2_000, &policy).unwrap().is_none());
    assert!(gc
        .tick(&p.recv, &pinned, 61_000, &policy)
        .unwrap()
        .is_some());
    assert_eq!(
        p.refs.count_for(p.f.descriptor.blob_id).unwrap(),
        1,
        "reference intact"
    );
}
