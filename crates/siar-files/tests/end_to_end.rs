//! Two `FilesService`s, real stores and engines, joined by an in-memory
//! channel that delivers frames in order — what a transport provides.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_domain::{DeviceId, MediaType};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, InMemoryEventStore};
use siar_file_transfer_service::transfer_engine::TransferEngine;
use siar_file_transfer_service::FileTransferService;
use siar_files::{
    ChannelError, CiphertextSource, Destination, DestinationPolicy, FileChannel, FileSendPolicy,
    FilesConfig, FilesError, FilesService, FrameKind, FsFileSink, FsFileSource, MemorySink,
};
use tokio::sync::mpsc;

struct Root(std::path::PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Meta = InMemoryBlobMetadata;
type Refs = InMemoryReferenceStore;
type Svc = FilesService<Link, Meta, Refs>;

/// Sends into the peer's inbox, tagged with who it is from.
struct Link {
    me: DeviceId,
    to_peer: mpsc::UnboundedSender<(DeviceId, u8, Vec<u8>)>,
    drop_chunks: Arc<std::sync::atomic::AtomicBool>,
    meter: Arc<Meter>,
}

/// Measures chunk sends (§41-42) and degrades the network on demand
/// (§190): loss, duplication, jitter-induced reordering, and a lost
/// completion acknowledgement. Deterministic xorshift, so a failure
/// reproduces.
#[derive(Default)]
struct Meter {
    delay_ms: std::sync::atomic::AtomicU64,
    inflight: std::sync::atomic::AtomicUsize,
    max_inflight: std::sync::atomic::AtomicUsize,
    chunk_frames: std::sync::atomic::AtomicUsize,
    lost_frames: std::sync::atomic::AtomicUsize,
    duplicated_frames: std::sync::atomic::AtomicUsize,
    loss_permille: std::sync::atomic::AtomicU64,
    dup_permille: std::sync::atomic::AtomicU64,
    jitter_ms: std::sync::atomic::AtomicU64,
    drop_complete: std::sync::atomic::AtomicBool,
    /// How many `Offer` frames to swallow before letting any through.
    drop_offers: std::sync::atomic::AtomicUsize,
    rng: std::sync::atomic::AtomicU64,
    /// Per-destination delivery for multi-device topologies; empty means
    /// "the one peer" (`Link::to_peer`).
    routes: std::sync::Mutex<Routes>,
}

/// Where each destination's frames go (empty: the single peer).
type Routes = std::collections::HashMap<DeviceId, mpsc::UnboundedSender<(DeviceId, u8, Vec<u8>)>>;

impl Meter {
    fn next_rand(&self) -> u64 {
        use std::sync::atomic::Ordering::Relaxed;
        let mut x = self.rng.load(Relaxed) | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng.store(x, Relaxed);
        x
    }
}

impl FileChannel for Link {
    async fn send_frame(
        &self,
        to: DeviceId,
        kind: FrameKind,
        frame: Vec<u8>,
    ) -> Result<(), ChannelError> {
        use std::sync::atomic::Ordering::Relaxed;
        let m = &self.meter;
        let dest_tx = m
            .routes
            .lock()
            .unwrap()
            .get(&to)
            .cloned()
            .unwrap_or_else(|| self.to_peer.clone());
        if kind == FrameKind::Control && m.drop_complete.load(Relaxed) {
            if let Ok(siar_blob_manifest::wire::FileControlV1::Complete(_)) =
                siar_blob_manifest::wire::decode_control(
                    &frame,
                    &siar_blob_manifest::wire::WireLimits::default(),
                )
            {
                return Ok(()); // the acknowledgement is lost in transit
            }
        }
        if kind == FrameKind::Control && m.drop_offers.load(Relaxed) > 0 {
            if let Ok(siar_blob_manifest::wire::FileControlV1::Offer(_)) =
                siar_blob_manifest::wire::decode_control(
                    &frame,
                    &siar_blob_manifest::wire::WireLimits::default(),
                )
            {
                m.drop_offers.fetch_sub(1, Relaxed);
                return Ok(()); // the offer never reaches the receiver
            }
        }
        if kind != FrameKind::Chunk {
            return dest_tx
                .send((self.me, kind as u8, frame))
                .map_err(|_| ChannelError::Closed);
        }
        if self.drop_chunks.load(Relaxed) {
            return Err(ChannelError::Unreachable);
        }
        m.chunk_frames.fetch_add(1, Relaxed);
        let now = m.inflight.fetch_add(1, Relaxed) + 1;
        m.max_inflight.fetch_max(now, Relaxed);
        let d = m.delay_ms.load(Relaxed);
        if d > 0 {
            tokio::time::sleep(Duration::from_millis(d)).await;
        }
        m.inflight.fetch_sub(1, Relaxed);

        let r = m.next_rand();
        if (r % 1000) < m.loss_permille.load(Relaxed) {
            m.lost_frames.fetch_add(1, Relaxed);
            return Ok(()); // lost on the wire; the sender cannot tell
        }
        let dup = ((r >> 10) % 1000) < m.dup_permille.load(Relaxed);
        let jitter = m.jitter_ms.load(Relaxed);
        let mut deliveries = vec![(r >> 20) % (jitter + 1)];
        if dup {
            m.duplicated_frames.fetch_add(1, Relaxed);
            deliveries.push(((r >> 30) % (jitter + 1)) + 1);
        }
        for delay in deliveries {
            let (tx, me, f) = (dest_tx.clone(), self.me, frame.clone());
            if delay == 0 {
                let _ = tx.send((me, kind as u8, f));
            } else {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    let _ = tx.send((me, kind as u8, f));
                });
            }
        }
        Ok(())
    }
}

struct Node {
    meter: Arc<Meter>,
    id: DeviceId,
    svc: Arc<Svc>,
    refs: Arc<Refs>,
    _root: Root,
}

struct Net {
    a: Node,
    b: Node,
    drop_a_chunks: Arc<std::sync::atomic::AtomicBool>,
}

fn node(
    id: DeviceId,
    to_peer: mpsc::UnboundedSender<(DeviceId, u8, Vec<u8>)>,
    drop_chunks: Arc<std::sync::atomic::AtomicBool>,
) -> Node {
    node_with(id, to_peer, drop_chunks, |_| {})
}

fn node_with(
    id: DeviceId,
    to_peer: mpsc::UnboundedSender<(DeviceId, u8, Vec<u8>)>,
    drop_chunks: Arc<std::sync::atomic::AtomicBool>,
    hook: impl FnOnce(&mut FilesConfig),
) -> Node {
    let meter = Arc::new(Meter::default());
    let root = Root(std::env::temp_dir().join(format!("siar-files-{}", uuid::Uuid::new_v4())));
    let store = FsBlobStore::open(
        &root.0,
        FsStoreConfig {
            limits: StoreLimits::default(),
            durability: DurabilityPolicy::None,
            ..FsStoreConfig::default()
        },
        Meta::new(),
    )
    .unwrap();
    let refs = Arc::new(Refs::new());
    let ns = NamespaceId::new("personal").unwrap();
    let service = FileTransferService::new().with_event_log(Arc::new(InMemoryEventStore::new()));
    let engine = Arc::new(TransferEngine::new(
        service,
        store,
        refs.clone(),
        ns.clone(),
    ));
    let mut cfg = FilesConfig::new(ns, EventOrigin::LocalDevice(id));
    cfg.stall_millis = 100;
    hook(&mut cfg);
    let svc = FilesService::new(
        engine,
        refs.clone(),
        Link {
            me: id,
            to_peer,
            drop_chunks,
            meter: meter.clone(),
        },
        cfg,
    );
    Node {
        meter,
        id,
        svc,
        refs,
        _root: root,
    }
}

/// Builds two nodes and spawns each one's receive loop.
fn net() -> Net {
    let (ida, idb) = (DeviceId::new(), DeviceId::new());
    let (tx_to_a, rx_a) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let (tx_to_b, rx_b) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let drop_a = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a = node(ida, tx_to_b, drop_a.clone());
    let b = node(
        idb,
        tx_to_a,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    for (svc, mut rx) in [(a.svc.clone(), rx_a), (b.svc.clone(), rx_b)] {
        tokio::spawn(async move {
            while let Some((from, kind, frame)) = rx.recv().await {
                if let Err(e) = svc.handle_frame(from, kind, &frame).await {
                    eprintln!("handle_frame: {e}");
                }
            }
        });
    }
    Net {
        a,
        b,
        drop_a_chunks: drop_a,
    }
}
fn payload(len: usize) -> Vec<u8> {
    let mut x = 0xC0FFEEu32;
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
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test]
async fn a_file_goes_device_to_device_with_no_conversation() {
    let n = net();
    let data = payload(300_000);
    let _handle =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &data,
                FileSendPolicy::default(),
            )
            .await
            .unwrap();

    let offer = tokio::time::timeout(Duration::from_secs(5), n.b.svc.next_offer())
        .await
        .expect("an offer arrives")
        .unwrap();
    assert_eq!(offer.from, n.a.id);
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("saved-1")).await.unwrap();

    eventually("blob published on the receiver", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    // Same bytes on both sides, and the receiver holds a reference.
    assert_eq!(
        n.a.svc.ciphertext(blob.as_bytes()),
        n.b.svc.ciphertext(blob.as_bytes())
    );
    assert_eq!(n.b.refs.count_for(blob).unwrap(), 1);
}

#[tokio::test]
async fn a_rejected_offer_moves_nothing_and_frees_the_sender() {
    let n = net();
    let h =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(10_000),
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.reject().await.unwrap();
    eventually("sender releases the declined blob", || {
        n.a.refs.count_for(blob).unwrap() == 0
    })
    .await;
    assert!(n.b.svc.ciphertext(blob.as_bytes()).is_none());
    // The handle is now stale, not a crash.
    assert_eq!(h.cancel().await, Err(FilesError::UnknownTransfer));
}

#[tokio::test]
async fn the_sender_can_cancel_and_the_receiver_cleans_up() {
    let n = net();
    let h =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(20_000),
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    h.cancel().await.unwrap();
    assert_eq!(n.a.refs.count_for(blob).unwrap(), 0);
    // Accepting a cancelled offer must not resurrect anything.
    let _ = offer.accept(policy("late")).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(n.b.svc.ciphertext(blob.as_bytes()).is_none());
}

#[tokio::test]
async fn pause_stops_chunks_and_resume_lets_the_receiver_finish() {
    let n = net();
    let data = payload(200_000);
    // Chunks cannot get through at first.
    n.drop_a_chunks
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let h =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &data,
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let (blob, id) = (offer.offer.blob.blob_id, offer.offer.transfer_id);
    offer.accept(policy("p")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        n.b.svc.ciphertext(blob.as_bytes()).is_none(),
        "nothing arrived"
    );

    // The link recovers; the receiver resumes asking only for what is missing.
    n.drop_a_chunks
        .store(false, std::sync::atomic::Ordering::Relaxed);
    h.pause().await.unwrap();
    h.resume().await.unwrap();
    n.b.svc.pause_incoming(id).await.unwrap();
    n.b.svc.resume_incoming(id).await.unwrap();
    eventually("blob completes after resume", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
}

#[tokio::test]
async fn an_attachment_is_imported_referenced_and_opened() {
    let n = net();
    let data = payload(50_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::ImageJpeg)
            .await
            .unwrap();
    // Held by the import owner until bound to a message.
    assert_eq!(n.a.refs.count_for(imported.blob_id).unwrap(), 1);

    let reference = imported.reference().unwrap();
    assert_eq!(reference.blob_hash, *imported.blob_id.as_bytes());
    assert_eq!(n.a.svc.open_attachment(&reference).await.unwrap(), data);

    // Bind to a message: still exactly one reference, now the message's.
    let msg = ObjectId::new("msg-1").unwrap();
    n.a.svc
        .bind_to_object(&imported, msg.clone(), BlobRole::MessageAttachment)
        .unwrap();
    assert_eq!(n.a.refs.count_for(imported.blob_id).unwrap(), 1);
    let ns = NamespaceId::new("personal").unwrap();
    let refs = n.a.refs.references_of_object(&ns, &msg).unwrap();
    assert_eq!(refs.len(), 1);

    // A wrong key opens nothing.
    let mut forged = reference.clone();
    forged.attachment_key[0] ^= 1;
    assert!(n.a.svc.open_attachment(&forged).await.is_err());
}

#[tokio::test]
async fn gc_spares_an_imported_blob_and_reclaims_it_once_unreferenced() {
    use siar_blob_manifest::reference_store::BlobReference;
    use siar_blob_manifest::reference_store::RefOp;
    use siar_blob_manifest::storage_lifecycle::GcPolicy;
    let n = net();
    let imported =
        n.a.svc
            .import_attachment(&payload(5_000), MediaType::Other)
            .await
            .unwrap();
    let policy = GcPolicy {
        grace_period_millis: 0,
    };
    let pinned = HashSet::new();
    n.a.svc.engine().gc_sweep(&pinned, 1, &policy, 16).unwrap();
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_some());

    n.a.refs
        .apply(&[RefOp::Remove(BlobReference {
            owner_namespace: NamespaceId::new("personal").unwrap(),
            object_id: imported.import_owner.clone(),
            blob_id: imported.blob_id,
            role: BlobRole::Transfer,
        })])
        .unwrap();
    // First sweep starts the grace clock, the next one deletes.
    n.a.svc.engine().gc_sweep(&pinned, 10, &policy, 16).unwrap();
    n.a.svc.engine().gc_sweep(&pinned, 20, &policy, 16).unwrap();
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_none());
}

#[tokio::test]
async fn path_sources_work_and_missing_or_oversized_ones_are_refused() {
    let n = net();
    let p = std::env::temp_dir().join(format!("siar-files-src-{}", uuid::Uuid::new_v4()));
    std::fs::write(&p, payload(8_000)).unwrap();
    let src = FsFileSource::open(&p).unwrap();
    assert!(n
        .a
        .svc
        .import_attachment(&src, MediaType::Other)
        .await
        .is_ok());
    std::fs::remove_file(&p).unwrap();
    assert!(matches!(
        FsFileSource::open(&p),
        Err(siar_files::FileError::Io(_))
    ));
    assert!(matches!(
        FsFileSource::open(std::env::temp_dir()),
        Err(siar_files::FileError::NotARegularFile)
    ));
}

#[tokio::test]
async fn an_attachment_is_exported_to_a_user_chosen_destination() {
    let n = net();
    let data = payload(700_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();

    // Memory sink: all bytes, only after commit.
    let sink = MemorySink::new();
    let handle = sink.handle();
    let written = n.a.svc.export_attachment(&reference, sink).await.unwrap();
    assert_eq!(written, data.len() as u64);
    assert_eq!(handle.lock().unwrap().as_deref(), Some(&data[..]));

    // Filesystem sink: appears atomically at the destination.
    let dir = std::env::temp_dir().join(format!("siar-export-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("saved.bin");
    n.a.svc
        .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);

    // A wrong key writes nothing: no destination, no leftover temp file.
    let mut forged = reference.clone();
    forged.attachment_key[3] ^= 1;
    let dest2 = dir.join("forged.bin");
    assert!(n
        .a
        .svc
        .export_attachment(&forged, FsFileSink::create(&dest2).unwrap())
        .await
        .is_err());
    assert!(!dest2.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn verified_bytes_come_only_from_an_authenticated_open() {
    let n = net();
    let data = payload(1_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::ImagePng)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    let v = n.a.svc.open_verified_attachment(&reference).await.unwrap();
    assert_eq!(v.as_inner(), &data);
    assert_eq!(format!("{v:?}"), "Verified(..)");
    let mut forged = reference;
    forged.attachment_key[0] ^= 1;
    assert!(n.a.svc.open_verified_attachment(&forged).await.is_err());
}

#[tokio::test]
async fn frames_from_strangers_and_unknown_kinds_are_errors_not_state() {
    let n = net();
    let stranger = DeviceId::new();
    assert_eq!(
        n.b.svc.handle_frame(stranger, 99, &[]).await,
        Err(FilesError::UnknownFrameKind(99))
    );
    // A chunk for a transfer nobody started.
    assert!(n
        .b
        .svc
        .handle_frame(stranger, FrameKind::Chunk as u8, &[0u8; 100])
        .await
        .is_err());
    // An offer before any capability exchange.
    let bogus =
        siar_blob_manifest::wire::encode_control(&siar_blob_manifest::wire::FileControlV1::Pause {
            transfer_id: siar_blob_manifest::ids::TransferId::new(),
        })
        .unwrap();
    assert_eq!(
        n.b.svc
            .handle_frame(stranger, FrameKind::Control as u8, &bogus)
            .await,
        Err(FilesError::UnknownTransfer)
    );
}

#[tokio::test]
async fn maintenance_runs_when_due_and_reclaims_unreferenced_blobs() {
    use siar_blob_manifest::reference_store::{BlobReference, RefOp};
    let n = net();
    let imported =
        n.a.svc
            .import_attachment(&payload(5_000), MediaType::Other)
            .await
            .unwrap();
    n.a.refs
        .apply(&[RefOp::Remove(BlobReference {
            owner_namespace: NamespaceId::new("personal").unwrap(),
            object_id: imported.import_owner.clone(),
            blob_id: imported.blob_id,
            role: BlobRole::Transfer,
        })])
        .unwrap();
    // Default config has a 24 h grace period: the first pass only starts
    // the clock; a pass after the grace period deletes.
    assert!(n.a.svc.maintenance(1_000).unwrap().is_some());
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_some());
    assert!(n.a.svc.maintenance(2_000).unwrap().is_none(), "not due yet");
    let day = 24 * 60 * 60 * 1000;
    let later = 1_000 + 11 * 60 * 1000 + day;
    assert!(n.a.svc.maintenance(later).unwrap().is_some());
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_none());
}

#[tokio::test]
async fn several_transfers_to_one_peer_all_complete_through_the_scheduler() {
    let n = net();
    let mut expected = Vec::new();
    for (i, len) in [400_000usize, 3_000, 90_000].into_iter().enumerate() {
        let class = if i == 1 {
            siar_blob_manifest::scheduling::TransferClass::InteractiveAttachment
        } else {
            siar_blob_manifest::scheduling::TransferClass::Bulk
        };
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(len),
                FileSendPolicy {
                    class,
                    ..FileSendPolicy::default()
                },
            )
            .await
            .unwrap();
        let offer = n.b.svc.next_offer().await.unwrap();
        expected.push(offer.offer.blob.blob_id);
        offer.accept(policy(&format!("multi-{i}"))).await.unwrap();
    }
    for blob in expected {
        eventually("every transfer completes", || {
            n.b.svc.ciphertext(blob.as_bytes()).is_some()
        })
        .await;
    }
}

#[tokio::test]
async fn the_plaintext_cache_is_off_by_default_and_bounded_when_enabled() {
    use siar_blob_manifest::plaintext_cache::PlaintextPolicy;
    let n = net();
    let imported =
        n.a.svc
            .import_attachment(&payload(2_000), MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    // Default: opening twice decrypts twice and caches nothing. Proven
    // by deleting the blob: the second open must fail, not hit a cache.
    n.a.svc.open_attachment(&reference).await.unwrap();
    n.a.svc
        .engine()
        .store()
        .delete_sync(imported.blob_id)
        .unwrap();
    assert!(n.a.svc.open_attachment(&reference).await.is_err());

    // Opted in: the second open is served from the bounded cache even
    // with the blob gone, and clearing the cache ends that.
    let root = std::env::temp_dir().join(format!("siar-files-pc-{}", uuid::Uuid::new_v4()));
    let store = FsBlobStore::open(
        &root,
        FsStoreConfig {
            limits: StoreLimits::default(),
            durability: DurabilityPolicy::None,
            ..FsStoreConfig::default()
        },
        Meta::new(),
    )
    .unwrap();
    let refs = Arc::new(Refs::new());
    let ns = NamespaceId::new("personal").unwrap();
    let engine = Arc::new(TransferEngine::new(
        FileTransferService::new().with_event_log(Arc::new(InMemoryEventStore::new())),
        store,
        refs.clone(),
        ns.clone(),
    ));
    let (tx, _rx) = mpsc::unbounded_channel();
    let id = DeviceId::new();
    let mut cfg = FilesConfig::new(ns, EventOrigin::LocalDevice(id));
    cfg.plaintext = PlaintextPolicy::Temporary {
        max_bytes: 10_000,
        ttl_millis: 60_000,
    };
    let svc = FilesService::new(
        engine,
        refs,
        Link {
            me: id,
            to_peer: tx,
            drop_chunks: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            meter: Arc::new(Meter::default()),
        },
        cfg,
    );
    let data = payload(2_000);
    let imp = svc
        .import_attachment(&data, MediaType::Other)
        .await
        .unwrap();
    let r = imp.reference().unwrap();
    assert_eq!(svc.open_attachment(&r).await.unwrap(), data);
    svc.engine().store().delete_sync(imp.blob_id).unwrap();
    assert_eq!(svc.open_attachment(&r).await.unwrap(), data, "cache hit");
    svc.clear_plaintext_cache();
    assert!(svc.open_attachment(&r).await.is_err());
    let _ = std::fs::remove_dir_all(root);
}

/// Counts the most bytes ever requested in one read: proof the streaming
/// import never asks for the whole file.
struct Probe {
    data: Vec<u8>,
    max_read: std::sync::atomic::AtomicUsize,
}

impl siar_files::FileSource for Probe {
    async fn len(&self) -> Result<u64, siar_files::FileError> {
        Ok(self.data.len() as u64)
    }
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, siar_files::FileError> {
        self.max_read
            .fetch_max(buf.len(), std::sync::atomic::Ordering::Relaxed);
        self.data.read_at(offset, buf).await
    }
}

#[tokio::test]
async fn streaming_import_round_trips_and_never_reads_the_whole_file_at_once() {
    let n = net();
    let data = payload(3_000_000);
    let probe = Probe {
        data: data.clone(),
        max_read: Default::default(),
    };
    let imported =
        n.a.svc
            .import_attachment_streaming(&probe, MediaType::Other)
            .await
            .unwrap();
    let max = probe.max_read.load(std::sync::atomic::Ordering::Relaxed);
    assert!(max <= 256 * 1024, "largest single read was {max} bytes");
    let reference = imported.reference().unwrap();
    assert_eq!(n.a.svc.open_attachment(&reference).await.unwrap(), data);

    // The stored manifest/blob id are the real ones.
    let ct = n.a.svc.ciphertext(imported.blob_id.as_bytes()).unwrap();
    assert_eq!(
        siar_blob_manifest::ids::BlobId::from_ciphertext(&ct),
        imported.blob_id
    );
    assert!(siar_blob_manifest::chunked_encryption::looks_chunked(&ct));
}

#[tokio::test]
async fn streaming_export_writes_a_v2_blob_to_disk_and_verifies_every_segment() {
    let n = net();
    let data = payload(1_200_000);
    let imported =
        n.a.svc
            .import_attachment_streaming(&data, MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    let dir = std::env::temp_dir().join(format!("siar-v2x-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("out.bin");
    let written =
        n.a.svc
            .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
            .await
            .unwrap();
    assert_eq!(written, data.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), data);

    // Wrong key: the very first segment fails, nothing is created.
    let mut forged = reference;
    forged.attachment_key[5] ^= 1;
    let dest2 = dir.join("forged.bin");
    assert!(n
        .a
        .svc
        .export_attachment(&forged, FsFileSink::create(&dest2).unwrap())
        .await
        .is_err());
    assert!(!dest2.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn corruption_in_a_later_segment_aborts_the_export_without_a_visible_file() {
    let n = net();
    let data = payload(900_000);
    let imported =
        n.a.svc
            .import_attachment_streaming(&data, MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    // Corrupt the stored blob near its end by rewriting it on disk.
    let id = imported.blob_id;
    let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let path = n.a._root.0.join("objects").join(&hex[..2]).join(&hex);
    let mut bytes = std::fs::read(&path).unwrap();
    let at = bytes.len() - 40;
    bytes[at] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let dir = std::env::temp_dir().join(format!("siar-v2c-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("never.bin");
    assert!(n
        .a
        .svc
        .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
        .await
        .is_err());
    assert!(
        !dest.exists(),
        "earlier verified segments never became visible"
    );
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "no temp file left"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn v1_and_v2_blobs_coexist_and_a_v2_blob_transfers_between_devices() {
    let n = net();
    let old = payload(10_000);
    let new = payload(10_001);
    let v1 =
        n.a.svc
            .import_attachment(&old, MediaType::Other)
            .await
            .unwrap();
    let v2 =
        n.a.svc
            .import_attachment_streaming(&new, MediaType::Other)
            .await
            .unwrap();
    assert_eq!(
        n.a.svc
            .open_attachment(&v1.reference().unwrap())
            .await
            .unwrap(),
        old
    );
    assert_eq!(
        n.a.svc
            .open_attachment(&v2.reference().unwrap())
            .await
            .unwrap(),
        new
    );

    // Send a v2 blob to the other device over files/1.
    let _h =
        n.a.svc
            .send_file_streaming(Destination::Device(n.b.id), &new, FileSendPolicy::default())
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    assert_eq!(
        offer.offer.blob.encryption.algorithm,
        siar_blob_manifest::descriptor::EncryptionAlgorithm::ChaCha20Poly1305Chunked
    );
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("v2-recv")).await.unwrap();
    eventually("v2 blob arrives", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    assert_eq!(
        n.a.svc.ciphertext(blob.as_bytes()),
        n.b.svc.ciphertext(blob.as_bytes())
    );
}

#[tokio::test]
async fn a_source_that_changes_between_the_two_passes_is_refused() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Flaky {
        data: Vec<u8>,
        reads: AtomicUsize,
    }
    impl siar_files::FileSource for Flaky {
        async fn len(&self) -> Result<u64, siar_files::FileError> {
            Ok(self.data.len() as u64)
        }
        async fn read_at(&self, o: u64, buf: &mut [u8]) -> Result<usize, siar_files::FileError> {
            let n = self.reads.fetch_add(1, Ordering::Relaxed);
            let r = self.data.read_at(o, buf).await?;
            // From the second pass on, silently corrupt a byte.
            if n >= 3 && r > 0 {
                buf[0] ^= 0xFF;
            }
            Ok(r)
        }
    }
    let n = net();
    let f = Flaky {
        data: payload(400_000),
        reads: AtomicUsize::new(0),
    };
    let err =
        n.a.svc
            .import_attachment_streaming(&f, MediaType::Other)
            .await;
    assert!(err.is_err(), "hash check at finalize catches the change");
    // The reference taken for the import was released.
    // (No blob is left referenced.)
}

// ---- §143-147, §158-173 wired through the real service ----

use siar_blob_manifest::app_policy::{
    AutoDownloadPolicy, NetworkKind, NotificationSink, TransferNotification,
};
use siar_blob_manifest::ids::TransferId;

#[derive(Default)]
struct Recorder(std::sync::Mutex<Vec<(TransferId, TransferNotification)>>);
impl NotificationSink for Recorder {
    fn notify(&self, t: TransferId, what: TransferNotification) {
        self.0.lock().unwrap().push((t, what));
    }
}

/// Like `node`, but the receiving side gets a notifier and policy.
fn policy_node(
    id: DeviceId,
    to_peer: mpsc::UnboundedSender<(DeviceId, u8, Vec<u8>)>,
    rec: Arc<Recorder>,
    auto: Option<AutoDownloadPolicy>,
) -> Node {
    let meter = Arc::new(Meter::default());
    let root = Root(std::env::temp_dir().join(format!("siar-files-{}", uuid::Uuid::new_v4())));
    let store = FsBlobStore::open(
        &root.0,
        FsStoreConfig {
            limits: StoreLimits::default(),
            durability: DurabilityPolicy::None,
            ..FsStoreConfig::default()
        },
        Meta::new(),
    )
    .unwrap();
    let refs = Arc::new(Refs::new());
    let ns = NamespaceId::new("personal").unwrap();
    let engine = Arc::new(TransferEngine::new(
        FileTransferService::new().with_event_log(Arc::new(InMemoryEventStore::new())),
        store,
        refs.clone(),
        ns.clone(),
    ));
    let mut cfg = FilesConfig::new(ns, EventOrigin::LocalDevice(id));
    cfg.notifier = Some(rec);
    if let Some(policy) = auto {
        cfg.auto_download = Some(siar_files::AutoDownload {
            policy,
            destination: Arc::new(|o| DestinationPolicy {
                object: ObjectId::new(&format!("auto-{}", o.transfer_id)).unwrap(),
                role: BlobRole::MessageAttachment,
                expires: TransferExpiry(u64::MAX),
            }),
        });
    }
    let svc = FilesService::new(
        engine,
        refs.clone(),
        Link {
            me: id,
            to_peer,
            drop_chunks: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            meter: meter.clone(),
        },
        cfg,
    );
    Node {
        meter,
        id,
        svc,
        refs,
        _root: root,
    }
}

fn policy_net(rec: Arc<Recorder>, auto: Option<AutoDownloadPolicy>) -> (Node, Node) {
    let (ida, idb) = (DeviceId::new(), DeviceId::new());
    let (tx_to_a, rx_a) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let (tx_to_b, rx_b) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let a = node(
        ida,
        tx_to_b,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let b = policy_node(idb, tx_to_a, rec, auto);
    for (svc, mut rx) in [(a.svc.clone(), rx_a), (b.svc.clone(), rx_b)] {
        tokio::spawn(async move {
            while let Some((from, kind, frame)) = rx.recv().await {
                let _ = svc.handle_frame(from, kind, &frame).await;
            }
        });
    }
    (a, b)
}

#[tokio::test]
async fn an_offer_notifies_action_required_and_a_completed_transfer_notifies_completed() {
    let rec = Arc::new(Recorder::default());
    let (a, b) = policy_net(rec.clone(), None);
    a.svc
        .send_file(
            Destination::Device(b.id),
            &payload(20_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = b.svc.next_offer().await.unwrap();
    // §144: the prompt carries only verified facts.
    let prompt = offer.prompt();
    assert_eq!(prompt.from, a.id);
    assert!(prompt.size_bytes > 20_000, "ciphertext size incl. overhead");
    let (blob, id) = (offer.offer.blob.blob_id, offer.offer.transfer_id);
    assert_eq!(
        rec.0.lock().unwrap().as_slice(),
        [(id, TransferNotification::ActionRequired)]
    );
    offer.accept(policy("n1")).await.unwrap();
    eventually("completed notification", || {
        rec.0
            .lock()
            .unwrap()
            .contains(&(id, TransferNotification::Completed))
    })
    .await;
    assert!(b.svc.ciphertext(blob.as_bytes()).is_some());
}

#[tokio::test]
async fn auto_download_accepts_images_on_wifi_and_leaves_the_rest_to_the_app() {
    let rec = Arc::new(Recorder::default());
    let (a, b) = policy_net(rec.clone(), Some(AutoDownloadPolicy::default()));
    // An image on an unmetered network: accepted with nobody asked.
    a.svc
        .send_file(
            Destination::Device(b.id),
            &payload(30_000),
            FileSendPolicy {
                media_type: MediaType::ImageJpeg,
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    eventually("auto-accepted image lands", || {
        rec.0
            .lock()
            .unwrap()
            .iter()
            .any(|(_, w)| *w == TransferNotification::Completed)
    })
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), b.svc.next_offer())
            .await
            .is_err(),
        "the app was never asked"
    );

    // The same image on a metered link: deferred to the application.
    b.svc.set_network_kind(NetworkKind::Metered);
    a.svc
        .send_file(
            Destination::Device(b.id),
            &payload(31_000),
            FileSendPolicy {
                media_type: MediaType::ImageJpeg,
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let queued = tokio::time::timeout(Duration::from_secs(5), b.svc.next_offer())
        .await
        .expect("metered image is queued for the app")
        .unwrap();
    queued.reject().await.unwrap();

    // A plain file is manual under the default policy even on Wi-Fi.
    b.svc.set_network_kind(NetworkKind::Unmetered);
    a.svc
        .send_file(
            Destination::Device(b.id),
            &payload(2_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), b.svc.next_offer())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn backup_plan_and_restore_work_against_the_real_store() {
    use siar_blob_manifest::app_policy::BlobStorageClass;
    use siar_blob_manifest::backup::{
        plan_backup, restore_entry, BackupCandidate, BackupPolicy, RestoreEntry,
    };
    use siar_blob_manifest::reference_store::BlobReference;
    let n = net();
    let imported =
        n.a.svc
            .import_attachment(&payload(40_000), MediaType::Other)
            .await
            .unwrap();
    let msg = ObjectId::new("msg-backup").unwrap();
    n.a.svc
        .bind_to_object(&imported, msg.clone(), BlobRole::MessageAttachment)
        .unwrap();

    let cand = BackupCandidate {
        blob_id: imported.blob_id,
        size: imported.reference().unwrap().encrypted_size.bytes(),
        class: BlobStorageClass::Durable,
        referenced: true,
        is_thumbnail: false,
    };
    let plan = plan_backup(&[cand], &BackupPolicy::default(), &HashSet::new());
    assert_eq!(plan.upload.len(), 1);
    assert!(plan_backup(
        &[cand],
        &BackupPolicy::default(),
        &HashSet::from([cand.blob_id])
    )
    .upload
    .is_empty());

    // "Restore" onto the second device: verify, then rebuild references.
    let bytes = n.a.svc.ciphertext(imported.blob_id.as_bytes()).unwrap();
    let entry = RestoreEntry {
        blob_id: imported.blob_id,
        size: bytes.len() as u64,
        references: vec![BlobReference {
            owner_namespace: NamespaceId::new("personal").unwrap(),
            object_id: msg,
            blob_id: imported.blob_id,
            role: BlobRole::MessageAttachment,
        }],
    };
    let mut tampered = bytes.clone();
    tampered[100] ^= 1;
    assert!(restore_entry(&entry, &tampered).is_err());
    let ops = restore_entry(&entry, &bytes).unwrap();
    n.b.refs.apply(&ops).unwrap();
    assert_eq!(n.b.refs.count_for(imported.blob_id).unwrap(), 1);
}

#[tokio::test]
async fn one_blob_key_is_shared_with_many_recipients_without_re_encrypting() {
    use siar_blob_manifest::key_wrapping::MultiRecipientBlob;
    let n = net();
    let data = payload(25_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    let key = siar_blob_manifest::ids::BlobEncryptionKey(reference.attachment_key);

    let mut shared = MultiRecipientBlob::new(imported.blob_id);
    let people: Vec<(DeviceId, [u8; 32])> = (1u8..=3).map(|i| (DeviceId::new(), [i; 32])).collect();
    for (d, s) in &people {
        shared.add_recipient(&key, s, *d).unwrap();
    }
    // Each recipient recovers the key and opens the SAME stored blob.
    for (d, s) in &people {
        let mut r = reference.clone();
        r.attachment_key = shared.open_for(*d, s).unwrap().0;
        assert_eq!(n.a.svc.open_attachment(&r).await.unwrap(), data);
    }
    // After revocation the wrap is gone (future access only).
    let rev = shared.revoke(people[0].0);
    assert!(rev.recipient_may_hold_key);
    assert!(shared.open_for(people[0].0, &people[0].1).is_err());
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_some());
}

// ---- §41-43 concurrency ----

use siar_blob_manifest::parallelism::{ResourceContext, Sample};
use siar_blob_manifest::scheduling::PathKind;

async fn run_one_transfer(n: &Net, len: usize) {
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(len),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy(&format!("c-{len}"))).await.unwrap();
    eventually("transfer completes", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
}

fn slow(n: &Net, ms: u64) {
    n.a.meter
        .delay_ms
        .store(ms, std::sync::atomic::Ordering::Relaxed);
}

fn max_overlap(n: &Net) -> usize {
    n.a.meter
        .max_inflight
        .load(std::sync::atomic::Ordering::Relaxed)
}

#[tokio::test]
async fn a_lan_peer_gets_several_chunks_in_flight_and_a_bluetooth_peer_exactly_one() {
    // Many small chunks: a 5 MB ciphertext at 1 MiB chunks is 5 chunks
    // per request, so overlap is observable.
    let lan = net();
    slow(&lan, 15);
    lan.a.svc.set_path_kind(lan.b.id, PathKind::Lan);
    // The cap is applied once capabilities are exchanged; a transfer does that.
    run_one_transfer(&lan, 4_000_000).await;
    lan.a.svc.set_path_kind(lan.b.id, PathKind::Lan);
    run_one_transfer(&lan, 4_100_000).await;
    assert!(
        max_overlap(&lan) >= 2,
        "LAN overlapped {} chunks",
        max_overlap(&lan)
    );

    let ble = net();
    slow(&ble, 15);
    run_one_transfer(&ble, 10_000).await; // exchanges capabilities
    ble.a.svc.set_path_kind(ble.b.id, PathKind::Ble);
    ble.a
        .meter
        .max_inflight
        .store(0, std::sync::atomic::Ordering::Relaxed);
    run_one_transfer(&ble, 4_200_000).await;
    assert_eq!(max_overlap(&ble), 1, "Bluetooth must never overlap chunks");
}

#[tokio::test]
async fn battery_saver_pulls_a_lan_peer_down_and_adaptive_mode_stays_inside_the_limit() {
    let n = net();
    slow(&n, 15);
    run_one_transfer(&n, 10_000).await;
    n.a.svc.set_path_kind(n.b.id, PathKind::Lan);
    n.a.svc.set_resources(ResourceContext {
        battery_saver: true,
        ..ResourceContext::default()
    });
    n.a.meter
        .max_inflight
        .store(0, std::sync::atomic::Ordering::Relaxed);
    run_one_transfer(&n, 4_300_000).await;
    // Battery saver on a LAN: 8 → 2 at most (and the default per-peer slot
    // cap of 3 also applies).
    assert!(max_overlap(&n) <= 2, "overlapped {}", max_overlap(&n));

    // Without adaptive mode a sample just reports the deterministic limit.
    let det = n.a.svc.report_sample(
        n.b.id,
        Sample {
            throughput_bytes_per_sec: 1_000_000,
            loss_permille: 0,
            rtt_millis: 10,
            memory_pressure: false,
            battery_saver: true,
        },
    );
    assert!((1..=2).contains(&det));
}

// ---- §195 large file ----

/// Resident-set high-water mark in KiB (Linux); `None` elsewhere.
fn peak_rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// A file that exists only as a formula: no buffer holds it.
struct Synthetic(u64);
impl siar_files::FileSource for Synthetic {
    async fn len(&self) -> Result<u64, siar_files::FileError> {
        Ok(self.0)
    }
    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, siar_files::FileError> {
        let n = (self.0.saturating_sub(offset) as usize).min(buf.len());
        for (i, b) in buf[..n].iter_mut().enumerate() {
            let p = offset + i as u64;
            *b = (p.wrapping_mul(2654435761) >> 13) as u8;
        }
        Ok(n)
    }
}

#[tokio::test]
async fn a_large_file_imports_and_exports_in_bounded_memory() {
    const SIZE: u64 = 32 * 1024 * 1024;
    let n = net();
    let before = peak_rss_kib();
    let imported =
        n.a.svc
            .import_attachment_streaming(&Synthetic(SIZE), MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();

    let dir = std::env::temp_dir().join(format!("siar-large-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("big.bin");
    let written =
        n.a.svc
            .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
            .await
            .unwrap();
    assert_eq!(written, SIZE);

    // Byte-exact against the formula, checked in pieces.
    use std::io::Read;
    let mut f = std::fs::File::open(&dest).unwrap();
    let mut buf = vec![0u8; 1 << 20];
    let mut off = 0u64;
    loop {
        let k = f.read(&mut buf).unwrap();
        if k == 0 {
            break;
        }
        for (i, b) in buf[..k].iter().enumerate() {
            let p = off + i as u64;
            assert_eq!(*b, (p.wrapping_mul(2654435761) >> 13) as u8, "byte {p}");
        }
        off += k as u64;
    }
    assert_eq!(off, SIZE);
    std::fs::remove_dir_all(dir).unwrap();

    if let (Some(b), Some(a)) = (before, peak_rss_kib()) {
        let grew_mib = a.saturating_sub(b) / 1024;
        // A whole-file path would need >= 3 x 32 MiB (plaintext,
        // ciphertext, a copy). Bounded streaming stays far below one copy.
        assert!(
            grew_mib < 40,
            "peak RSS grew {grew_mib} MiB for a 32 MiB file"
        );
    }
}

// ---- §190 hostile network, §193 service-level crash points -----------

/// Drives `retry_stalled` on a timer the way `spawn_maintenance` would,
/// but under the test's control.
fn spawn_retry(svc: Arc<Svc>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let _ = svc.retry_stalled(now).await;
        }
    })
}

fn degrade(n: &Net, loss: u64, dup: u64, jitter: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    n.a.meter.loss_permille.store(loss, Relaxed);
    n.a.meter.dup_permille.store(dup, Relaxed);
    n.a.meter.jitter_ms.store(jitter, Relaxed);
    n.a.meter.rng.store(0x1234_5678_9ABC_DEF1, Relaxed);
}

#[tokio::test]
async fn a_transfer_survives_loss_duplication_and_reordering_and_arrives_byte_exact() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    degrade(&n, 300, 200, 25); // 30% loss, 20% duplicates, up to 25 ms reordering
    let retry = spawn_retry(n.b.svc.clone());
    let data = payload(6_000_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    let chunk = u64::from(offer.offer.blob.chunking.chunk_size);
    let chunks = offer.offer.blob.size.bytes().div_ceil(chunk) as usize;
    offer.accept(policy("lossy")).await.unwrap();
    eventually("transfer completes despite a hostile network", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    retry.abort();
    // Byte-exact…
    assert_eq!(
        n.a.svc.ciphertext(blob.as_bytes()),
        n.b.svc.ciphertext(blob.as_bytes())
    );
    // …and the network really was hostile: frames were lost and
    // duplicated, so the sender had to send more than one copy of each.
    let (sent, lost, dups) = (
        n.a.meter.chunk_frames.load(Relaxed),
        n.a.meter.lost_frames.load(Relaxed),
        n.a.meter.duplicated_frames.load(Relaxed),
    );
    assert!(
        chunks >= 8,
        "need enough chunks for the test to mean something ({chunks})"
    );
    assert!(lost > 0 && dups > 0, "lost {lost}, duplicated {dups}");
    assert!(sent > chunks, "sent {sent} frames for {chunks} chunks");
    assert_eq!(n.b.refs.count_for(blob).unwrap(), 1);
}

#[tokio::test]
async fn a_lost_acknowledgement_leaves_the_sender_holding_the_blob_and_a_resend_costs_no_bytes() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    // The RECEIVER's completion ack never reaches the sender.
    n.b.meter.drop_complete.store(true, Relaxed);
    let data = payload(500_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::Other)
            .await
            .unwrap();
    n.a.svc
        .bind_to_object(
            &imported,
            ObjectId::new("msg-ack").unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    let blob = imported.blob_id;
    let _h =
        n.a.svc
            .send_imported(
                Destination::Device(n.b.id),
                &imported,
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    offer.accept(policy("ack-1")).await.unwrap();
    eventually("blob lands though the ack is lost", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The sender never saw the ack, so it still holds the transfer's
    // reference on top of the message's: it does not assume delivery.
    assert_eq!(n.a.refs.count_for(blob).unwrap(), 2);

    // The ack path recovers; the sender retries the offer. The receiver
    // already has the blob (dedup): no chunk crosses the wire again.
    n.b.meter.drop_complete.store(false, Relaxed);
    n.a.meter.chunk_frames.store(0, Relaxed);
    n.drop_a_chunks.store(true, Relaxed); // any chunk send would now fail loudly
    let _h2 =
        n.a.svc
            .send_imported(
                Destination::Device(n.b.id),
                &imported,
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer2 = n.b.svc.next_offer().await.unwrap();
    offer2.accept(policy("ack-2")).await.unwrap();
    eventually("the retried transfer completes and is acknowledged", || {
        // Second transfer's ack releases that send's reference.
        n.a.refs.count_for(blob).unwrap() <= 2
    })
    .await;
    assert_eq!(n.a.meter.chunk_frames.load(Relaxed), 0, "no bytes re-sent");
    assert_eq!(
        n.b.refs.count_for(blob).unwrap(),
        2,
        "one reference per accepted offer"
    );
}

#[tokio::test]
async fn a_receiver_that_dies_after_the_offer_leaves_nothing_behind_and_the_sender_recovers() {
    let n = net();
    let data = payload(80_000);
    let h =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &data,
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    // The receiver "crashes" holding the unanswered offer: nothing was
    // staged, nothing referenced on its side.
    drop(offer);
    assert!(n.b.svc.ciphertext(blob.as_bytes()).is_none());
    assert_eq!(n.b.refs.count_for(blob).unwrap(), 0);
    assert!(n.b.svc.engine().store().list_staging().unwrap().is_empty());
    // The sender, never answered, can give up cleanly and free its blob.
    h.cancel().await.unwrap();
    assert_eq!(n.a.refs.count_for(blob).unwrap(), 0);
}

#[tokio::test]
async fn a_lost_manifest_request_is_retried_and_the_transfer_completes() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    let retry = spawn_retry(n.b.svc.clone());
    // Everything the sender transmits after the offer is lost for a while.
    let data = payload(150_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    n.drop_a_chunks.store(true, Relaxed);
    offer.accept(policy("retry-manifest")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(n.b.svc.ciphertext(blob.as_bytes()).is_none());
    n.drop_a_chunks.store(false, Relaxed); // the link recovers
    eventually("stall retry finishes the transfer", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    retry.abort();
}

// ---- §197 Bluetooth scenario ------------------------------------------

#[tokio::test]
async fn a_bluetooth_transfer_is_cut_off_upgraded_to_wifi_and_resumes_only_what_is_missing() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    slow(&n, 10);
    run_one_transfer(&n, 5_000).await; // exchanges capabilities

    // Phase 1: the peer is on Bluetooth: one chunk at a time.
    n.a.svc.set_path_kind(n.b.id, PathKind::Ble);
    n.a.meter.max_inflight.store(0, Relaxed);
    n.a.meter.chunk_frames.store(0, Relaxed);
    let retry = spawn_retry(n.b.svc.clone());
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(4_400_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    let chunk = u64::from(offer.offer.blob.chunking.chunk_size);
    let chunks = offer.offer.blob.size.bytes().div_ceil(chunk) as usize;
    assert!(chunks >= 6, "need several chunks ({chunks})");
    offer.accept(policy("ble")).await.unwrap();
    eventually("a couple of chunks cross", || {
        n.a.meter.chunk_frames.load(Relaxed) >= 2
    })
    .await;

    // The Bluetooth link drops.
    n.drop_a_chunks.store(true, Relaxed);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        n.b.svc.ciphertext(blob.as_bytes()).is_none(),
        "cut off mid-transfer"
    );
    assert_eq!(max_overlap(&n), 1, "Bluetooth never overlaps chunks");
    let sent_before_cut = n.a.meter.chunk_frames.load(Relaxed);

    // Wi-Fi Direct comes up: same transfer, same blob, only the rest.
    n.a.svc.set_path_kind(n.b.id, PathKind::WifiDirect);
    n.drop_a_chunks.store(false, Relaxed);
    eventually("the transfer finishes over the new path", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    retry.abort();

    assert!(max_overlap(&n) >= 2, "the faster path overlapped chunks");
    let total = n.a.meter.chunk_frames.load(Relaxed);
    assert!(
        total <= chunks + 2,
        "{total} frames for {chunks} chunks: chunks received before the cut were not re-sent \
         (sent {sent_before_cut} before it)"
    );
    assert_eq!(
        n.a.svc.ciphertext(blob.as_bytes()),
        n.b.svc.ciphertext(blob.as_bytes())
    );
}

// ---- pull: fetching a blob by id over files/1 ------------------------

/// Two nodes; `hook_a`/`hook_b` adjust each side's configuration.
fn net_with(hook_a: impl FnOnce(&mut FilesConfig), hook_b: impl FnOnce(&mut FilesConfig)) -> Net {
    let (ida, idb) = (DeviceId::new(), DeviceId::new());
    let (tx_to_a, rx_a) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let (tx_to_b, rx_b) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let drop_a = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a = node_with(ida, tx_to_b, drop_a.clone(), hook_a);
    let b = node_with(
        idb,
        tx_to_a,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        hook_b,
    );
    for (svc, mut rx) in [(a.svc.clone(), rx_a), (b.svc.clone(), rx_b)] {
        tokio::spawn(async move {
            while let Some((from, kind, frame)) = rx.recv().await {
                let _ = svc.handle_frame(from, kind, &frame).await;
            }
        });
    }
    Net {
        a,
        b,
        drop_a_chunks: drop_a,
    }
}

async fn message_attachment(n: &Net, data: &Vec<u8>, v2: bool) -> siar_domain::AttachmentReference {
    let imported = if v2 {
        n.a.svc
            .import_attachment_streaming(data, MediaType::ImagePng)
            .await
            .unwrap()
    } else {
        n.a.svc
            .import_attachment(data, MediaType::ImagePng)
            .await
            .unwrap()
    };
    n.a.svc
        .bind_to_object(
            &imported,
            ObjectId::new(&format!("msg-{}", uuid::Uuid::new_v4())).unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    imported.reference().unwrap()
}

#[tokio::test]
async fn a_peer_pulls_an_attachment_by_its_reference_and_decrypts_it() {
    let n = net();
    let data = payload(1_800_000);
    let reference = message_attachment(&n, &data, false).await;
    let blob = siar_blob_manifest::ids::BlobId::from_reference_bytes(reference.blob_hash);

    let plaintext =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("pulled-1"))
            .await
            .unwrap();
    assert_eq!(plaintext, data);
    assert_eq!(n.b.refs.count_for(blob).unwrap(), 1);
    assert_eq!(
        n.a.svc.ciphertext(blob.as_bytes()),
        n.b.svc.ciphertext(blob.as_bytes())
    );
    // The holder's transfer reference is released once the pull is
    // acknowledged: only the message's own reference remains.
    eventually("holder releases its transfer reference", || {
        n.a.refs.count_for(blob).unwrap() == 1
    })
    .await;
}

#[tokio::test]
async fn a_v2_attachment_is_pulled_the_same_way() {
    let n = net();
    let data = payload(2_300_000);
    let reference = message_attachment(&n, &data, true).await;
    let plaintext =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("pulled-v2"))
            .await
            .unwrap();
    assert_eq!(plaintext, data);
}

#[tokio::test]
async fn pulling_a_blob_the_peer_does_not_have_fails_fast_with_unavailable() {
    let n = net();
    let missing = siar_blob_manifest::ids::BlobId::from_ciphertext(b"nobody has this");
    let started = std::time::Instant::now();
    let r =
        n.b.svc
            .fetch_blob(Destination::Device(n.a.id), missing, policy("nope"))
            .await;
    assert_eq!(r, Err(FilesError::BlobUnavailable));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "an answer, not a timeout"
    );
    assert_eq!(n.b.refs.count_for(missing).unwrap(), 0);
}

#[tokio::test]
async fn a_serve_gate_can_refuse_a_blob_the_device_does_hold_and_it_looks_like_unavailable() {
    struct Deny;
    impl siar_files::ServeGate for Deny {
        fn may_serve(&self, _: DeviceId, _: siar_blob_manifest::ids::BlobId) -> bool {
            false
        }
    }
    let n = net_with(|cfg| cfg.serve_gate = Some(Arc::new(Deny)), |_| {});
    let reference = message_attachment(&n, &payload(30_000), false).await;
    let r =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("denied"))
            .await;
    // Refusal is indistinguishable from "I don't have it".
    assert_eq!(r, Err(FilesError::BlobUnavailable));
}

#[tokio::test]
async fn the_default_gate_serves_only_blobs_that_are_still_referenced() {
    let n = net();
    let data = payload(40_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::Other)
            .await
            .unwrap();
    let reference = imported.reference().unwrap();
    // Held under the import's own reference: served.
    assert!(n
        .b
        .svc
        .fetch_attachment(Destination::Device(n.a.id), &reference, policy("held"))
        .await
        .is_ok());
    // Release every reference on A (the blob is now garbage awaiting GC).
    use siar_blob_manifest::reference_store::{BlobReference, RefOp};
    let _ = n.a.refs.apply(&[RefOp::Remove(BlobReference {
        owner_namespace: NamespaceId::new("personal").unwrap(),
        object_id: imported.import_owner.clone(),
        blob_id: imported.blob_id,
        role: BlobRole::Transfer,
    })]);
    eventually("send reference released", || {
        n.a.refs.count_for(imported.blob_id).unwrap() == 0
    })
    .await;
    // A third device asks A for the same (now unreferenced) blob.
    let c = net_with(|_| {}, |_| {});
    let _ = c; // separate pair: its A has nothing, as above
    n.b.svc
        .engine()
        .store()
        .delete_sync(imported.blob_id)
        .unwrap();
    let again =
        n.b.svc
            .fetch_blob(
                Destination::Device(n.a.id),
                imported.blob_id,
                policy("after-release"),
            )
            .await;
    assert_eq!(again, Err(FilesError::BlobUnavailable));
}

#[tokio::test]
async fn a_blob_already_stored_locally_is_not_fetched_again() {
    let n = net();
    let reference = message_attachment(&n, &payload(20_000), false).await;
    n.b.svc
        .fetch_attachment(Destination::Device(n.a.id), &reference, policy("first"))
        .await
        .unwrap();
    // Take the link down entirely: a second fetch must not need it.
    n.drop_a_chunks
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let again =
        n.b.svc
            .fetch_blob(
                Destination::Device(n.a.id),
                siar_blob_manifest::ids::BlobId::from_reference_bytes(reference.blob_hash),
                policy("second"),
            )
            .await;
    assert_eq!(again, Ok(()));
}

#[tokio::test]
async fn a_fetch_whose_chunks_never_arrive_times_out_instead_of_hanging() {
    let n = net_with(|_| {}, |cfg| cfg.fetch_timeout_millis = 400);
    let reference = message_attachment(&n, &payload(600_000), false).await;
    n.drop_a_chunks
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    let r =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("slow"))
            .await;
    assert_eq!(r, Err(FilesError::FetchTimeout));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn concurrent_fetches_of_the_same_blob_all_resolve_from_one_transfer() {
    let n = net();
    let data = payload(900_000);
    let reference = message_attachment(&n, &data, false).await;
    let blob = siar_blob_manifest::ids::BlobId::from_reference_bytes(reference.blob_hash);
    let (b1, b2) = (n.b.svc.clone(), n.b.svc.clone());
    let (r1, r2) = tokio::join!(
        b1.fetch_blob(Destination::Device(n.a.id), blob, policy("c1")),
        b2.fetch_blob(Destination::Device(n.a.id), blob, policy("c2")),
    );
    assert_eq!((r1, r2), (Ok(()), Ok(())));
    assert_eq!(n.b.svc.open_attachment(&reference).await.unwrap(), data);
    assert_eq!(
        n.b.refs.count_for(blob).unwrap(),
        1,
        "one transfer, one reference"
    );
}

#[tokio::test]
async fn a_redundant_second_offer_for_the_blob_being_fetched_does_not_sink_the_fetch() {
    let n = net();
    let data = payload(1_500_000);
    let imported =
        n.a.svc
            .import_attachment(&data, MediaType::ImagePng)
            .await
            .unwrap();
    n.a.svc
        .bind_to_object(
            &imported,
            ObjectId::new("msg-dup").unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    let reference = imported.reference().unwrap();
    let blob = imported.blob_id;

    let b = n.b.svc.clone();
    let from = n.a.id;
    let fetch = tokio::spawn(async move {
        b.fetch_blob(Destination::Device(from), blob, policy("main"))
            .await
    });
    // While B is pulling it, A also pushes the very same blob unasked: two
    // transfers for one blob now race on B.
    n.a.svc
        .send_imported(
            Destination::Device(n.b.id),
            &imported,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(20), fetch)
            .await
            .expect("the fetch resolved")
            .unwrap(),
        Ok(()),
        "a redundant transfer must not fail the fetch"
    );
    assert_eq!(n.b.svc.open_attachment(&reference).await.unwrap(), data);
    assert!(n.b.refs.count_for(blob).unwrap() >= 1);
}

// ---- the file-only flow actually delivers something openable ----------

#[tokio::test]
async fn a_file_sent_with_no_conversation_arrives_named_and_can_be_opened_by_the_receiver() {
    let n = net();
    let data = payload(700_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy {
                media_type: MediaType::ImagePng,
                file_name: Some("holiday photo.png".to_string()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    // The prompt can show the sender's name before anything is accepted.
    assert_eq!(offer.display_name(), Some("holiday photo.png"));
    offer.accept(policy("recv-1")).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(10), n.b.svc.next_received())
        .await
        .expect("a ReceivedFile is delivered when the transfer completes")
        .unwrap();
    assert_eq!(received.from, n.a.id);
    assert_eq!(received.display_name(), Some("holiday photo.png"));
    assert_eq!(received.export_name(), "holiday photo.png");
    assert_eq!(received.media_type, Some(MediaType::ImagePng));

    // The thing the receiving app could not do before: open it.
    let reference = received.reference().unwrap();
    assert_eq!(n.b.svc.open_attachment(&reference).await.unwrap(), data);

    // …and export it under the safe name, atomically.
    let dir = std::env::temp_dir().join(format!("siar-recv-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(received.export_name());
    n.b.svc
        .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_hostile_file_name_is_displayed_as_sent_but_exported_safely() {
    let n = net();
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(5_000),
            FileSendPolicy {
                file_name: Some("../../../etc/passwd".to_string()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    offer.accept(policy("evil")).await.unwrap();
    let received = n.b.svc.next_received().await.unwrap();
    assert_eq!(received.display_name(), Some("../../../etc/passwd"));
    let safe = received.export_name();
    assert!(!safe.contains('/') && !safe.contains('\\'), "{safe}");
    assert_ne!(safe, "..");
}

#[tokio::test]
async fn a_file_sent_without_a_name_still_arrives_openable() {
    let n = net();
    let data = payload(9_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    assert_eq!(offer.display_name(), None);
    offer.accept(policy("noname")).await.unwrap();
    let received = n.b.svc.next_received().await.unwrap();
    assert_eq!(received.export_name(), "unnamed_file");
    assert_eq!(
        n.b.svc
            .open_attachment(&received.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_name_over_255_bytes_is_refused_at_send_and_leaves_no_reference() {
    let n = net();
    let r =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(1_000),
                FileSendPolicy {
                    file_name: Some("n".repeat(300)),
                    ..FileSendPolicy::default()
                },
            )
            .await;
    assert_eq!(r.err(), Some(FilesError::FileNameTooLong));
}

#[tokio::test]
async fn auto_accepted_and_streamed_files_deliver_their_key_too() {
    let rec = Arc::new(Recorder::default());
    let (a, b) = policy_net(rec, Some(AutoDownloadPolicy::default()));
    let data = payload(2_000_000);
    a.svc
        .send_file_streaming(
            Destination::Device(b.id),
            &data,
            FileSendPolicy {
                media_type: MediaType::ImageJpeg,
                file_name: Some("auto.jpg".to_string()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    // Nobody called accept: the policy did.
    let received = tokio::time::timeout(Duration::from_secs(10), b.svc.next_received())
        .await
        .expect("auto-accepted file delivered")
        .unwrap();
    assert_eq!(received.display_name(), Some("auto.jpg"));
    assert_eq!(
        b.svc
            .open_attachment(&received.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_rejected_or_cancelled_transfer_delivers_no_received_file() {
    let n = net();
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(4_000),
            FileSendPolicy {
                file_name: Some("no.bin".to_string()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    n.b.svc.next_offer().await.unwrap().reject().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), n.b.svc.next_received())
            .await
            .is_err(),
        "no file, no key"
    );
}

// ---- sender-side offer resend, manifest cache ---------------------------

fn real_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test]
async fn an_offer_that_was_lost_is_sent_again_and_the_transfer_completes() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    n.a.meter.drop_offers.store(1, Relaxed); // the first offer vanishes
    let data = payload(120_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    assert!(
        // Shorter than the node's 100 ms stall window, so the offer is not
        // yet due when it is checked below.
        tokio::time::timeout(Duration::from_millis(30), n.b.svc.next_offer())
            .await
            .is_err(),
        "the receiver has seen nothing"
    );
    // Within the stall window nothing is re-sent; after it, the offer is.
    assert_eq!(
        n.a.svc.retry_unanswered_offers(real_now()).await.unwrap(),
        0
    );
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        n.a.svc.retry_unanswered_offers(real_now()).await.unwrap(),
        1
    );

    let offer = tokio::time::timeout(Duration::from_secs(5), n.b.svc.next_offer())
        .await
        .expect("the re-sent offer arrives")
        .unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("resent")).await.unwrap();
    eventually("completes", || {
        n.b.svc.ciphertext(blob.as_bytes()).is_some()
    })
    .await;
    // Answered now: nothing more to re-send.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        n.a.svc.retry_unanswered_offers(real_now()).await.unwrap(),
        0
    );
}

#[tokio::test]
async fn an_offer_nobody_ever_answers_is_given_up_and_its_blob_released() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    n.a.meter.drop_offers.store(1000, Relaxed);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(30_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    assert_eq!(n.a.refs.referenced_blob_ids(None, 8).unwrap().len(), 1);
    let mut t = real_now();
    let mut resent = 0;
    for _ in 0..3 {
        t += 1_000;
        resent += n.a.svc.retry_unanswered_offers(t).await.unwrap();
    }
    assert_eq!(resent, 3, "three resends");
    t += 1_000;
    assert_eq!(n.a.svc.retry_unanswered_offers(t).await.unwrap(), 0);
    assert!(
        n.a.refs.referenced_blob_ids(None, 8).unwrap().is_empty(),
        "given up: the blob is no longer pinned by the dead transfer"
    );
}

#[tokio::test]
async fn a_resent_offer_to_a_receiver_that_already_has_it_is_ignored() {
    let n = net();
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(40_000),
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    // B has the offer but its app has not answered yet; A resends.
    let first = n.b.svc.next_offer().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        n.a.svc.retry_unanswered_offers(real_now()).await.unwrap(),
        1
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), n.b.svc.next_offer())
            .await
            .is_err(),
        "the duplicate must not queue a second prompt"
    );
    first.accept(policy("once")).await.unwrap();
}

#[tokio::test]
async fn pulled_manifests_are_cached_and_a_stale_entry_is_never_served() {
    let n = net();
    let data = payload(300_000);
    let reference = message_attachment(&n, &data, false).await;
    let blob = siar_blob_manifest::ids::BlobId::from_reference_bytes(reference.blob_hash);
    assert_eq!(n.a.svc.cached_manifests(), 0);

    n.b.svc
        .fetch_attachment(Destination::Device(n.a.id), &reference, policy("p1"))
        .await
        .unwrap();
    assert_eq!(n.a.svc.cached_manifests(), 1, "the first pull cached it");

    // A second device-equivalent pull (B forgets the blob) is served from
    // the cache.
    n.b.svc.engine().store().delete_sync(blob).unwrap();
    n.b.svc
        .fetch_attachment(Destination::Device(n.a.id), &reference, policy("p2"))
        .await
        .unwrap();
    assert_eq!(n.a.svc.cached_manifests(), 1);

    // The holder loses the blob: the cache must not offer a ghost.
    n.a.svc.engine().store().delete_sync(blob).unwrap();
    n.b.svc.engine().store().delete_sync(blob).unwrap();
    let r =
        n.b.svc
            .fetch_blob(Destination::Device(n.a.id), blob, policy("p3"))
            .await;
    assert_eq!(r, Err(FilesError::BlobUnavailable));
    assert_eq!(n.a.svc.cached_manifests(), 0, "the stale entry was evicted");
}

// ---- the receiver can OPEN what it is sent (key + name carriage) -------

#[tokio::test]
async fn a_file_sent_device_to_device_can_be_opened_by_the_receiver_with_its_name() {
    let n = net();
    let data = payload(700_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy {
                media_type: MediaType::ImagePng,
                file_name: Some("quarterly report.png".into()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    assert_eq!(offer.display_name(), Some("quarterly report.png"));
    offer.accept(policy("received-1")).await.unwrap();

    let got = tokio::time::timeout(Duration::from_secs(10), n.b.svc.next_received())
        .await
        .expect("a received file is announced")
        .expect("the queue is open");
    assert_eq!(got.from, n.a.id);
    assert_eq!(got.display_name(), Some("quarterly report.png"));
    assert_eq!(got.media_type, Some(MediaType::ImagePng));
    // The thing that was impossible before: the receiver opens the file.
    let reference = got.reference().unwrap();
    assert_eq!(n.b.svc.open_attachment(&reference).await.unwrap(), data);
    // …and can save it under a safe name.
    let dir = std::env::temp_dir().join(format!("siar-recv-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join(got.export_name());
    n.b.svc
        .export_attachment(&reference, FsFileSink::create(&dest).unwrap())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_streamed_v2_file_is_openable_by_the_receiver_too() {
    let n = net();
    let data = payload(2_000_000);
    n.a.svc
        .send_file_streaming(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    n.b.svc
        .next_offer()
        .await
        .unwrap()
        .accept(policy("recv-v2"))
        .await
        .unwrap();
    let got = n.b.svc.next_received().await.unwrap();
    assert_eq!(got.display_name(), None);
    assert_eq!(got.export_name(), "unnamed_file");
    assert_eq!(
        n.b.svc
            .open_attachment(&got.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_hostile_file_name_cannot_reach_the_filesystem() {
    let n = net();
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(10_000),
            FileSendPolicy {
                file_name: Some("../../../etc/cron.d/evil".into()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    // Display keeps what the peer said; nothing path-like is exportable.
    assert_eq!(offer.display_name(), Some("../../../etc/cron.d/evil"));
    offer.accept(policy("hostile")).await.unwrap();
    let got = n.b.svc.next_received().await.unwrap();
    let safe = got.export_name();
    assert!(!safe.contains('/') && !safe.contains('\\'), "{safe}");
    assert_ne!(safe, "..");
}

#[tokio::test]
async fn an_overlong_file_name_is_refused_before_anything_is_sent() {
    let n = net();
    let r =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(1_000),
                FileSendPolicy {
                    file_name: Some("x".repeat(256)),
                    ..FileSendPolicy::default()
                },
            )
            .await;
    assert_eq!(r.err(), Some(FilesError::FileNameTooLong));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), n.b.svc.next_offer())
            .await
            .is_err(),
        "nothing was offered"
    );
}

#[tokio::test]
async fn an_auto_accepted_file_is_delivered_with_its_key_without_the_app_being_asked() {
    let rec = Arc::new(Recorder::default());
    let (a, b) = policy_net(rec, Some(AutoDownloadPolicy::default()));
    let data = payload(40_000);
    a.svc
        .send_file(
            Destination::Device(b.id),
            &data,
            FileSendPolicy {
                media_type: MediaType::ImageJpeg,
                file_name: Some("photo.jpg".into()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    let got = tokio::time::timeout(Duration::from_secs(10), b.svc.next_received())
        .await
        .expect("auto-accepted file is announced")
        .unwrap();
    assert_eq!(got.display_name(), Some("photo.jpg"));
    assert_eq!(
        b.svc
            .open_attachment(&got.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_pulled_attachment_announces_nothing_because_the_requester_already_has_the_key() {
    let n = net();
    let data = payload(50_000);
    let reference = message_attachment(&n, &data, false).await;
    n.b.svc
        .fetch_attachment(
            Destination::Device(n.a.id),
            &reference,
            policy("pulled-quiet"),
        )
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), n.b.svc.next_received())
            .await
            .is_err(),
        "a pull carries no key and produces no ReceivedFile"
    );
}

#[tokio::test]
async fn a_rejected_or_cancelled_offer_never_yields_a_received_file() {
    let n = net();
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(20_000),
            FileSendPolicy {
                file_name: Some("nope.bin".into()),
                ..FileSendPolicy::default()
            },
        )
        .await
        .unwrap();
    n.b.svc.next_offer().await.unwrap().reject().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), n.b.svc.next_received())
            .await
            .is_err()
    );
}

// ---- first-response deadline --------------------------------------------

#[tokio::test]
async fn a_peer_that_never_answers_is_given_up_on_quickly_not_after_the_full_timeout() {
    // B's frames go into a channel nobody reads: the peer is silent.
    let (tx, _rx_kept_alive_but_never_read) = mpsc::unbounded_channel();
    let b = node_with(
        DeviceId::new(),
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        |cfg| {
            cfg.fetch_first_response_millis = 200;
            cfg.fetch_timeout_millis = 30_000;
        },
    );
    let blob = siar_blob_manifest::ids::BlobId::from_ciphertext(b"anything");
    let started = std::time::Instant::now();
    let r = b
        .svc
        .fetch_blob(Destination::Device(DeviceId::new()), blob, policy("silent"))
        .await;
    assert_eq!(r, Err(FilesError::NoResponse));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "gave up after {:?}, not the 30 s overall timeout",
        started.elapsed()
    );
    // And nothing is left waiting.
    assert_eq!(b.refs.count_for(blob).unwrap(), 0);
}

#[tokio::test]
async fn a_peer_that_answered_is_not_cut_off_by_the_first_response_deadline() {
    // A offers promptly, but its chunks never come: the deadline that
    // matters is the overall one, and the error says so.
    let n = net_with(
        |_| {},
        |cfg| {
            cfg.fetch_first_response_millis = 150;
            cfg.fetch_timeout_millis = 900;
        },
    );
    let reference = message_attachment(&n, &payload(400_000), false).await;
    n.drop_a_chunks
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let started = std::time::Instant::now();
    let r =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("slow-peer"))
            .await;
    assert_eq!(
        r,
        Err(FilesError::FetchTimeout),
        "it responded, so this is slowness"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(800),
        "waited out the overall timeout"
    );
}

#[tokio::test]
async fn an_unavailable_answer_is_not_mistaken_for_silence() {
    let n = net_with(
        |_| {},
        |cfg| {
            cfg.fetch_first_response_millis = 5_000;
        },
    );
    let missing = siar_blob_manifest::ids::BlobId::from_ciphertext(b"not here");
    let r =
        n.b.svc
            .fetch_blob(Destination::Device(n.a.id), missing, policy("absent"))
            .await;
    assert_eq!(r, Err(FilesError::BlobUnavailable));
}

// ---- multi-recipient wrapped-key delivery -----------------------------

struct PairSecrets(std::collections::HashMap<DeviceId, [u8; 32]>);
impl siar_files::PairwiseSecrets for PairSecrets {
    fn secret_for(&self, peer: DeviceId) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        self.0.get(&peer).map(|s| zeroize::Zeroizing::new(*s))
    }
}

fn secrets(pairs: &[(DeviceId, [u8; 32])]) -> Arc<dyn siar_files::PairwiseSecrets> {
    Arc::new(PairSecrets(pairs.iter().copied().collect()))
}

/// Two nodes with fixed ids, each configured with the given pairwise
/// secrets (`None` = this side has no secret for the other).
fn net_secrets(a_has: Option<[u8; 32]>, b_has: Option<[u8; 32]>) -> Net {
    let (ida, idb) = (DeviceId::new(), DeviceId::new());
    let (tx_to_a, rx_a) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let (tx_to_b, rx_b) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
    let drop_a = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let a = node_with(ida, tx_to_b, drop_a.clone(), |cfg| {
        if let Some(s) = a_has {
            cfg.pairwise_secrets = Some(secrets(&[(idb, s)]));
        }
    });
    let b = node_with(
        idb,
        tx_to_a,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        |cfg| {
            if let Some(s) = b_has {
                cfg.pairwise_secrets = Some(secrets(&[(ida, s)]));
            }
        },
    );
    for (svc, mut rx) in [(a.svc.clone(), rx_a), (b.svc.clone(), rx_b)] {
        tokio::spawn(async move {
            while let Some((from, kind, frame)) = rx.recv().await {
                let _ = svc.handle_frame(from, kind, &frame).await;
            }
        });
    }
    Net {
        a,
        b,
        drop_a_chunks: drop_a,
    }
}

fn wrapped(name: Option<&str>) -> FileSendPolicy {
    FileSendPolicy {
        key_delivery: siar_files::KeyDelivery::Wrapped,
        file_name: name.map(str::to_owned),
        ..FileSendPolicy::default()
    }
}

#[tokio::test]
async fn a_wrapped_key_lets_the_intended_recipient_open_the_file() {
    let s = [11u8; 32];
    let n = net_secrets(Some(s), Some(s));
    let data = payload(300_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            wrapped(Some("wrapped.bin")),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    assert_eq!(offer.key_status(), siar_files::KeyStatus::Present);
    assert_eq!(offer.display_name(), Some("wrapped.bin"));
    offer.accept(policy("wrapped-1")).await.unwrap();
    let got = n.b.svc.next_received().await.unwrap();
    assert_eq!(
        n.b.svc
            .open_attachment(&got.reference().unwrap())
            .await
            .unwrap(),
        data
    );
}

#[tokio::test]
async fn a_recipient_without_the_right_secret_gets_the_bytes_but_cannot_open_them() {
    for b_secret in [None, Some([99u8; 32])] {
        let n = net_secrets(Some([11u8; 32]), b_secret);
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(50_000),
                wrapped(Some("x.bin")),
            )
            .await
            .unwrap();
        let offer = n.b.svc.next_offer().await.unwrap();
        assert_eq!(offer.key_status(), siar_files::KeyStatus::WrapUnopenable);
        assert_eq!(offer.display_name(), Some("x.bin"), "the name still shows");
        let blob = offer.offer.blob.blob_id;
        offer.accept(policy("no-key")).await.unwrap();
        eventually("ciphertext still arrives and verifies", || {
            n.b.svc.ciphertext(blob.as_bytes()).is_some()
        })
        .await;
        // But no key was ever delivered, so no ReceivedFile is announced.
        assert!(
            tokio::time::timeout(Duration::from_millis(250), n.b.svc.next_received())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn wrapping_without_a_secret_for_the_recipient_fails_before_anything_is_sent() {
    // The sender has no secret configured at all, then one for someone else.
    let n = net_secrets(None, Some([1u8; 32]));
    let r =
        n.a.svc
            .send_file(Destination::Device(n.b.id), &payload(1_000), wrapped(None))
            .await;
    assert_eq!(r.err(), Some(FilesError::NoPairwiseSecret(n.b.id)));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), n.b.svc.next_offer())
            .await
            .is_err(),
        "nothing was offered"
    );
}

/// Three devices; A can send to B and C, with a different pairwise
/// secret for each.
fn star3(a_knows_c: bool) -> (Node, Node, Node) {
    let (ida, idb, idc) = (DeviceId::new(), DeviceId::new(), DeviceId::new());
    let mk = |id: DeviceId, cfg_secrets: Vec<(DeviceId, [u8; 32])>| {
        let (tx, rx) = mpsc::unbounded_channel::<(DeviceId, u8, Vec<u8>)>();
        let (dummy, _) = mpsc::unbounded_channel();
        let node = node_with(
            id,
            dummy,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            move |cfg| cfg.pairwise_secrets = Some(secrets(&cfg_secrets)),
        );
        (node, tx, rx)
    };
    let mut sa = vec![(idb, [0xB; 32])];
    if a_knows_c {
        sa.push((idc, [0xC; 32]));
    }
    let (a, tx_a, rx_a) = mk(ida, sa);
    let (b, tx_b, rx_b) = mk(idb, vec![(ida, [0xB; 32])]);
    let (c, tx_c, rx_c) = mk(idc, vec![(ida, [0xC; 32])]);
    for node in [&a, &b, &c] {
        let mut routes = node.meter.routes.lock().unwrap();
        for (id, tx) in [(ida, &tx_a), (idb, &tx_b), (idc, &tx_c)] {
            if id != node.id {
                routes.insert(id, tx.clone());
            }
        }
    }
    for (svc, mut rx) in [
        (a.svc.clone(), rx_a),
        (b.svc.clone(), rx_b),
        (c.svc.clone(), rx_c),
    ] {
        tokio::spawn(async move {
            while let Some((from, kind, frame)) = rx.recv().await {
                let _ = svc.handle_frame(from, kind, &frame).await;
            }
        });
    }
    (a, b, c)
}

#[tokio::test]
async fn one_blob_is_stored_once_and_opened_by_each_recipient_with_their_own_wrapped_key() {
    let (a, b, c) = star3(true);
    let data = payload(1_200_000);
    let results = a
        .svc
        .send_file_to_many(
            &[Destination::Device(b.id), Destination::Device(c.id)],
            &data,
            wrapped(Some("shared.bin")),
        )
        .await
        .unwrap();
    assert!(results.iter().all(|(_, r)| r.is_ok()));

    let (offer_b, offer_c) = (
        b.svc.next_offer().await.unwrap(),
        c.svc.next_offer().await.unwrap(),
    );
    // ONE encrypted blob: both offers name the same BlobId.
    assert_eq!(offer_b.offer.blob.blob_id, offer_c.offer.blob.blob_id);
    let blob = offer_b.offer.blob.blob_id;
    assert_eq!(offer_b.key_status(), siar_files::KeyStatus::Present);
    assert_eq!(offer_c.key_status(), siar_files::KeyStatus::Present);
    offer_b.accept(policy("to-b")).await.unwrap();
    offer_c.accept(policy("to-c")).await.unwrap();

    for node in [&b, &c] {
        let got = tokio::time::timeout(Duration::from_secs(15), node.svc.next_received())
            .await
            .expect("each recipient receives it")
            .unwrap();
        assert_eq!(got.display_name(), Some("shared.bin"));
        assert_eq!(
            node.svc
                .open_attachment(&got.reference().unwrap())
                .await
                .unwrap(),
            data
        );
    }
    // The sender kept one copy, and after both acknowledged nothing but
    // GC holds on to it.
    eventually("the sender's transfer references are released", || {
        a.refs.count_for(blob).unwrap() == 0
    })
    .await;
    assert!(a.svc.ciphertext(blob.as_bytes()).is_some());
}

#[tokio::test]
async fn sending_to_several_requires_wrapped_keys() {
    let (a, b, c) = star3(true);
    let r = a
        .svc
        .send_file_to_many(
            &[Destination::Device(b.id), Destination::Device(c.id)],
            &payload(1_000),
            FileSendPolicy::default(),
        )
        .await;
    assert_eq!(r.err(), Some(FilesError::KeyMustBeWrapped));
}

#[tokio::test]
async fn one_recipient_failing_does_not_stop_the_others() {
    // A has no secret for C.
    let (a, b, c) = star3(false);
    let results = a
        .svc
        .send_file_to_many(
            &[Destination::Device(c.id), Destination::Device(b.id)],
            &payload(60_000),
            wrapped(None),
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0].1.as_ref().err(),
        Some(FilesError::NoPairwiseSecret(d)) if *d == c.id
    ));
    assert!(results[1].1.is_ok());
    let offer = b.svc.next_offer().await.unwrap();
    assert_eq!(offer.key_status(), siar_files::KeyStatus::Present);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), c.svc.next_offer())
            .await
            .is_err(),
        "C was never offered anything"
    );
}

#[tokio::test]
async fn when_every_recipient_fails_the_blob_is_not_left_pinned() {
    let (a, b, _c) = star3(false);
    let nobody = DeviceId::new();
    let results = a
        .svc
        .send_file_to_many(
            &[Destination::Device(nobody), Destination::Device(b.id)],
            &payload(10_000),
            wrapped(None),
        )
        .await
        .unwrap();
    // `nobody` has no secret, and B is offered; reject B's offer too.
    assert!(results[0].1.is_err());
    b.svc.next_offer().await.unwrap().reject().await.unwrap();
    eventually("nothing references the blob any more", || {
        a.refs.referenced_blob_ids(None, 8).unwrap().is_empty()
    })
    .await;
}

// ---- streaming multi-recipient --------------------------------------------

#[tokio::test]
async fn a_streamed_send_to_several_never_reads_the_file_whole_and_every_recipient_opens_it() {
    let (a, b, c) = star3(true);
    let data = payload(2_800_000);
    let probe = Probe {
        data: data.clone(),
        max_read: Default::default(),
    };
    let results = a
        .svc
        .send_file_to_many_streaming(
            &[Destination::Device(b.id), Destination::Device(c.id)],
            &probe,
            wrapped(Some("big-shared.bin")),
        )
        .await
        .unwrap();
    assert!(results.iter().all(|(_, r)| r.is_ok()));
    let largest = probe.max_read.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        largest <= 256 * 1024,
        "the largest single read was {largest} bytes: the file was not streamed"
    );

    let (offer_b, offer_c) = (
        b.svc.next_offer().await.unwrap(),
        c.svc.next_offer().await.unwrap(),
    );
    // One v2 blob for everyone.
    assert_eq!(offer_b.offer.blob.blob_id, offer_c.offer.blob.blob_id);
    for o in [&offer_b, &offer_c] {
        assert_eq!(
            o.offer.blob.encryption.algorithm,
            siar_blob_manifest::descriptor::EncryptionAlgorithm::ChaCha20Poly1305Chunked
        );
        assert_eq!(o.key_status(), siar_files::KeyStatus::Present);
    }
    let blob = offer_b.offer.blob.blob_id;
    offer_b.accept(policy("s-b")).await.unwrap();
    offer_c.accept(policy("s-c")).await.unwrap();
    for node in [&b, &c] {
        let got = tokio::time::timeout(Duration::from_secs(20), node.svc.next_received())
            .await
            .expect("each recipient receives it")
            .unwrap();
        assert_eq!(
            node.svc
                .open_attachment(&got.reference().unwrap())
                .await
                .unwrap(),
            data
        );
    }
    eventually("the sender's transfer references are released", || {
        a.refs.count_for(blob).unwrap() == 0
    })
    .await;
}

#[tokio::test]
async fn the_streaming_fan_out_also_requires_wrapped_keys() {
    let (a, b, c) = star3(true);
    let r = a
        .svc
        .send_file_to_many_streaming(
            &[Destination::Device(b.id), Destination::Device(c.id)],
            &payload(1_000),
            FileSendPolicy::default(),
        )
        .await;
    assert_eq!(r.err(), Some(FilesError::KeyMustBeWrapped));
}

#[tokio::test]
async fn in_the_streaming_fan_out_one_recipient_failing_does_not_stop_the_others() {
    let (a, b, c) = star3(false); // A has no secret for C
    let results = a
        .svc
        .send_file_to_many_streaming(
            &[Destination::Device(c.id), Destination::Device(b.id)],
            &payload(300_000),
            wrapped(None),
        )
        .await
        .unwrap();
    assert!(matches!(
        results[0].1.as_ref().err(),
        Some(FilesError::NoPairwiseSecret(d)) if *d == c.id
    ));
    assert!(results[1].1.is_ok());
    let offer = b.svc.next_offer().await.unwrap();
    offer.accept(policy("only-b")).await.unwrap();
    assert!(b.svc.next_received().await.is_some());
}

#[tokio::test]
async fn a_streamed_fan_out_with_no_recipients_leaves_nothing_pinned() {
    let (a, _b, _c) = star3(true);
    let out = a
        .svc
        .send_file_to_many_streaming(&[], &payload(20_000), wrapped(None))
        .await
        .unwrap();
    assert!(out.is_empty());
    assert!(
        a.refs.referenced_blob_ids(None, 8).unwrap().is_empty(),
        "an import with nobody to send to must not stay referenced"
    );
}

// ---- revocation ----------------------------------------------------------

use siar_files::RevocationEffect;

async fn bound_blob(
    n: &Net,
    data: &Vec<u8>,
    name: &str,
) -> (
    siar_files::ImportedAttachment,
    siar_domain::AttachmentReference,
) {
    let imported =
        n.a.svc
            .import_attachment(data, MediaType::Other)
            .await
            .unwrap();
    n.a.svc
        .bind_to_object(
            &imported,
            ObjectId::new(name).unwrap(),
            BlobRole::MessageAttachment,
        )
        .unwrap();
    let reference = imported.reference().unwrap();
    (imported, reference)
}

#[tokio::test]
async fn revoking_mid_transfer_cancels_it_and_the_recipient_never_gets_the_file() {
    use std::sync::atomic::Ordering::Relaxed;
    let s = [5u8; 32];
    let n = net_secrets(Some(s), Some(s));
    n.drop_a_chunks.store(true, Relaxed); // the transfer stalls in flight
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &payload(800_000),
            wrapped(None),
        )
        .await
        .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let blob = offer.offer.blob.blob_id;
    offer.accept(policy("revoked-mid")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let rev =
        n.a.svc
            .revoke_access(blob, Destination::Device(n.b.id))
            .await
            .unwrap();
    assert_eq!(rev.effect, RevocationEffect::StopsFutureAccessOnly);
    assert!(rev.recipient_may_hold_key, "a key had been sent");

    // The link recovers: nothing may arrive any more.
    n.drop_a_chunks.store(false, Relaxed);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(n.b.svc.ciphertext(blob.as_bytes()).is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), n.b.svc.next_received())
            .await
            .is_err()
    );
    assert_eq!(
        n.a.refs.count_for(blob).unwrap(),
        0,
        "the cancelled transfer released its reference"
    );
}

#[tokio::test]
async fn after_revocation_the_device_cannot_pull_the_blob_and_the_refusal_looks_like_absence() {
    let n = net();
    let (imported, reference) = bound_blob(&n, &payload(60_000), "msg-rev-pull").await;
    let rev =
        n.a.svc
            .revoke_access(imported.blob_id, Destination::Device(n.b.id))
            .await
            .unwrap();
    assert!(!rev.recipient_may_hold_key, "no key was ever sent to B");
    let r =
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("blocked"))
            .await;
    assert_eq!(r, Err(FilesError::BlobUnavailable));
}

#[tokio::test]
async fn after_revocation_no_key_is_sent_again_until_access_is_restored() {
    let s = [6u8; 32];
    let n = net_secrets(Some(s), Some(s));
    let (imported, reference) = bound_blob(&n, &payload(40_000), "msg-rev-resend").await;
    n.a.svc
        .revoke_access(imported.blob_id, Destination::Device(n.b.id))
        .await
        .unwrap();
    let blocked =
        n.a.svc
            .send_imported(Destination::Device(n.b.id), &imported, wrapped(None))
            .await;
    assert_eq!(blocked.err(), Some(FilesError::AccessRevoked));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), n.b.svc.next_offer())
            .await
            .is_err(),
        "nothing was offered"
    );

    // The owner changes their mind.
    n.a.svc
        .restore_access(imported.blob_id, Destination::Device(n.b.id));
    n.a.svc
        .send_imported(Destination::Device(n.b.id), &imported, wrapped(None))
        .await
        .unwrap();
    n.b.svc
        .next_offer()
        .await
        .unwrap()
        .accept(policy("restored"))
        .await
        .unwrap();
    assert!(n.b.svc.next_received().await.is_some());
    let _ = reference;
}

#[tokio::test]
async fn revoking_a_recipient_who_already_has_the_file_cannot_take_it_back_and_says_so() {
    let s = [7u8; 32];
    let n = net_secrets(Some(s), Some(s));
    let data = payload(100_000);
    n.a.svc
        .send_file(Destination::Device(n.b.id), &data, wrapped(None))
        .await
        .unwrap();
    n.b.svc
        .next_offer()
        .await
        .unwrap()
        .accept(policy("already"))
        .await
        .unwrap();
    let got = n.b.svc.next_received().await.unwrap();
    let reference = got.reference().unwrap();
    let blob = got.blob_id;

    let rev =
        n.a.svc
            .revoke_access(blob, Destination::Device(n.b.id))
            .await
            .unwrap();
    assert_eq!(rev.effect, RevocationEffect::StopsFutureAccessOnly);
    assert!(rev.recipient_may_hold_key);
    // The honest limit, demonstrated: B still opens what it already has.
    assert_eq!(n.b.svc.open_attachment(&reference).await.unwrap(), data);
}

#[tokio::test]
async fn revoking_one_of_two_recipients_leaves_the_other_untouched() {
    let (a, b, c) = star3(true);
    let results = a
        .svc
        .send_file_to_many(
            &[Destination::Device(b.id), Destination::Device(c.id)],
            &payload(500_000),
            wrapped(None),
        )
        .await
        .unwrap();
    assert!(results.iter().all(|(_, r)| r.is_ok()));
    let (offer_b, offer_c) = (
        b.svc.next_offer().await.unwrap(),
        c.svc.next_offer().await.unwrap(),
    );
    let blob = offer_b.offer.blob.blob_id;

    // B is revoked before it answers; C is not.
    a.svc
        .revoke_access(blob, Destination::Device(b.id))
        .await
        .unwrap();
    offer_c.accept(policy("c-keeps")).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(15), c.svc.next_received())
        .await
        .expect("C still receives it")
        .unwrap();
    assert!(c
        .svc
        .open_attachment(&got.reference().unwrap())
        .await
        .is_ok());

    // B's late acceptance gets nothing.
    let _ = offer_b.accept(policy("b-late")).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(b.svc.ciphertext(blob.as_bytes()).is_none());
}

#[tokio::test]
async fn the_streaming_and_in_memory_fan_outs_both_skip_a_revoked_recipient() {
    let (a, b, c) = star3(true);
    let (imported, _r) = {
        let imp = a
            .svc
            .import_attachment(&payload(30_000), MediaType::Other)
            .await
            .unwrap();
        a.svc
            .bind_to_object(
                &imp,
                ObjectId::new("msg-fan").unwrap(),
                BlobRole::MessageAttachment,
            )
            .unwrap();
        let r = imp.reference().unwrap();
        (imp, r)
    };
    a.svc
        .revoke_access(imported.blob_id, Destination::Device(c.id))
        .await
        .unwrap();
    // send_imported to C is refused; to B it works.
    assert_eq!(
        a.svc
            .send_imported(Destination::Device(c.id), &imported, wrapped(None))
            .await
            .err(),
        Some(FilesError::AccessRevoked)
    );
    assert!(a
        .svc
        .send_imported(Destination::Device(b.id), &imported, wrapped(None))
        .await
        .is_ok());
}

struct RecordingLedger {
    log: std::sync::Mutex<Vec<String>>,
    revoked:
        std::sync::Mutex<std::collections::HashSet<(siar_blob_manifest::ids::BlobId, DeviceId)>>,
}
impl siar_files::AccessLedger for RecordingLedger {
    fn record_grant(&self, _b: siar_blob_manifest::ids::BlobId, _d: DeviceId) {
        self.log.lock().unwrap().push("grant".into());
    }
    fn revoke(&self, b: siar_blob_manifest::ids::BlobId, d: DeviceId) -> bool {
        self.log.lock().unwrap().push("revoke".into());
        self.revoked.lock().unwrap().insert((b, d));
        true
    }
    fn restore(&self, b: siar_blob_manifest::ids::BlobId, d: DeviceId) {
        self.log.lock().unwrap().push("restore".into());
        self.revoked.lock().unwrap().remove(&(b, d));
    }
    fn is_revoked(&self, b: siar_blob_manifest::ids::BlobId, d: DeviceId) -> bool {
        self.revoked.lock().unwrap().contains(&(b, d))
    }
}

#[tokio::test]
async fn a_host_can_plug_in_a_durable_ledger_and_the_service_uses_it() {
    let ledger = Arc::new(RecordingLedger {
        log: Default::default(),
        revoked: Default::default(),
    });
    let l = ledger.clone();
    let n = net_with(move |cfg| cfg.access_ledger = Some(l), |_| {});
    let (imported, reference) = bound_blob(&n, &payload(20_000), "msg-ledger").await;
    // A pull does not grant a key; a revoke is recorded; a refused pull
    // consults the ledger.
    n.a.svc
        .revoke_access(imported.blob_id, Destination::Device(n.b.id))
        .await
        .unwrap();
    assert_eq!(
        n.b.svc
            .fetch_attachment(Destination::Device(n.a.id), &reference, policy("l"))
            .await,
        Err(FilesError::BlobUnavailable)
    );
    n.a.svc
        .restore_access(imported.blob_id, Destination::Device(n.b.id));
    assert_eq!(&*ledger.log.lock().unwrap(), &["revoke", "restore"]);
}

// ---- manifest cache: pruning after GC -------------------------------------

#[tokio::test]
async fn gc_clears_cached_manifests_of_blobs_it_reclaimed() {
    use siar_blob_manifest::reference_store::{BlobReference, RefOp};
    let n = net_with(|cfg| cfg.gc_grace_millis = 0, |_| {});
    let data = payload(70_000);
    let (imported, reference) = bound_blob(&n, &data, "msg-cache").await;
    // A pull makes A rebuild and cache the manifest.
    n.b.svc
        .fetch_attachment(
            Destination::Device(n.a.id),
            &reference,
            policy("cache-pull"),
        )
        .await
        .unwrap();
    assert_eq!(n.a.svc.cached_manifests(), 1);
    eventually("the pull's reference is released", || {
        n.a.refs.count_for(imported.blob_id).unwrap() == 1
    })
    .await;

    // The message is deleted; GC reclaims the blob in two passes.
    n.a.refs
        .apply(&[RefOp::Remove(BlobReference {
            owner_namespace: NamespaceId::new("personal").unwrap(),
            object_id: ObjectId::new("msg-cache").unwrap(),
            blob_id: imported.blob_id,
            role: BlobRole::MessageAttachment,
        })])
        .unwrap();
    n.a.svc.maintenance(1_000).unwrap();
    n.a.svc.maintenance(1_000 + 11 * 60 * 1000).unwrap();
    assert!(n.a.svc.ciphertext(imported.blob_id.as_bytes()).is_none());
    assert_eq!(
        n.a.svc.cached_manifests(),
        0,
        "the reclaimed blob's manifest must not linger in memory"
    );
}

// ---- regression: cancelling an ACCEPTED transfer must not wedge the receiver ----
//
// A guard taken in an `if let` scrutinee lived across a call that locked
// the same mutex, so the receiver's inbound handler deadlocked on itself
// the moment a sender cancelled a transfer that had already been
// accepted. Earlier cancel tests cancelled before accepting (or ended
// before the receiver processed the cancel), so none noticed.

#[tokio::test]
async fn cancelling_an_accepted_transfer_leaves_the_receiver_fully_working() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net();
    n.drop_a_chunks.store(true, Relaxed); // keeps the first transfer in flight
    let h =
        n.a.svc
            .send_file(
                Destination::Device(n.b.id),
                &payload(600_000),
                FileSendPolicy::default(),
            )
            .await
            .unwrap();
    let offer = n.b.svc.next_offer().await.unwrap();
    let cancelled_blob = offer.offer.blob.blob_id;
    offer.accept(policy("to-be-cancelled")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    h.cancel().await.unwrap();
    // Give the receiver time to process the cancel — this is where it hung.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The receiver is alive: a brand-new transfer goes straight through.
    n.drop_a_chunks.store(false, Relaxed);
    let data = payload(90_000);
    n.a.svc
        .send_file(
            Destination::Device(n.b.id),
            &data,
            FileSendPolicy::default(),
        )
        .await
        .unwrap();
    let offer2 = tokio::time::timeout(Duration::from_secs(10), n.b.svc.next_offer())
        .await
        .expect("the receiver still processes frames after a cancel")
        .unwrap();
    let blob2 = offer2.offer.blob.blob_id;
    offer2.accept(policy("after-cancel")).await.unwrap();
    eventually("the next transfer completes", || {
        n.b.svc.ciphertext(blob2.as_bytes()).is_some()
    })
    .await;
    assert!(
        n.b.svc.ciphertext(cancelled_blob.as_bytes()).is_none(),
        "and the cancelled one never lands"
    );
}

#[tokio::test]
async fn a_holder_cancelling_a_pull_midway_fails_the_waiting_fetch_promptly() {
    use std::sync::atomic::Ordering::Relaxed;
    let n = net_with(|_| {}, |cfg| cfg.fetch_timeout_millis = 30_000);
    let (imported, reference) = bound_blob(&n, &payload(700_000), "msg-cancelled-pull").await;
    n.drop_a_chunks.store(true, Relaxed); // the pull stalls after the offer
    let b = n.b.svc.clone();
    let from = n.a.id;
    let fetch = tokio::spawn(async move {
        b.fetch_attachment(
            Destination::Device(from),
            &reference,
            policy("pull-cancelled"),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(250)).await;

    // The holder pulls the plug (revoking this device's access cancels the
    // transfer it was serving).
    n.a.svc
        .revoke_access(imported.blob_id, Destination::Device(n.b.id))
        .await
        .unwrap();

    let started = std::time::Instant::now();
    let r = tokio::time::timeout(Duration::from_secs(10), fetch)
        .await
        .expect("the fetch must resolve, not wait out its 30 s timeout")
        .unwrap();
    assert_eq!(r, Err(FilesError::FetchFailed));
    assert!(started.elapsed() < Duration::from_secs(5));
}
