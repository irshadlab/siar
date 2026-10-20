//! Spec 05 §193 crash injection against the DURABLE stores: blob
//! metadata, the reference model and the event log all live in `stoolap`
//! files, and a "crash" drops every handle and reopens everything from
//! disk. `crash_injection.rs` approximates this with in-memory stores;
//! this is the real thing.

use std::collections::HashSet;
use std::sync::Arc;

use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::events::{decode_file_event, FileEvent};
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig, ReconcileReport};
use siar_blob_manifest::gc::sweep;
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, ManifestId, TransferId};
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::{build_manifest, BlobManifest};
use siar_blob_manifest::metadata_store::{BlobMetadataStore, StoolapBlobMetadata};
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobReference, BlobRole, ObjectId, RefOp, ReferenceStore, StoolapReferenceStore,
};
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::transfer_state::TransferState;
use siar_domain::{BlobSize, DeviceId};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::stoolap_store::StoolapEventStore;
use siar_event_log::{EventOrigin, EventStore, LocalLogOffset};
use siar_file_transfer_service::transfer_engine::{ReceiveRequest, TransferEngine};
use siar_file_transfer_service::FileTransferService;

/// One test's directory on disk. Everything durable lives under it.
struct Disk(std::path::PathBuf);
impl Drop for Disk {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn disk() -> Disk {
    let d = Disk(std::env::temp_dir().join(format!("siar-durable-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&d.0).unwrap();
    d
}

struct Fx {
    bytes: Vec<u8>,
    descriptor: BlobDescriptor,
    manifest: BlobManifest,
}

fn fixture(len: usize, seed: u8) -> Fx {
    let mut x = (seed as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);
    let bytes: Vec<u8> = (0..len)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect();
    let chunk = 1_000u32;
    let manifest = build_manifest(&bytes, chunk, &ManifestLimits::default()).unwrap();
    Fx {
        descriptor: BlobDescriptor {
            blob_id: BlobId::from_ciphertext(&bytes),
            size: BlobSize::parse(len as u64).unwrap(),
            chunking: ChunkingDescriptor { chunk_size: chunk },
            encryption: EncryptionDescriptor {
                algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                nonce: [0; 12],
            },
            media_type: None,
            manifest_id: None,
        },
        manifest,
        bytes,
    }
}

fn chunk(f: &Fx, i: u32) -> &[u8] {
    let c = &f.manifest.chunks[i as usize];
    &f.bytes[c.offset as usize..(c.offset + c.size as u64) as usize]
}

fn origin() -> EventOrigin {
    EventOrigin::LocalDevice(DeviceId::new())
}
fn ns() -> NamespaceId {
    NamespaceId::new("personal").unwrap()
}
fn obj(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}

type Engine = TransferEngine<StoolapBlobMetadata, StoolapReferenceStore>;

/// A "process": every handle opened fresh from the files. Dropping it is
/// the crash.
struct Process {
    engine: Engine,
    log: Arc<StoolapEventStore>,
    refs: Arc<StoolapReferenceStore>,
}

fn p(d: &Disk, name: &str) -> String {
    d.0.join(name).to_str().unwrap().to_owned()
}

fn start(d: &Disk) -> Process {
    let cfg = FsStoreConfig {
        limits: StoreLimits::default(),
        durability: DurabilityPolicy::None,
        ..FsStoreConfig::default()
    };
    let store = FsBlobStore::open(
        d.0.join("blobs"),
        cfg,
        StoolapBlobMetadata::open(&p(d, "meta.db")).unwrap(),
    )
    .unwrap();
    let refs = Arc::new(StoolapReferenceStore::open(&p(d, "refs.db")).unwrap());
    let log = Arc::new(StoolapEventStore::open(&p(d, "log.db")).unwrap());
    let service = FileTransferService::new().with_event_log(log.clone());
    Process {
        engine: TransferEngine::new(service, store, refs.clone(), ns()),
        log,
        refs,
    }
}

fn req(f: &Fx, transfer: TransferId, object: &str) -> ReceiveRequest {
    ReceiveRequest {
        transfer_id: transfer,
        manifest_id: ManifestId::new(),
        logical_attachment_id: LogicalAttachmentId::new(),
        descriptor: f.descriptor,
        manifest: f.manifest.clone(),
        object: obj(object),
        role: BlobRole::MessageAttachment,
        expires: TransferExpiry(u64::MAX),
        origin: origin(),
    }
}

async fn recorded(log: &StoolapEventStore) -> Vec<FileEvent> {
    let mut out = Vec::new();
    for s in log.read_log(LocalLogOffset(0), 1000).await.unwrap() {
        out.push(decode_file_event(s.envelope.schema_version, &s.envelope.payload).unwrap());
    }
    out
}

fn read_back(e: &Engine, f: &Fx) -> Vec<u8> {
    e.store()
        .open_sync(f.descriptor.blob_id)
        .unwrap()
        .stream_sync(1_000)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect()
}

fn hex(id: BlobId) -> String {
    id.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn object_path(d: &Disk, id: BlobId) -> std::path::PathBuf {
    let h = hex(id);
    d.0.join("blobs").join("objects").join(&h[..2]).join(h)
}

/// Restart protocol, then finish the transfer however recovery allows.
async fn restart_and_finish(d: &Disk, f: &Fx, t: TransferId, object: &str) -> Process {
    let pr = start(d);
    pr.engine.restore(&*pr.log).await.unwrap();
    let out = pr.engine.reconcile_after_restart(origin()).await.unwrap();
    if out.resumable.contains(&t) {
        pr.engine
            .resume_after_restart(t, obj(object), BlobRole::MessageAttachment, origin())
            .await
            .unwrap();
        for (s, e) in pr.engine.missing_ranges(t).unwrap() {
            for i in s..e {
                pr.engine.receive_chunk(t, i, chunk(f, i)).await.unwrap();
            }
        }
        assert_eq!(
            pr.engine.finish_receive(t).await.unwrap(),
            f.descriptor.blob_id
        );
    } else if out.completable_late.contains(&t) {
        let id = pr
            .engine
            .complete_late(t, obj(object), BlobRole::MessageAttachment, origin())
            .await
            .unwrap();
        assert_eq!(id, f.descriptor.blob_id);
    } else {
        assert!(
            out.failed_lost_staging.contains(&t) || pr.engine.service().current_state(t).is_none(),
            "unrecognised recovery outcome: {out:?}"
        );
        let t2 = TransferId::new();
        pr.engine.begin_receive(req(f, t2, object)).await.unwrap();
        for i in 0..f.manifest.chunks.len() as u32 {
            pr.engine.receive_chunk(t2, i, chunk(f, i)).await.unwrap();
        }
        pr.engine.finish_receive(t2).await.unwrap();
    }
    pr
}

// ---------------------------------------------------------------------

/// Every crash point in the chunk sequence, with durable metadata, refs
/// and log all reopened from disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_after_any_number_of_chunks_recovers_with_every_store_reopened_from_disk() {
    let f = fixture(7_300, 31); // 8 chunks
    let n = f.manifest.chunks.len() as u32;
    for k in 0..=n {
        let d = disk();
        let t = TransferId::new();
        {
            let pr = start(&d);
            pr.engine.begin_receive(req(&f, t, "msg-1")).await.unwrap();
            for i in 0..k {
                pr.engine.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
            }
            // The reference was written durably before any bytes (§148):
            assert_eq!(pr.refs.count_for(f.descriptor.blob_id).unwrap(), 1, "k={k}");
            // -- killed: every handle dropped here --
        }
        // The reference survived the crash on its own, before recovery ran.
        {
            let pr = start(&d);
            assert_eq!(
                pr.refs.count_for(f.descriptor.blob_id).unwrap(),
                1,
                "the durable reference store lost it at k={k}"
            );
        }
        let pr = restart_and_finish(&d, &f, t, "msg-1").await;
        assert!(
            read_back(&pr.engine, &f) == f.bytes,
            "bytes differ at k={k}"
        );
        assert_eq!(
            pr.refs.count_for(f.descriptor.blob_id).unwrap(),
            1,
            "resume must not double the reference (k={k})"
        );
        assert!(
            pr.engine.store().list_staging().unwrap().is_empty(),
            "k={k}"
        );
    }
}

/// The durable log is the record: after a crash it still says what the
/// transfer was doing, and replay reaches the same state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_durable_log_replays_to_the_pre_crash_state() {
    let d = disk();
    let f = fixture(4_000, 32);
    let t = TransferId::new();
    {
        let pr = start(&d);
        pr.engine.begin_receive(req(&f, t, "m")).await.unwrap();
        for i in 0..3 {
            pr.engine.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
        let before = recorded(&pr.log).await.len();
        assert!(before >= 2, "created + started at least");
    }
    let pr = start(&d);
    let evs = recorded(&pr.log).await;
    assert!(!evs.is_empty(), "the log survived the crash");
    let summary = pr.engine.restore(&*pr.log).await.unwrap();
    assert_eq!(summary.replayed.len(), 1);
    assert_eq!(summary.replayed[0].state, TransferState::InProgress);
    assert_eq!(summary.staging.resumable[0].verified_chunks, 3);
}

/// A transfer that completed before the crash stays completed, verified
/// and referenced exactly once; offering the same blob again is a no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completed_transfer_survives_a_restart_and_a_repeat_costs_nothing() {
    let d = disk();
    let f = fixture(5_000, 33);
    let t = TransferId::new();
    {
        let pr = start(&d);
        pr.engine.begin_receive(req(&f, t, "msg-1")).await.unwrap();
        for i in 0..5 {
            pr.engine.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
        pr.engine.finish_receive(t).await.unwrap();
    }
    let pr = start(&d);
    let summary = pr.engine.restore(&*pr.log).await.unwrap();
    assert!(summary
        .replayed
        .iter()
        .any(|r| r.transfer_id == t && r.state == TransferState::Completed));
    assert!(read_back(&pr.engine, &f) == f.bytes);
    assert_eq!(pr.refs.count_for(f.descriptor.blob_id).unwrap(), 1);

    // The same blob offered again completes immediately without bytes.
    let t2 = TransferId::new();
    let st = pr.engine.begin_receive(req(&f, t2, "msg-2")).await.unwrap();
    assert_eq!(st, TransferState::Completed);
    assert_eq!(pr.refs.count_for(f.descriptor.blob_id).unwrap(), 2);
}

/// Cancelling after a crash releases the durable reference, so GC can
/// reclaim the partial data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_after_a_crash_releases_the_durable_reference() {
    let d = disk();
    let f = fixture(4_000, 34);
    let t = TransferId::new();
    {
        let pr = start(&d);
        pr.engine.begin_receive(req(&f, t, "m")).await.unwrap();
        pr.engine.receive_chunk(t, 0, chunk(&f, 0)).await.unwrap();
    }
    let pr = start(&d);
    pr.engine.restore(&*pr.log).await.unwrap();
    pr.engine.reconcile_after_restart(origin()).await.unwrap();
    pr.engine
        .resume_after_restart(t, obj("m"), BlobRole::MessageAttachment, origin())
        .await
        .unwrap();
    pr.engine.cancel(t).await.unwrap();
    assert_eq!(pr.refs.count_for(f.descriptor.blob_id).unwrap(), 0);
    drop(pr);
    // …and that is durable too.
    let pr = start(&d);
    assert_eq!(pr.refs.count_for(f.descriptor.blob_id).unwrap(), 0);
}

// ---- durable GC crash states -----------------------------------------

fn seed_blob(store: &FsBlobStore<StoolapBlobMetadata>, n: u8) -> BlobId {
    let bytes = vec![n; 700];
    let m = build_manifest(&bytes, 700, &ManifestLimits::default()).unwrap();
    let desc = BlobDescriptor {
        blob_id: m.blob_id,
        size: BlobSize::parse(700).unwrap(),
        chunking: ChunkingDescriptor { chunk_size: 700 },
        encryption: EncryptionDescriptor {
            algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
            nonce: [0; 12],
        },
        media_type: None,
        manifest_id: None,
    };
    let mut w = store.begin_write_with_manifest(desc, Some(m)).unwrap();
    w.write_chunk_sync(0, &bytes).unwrap();
    w.finalize_sync().unwrap()
}

/// The two states a sweep killed mid-flight leaves — a row with no file,
/// a file with no row — are now REAL persisted states: the durable
/// metadata is closed and reopened between the crash and the repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_during_gc_with_durable_metadata_is_repaired_on_the_next_start() {
    let d = disk();
    let (referenced, unreferenced) = {
        let pr = start(&d);
        let store = pr.engine.store();
        let referenced: Vec<BlobId> = (1..=3).map(|n| seed_blob(store, n)).collect();
        let unreferenced: Vec<BlobId> = (10..=13).map(|n| seed_blob(store, n)).collect();
        for (i, id) in referenced.iter().enumerate() {
            pr.refs
                .apply(&[RefOp::Add(BlobReference {
                    owner_namespace: ns(),
                    object_id: obj(&format!("keep-{i}")),
                    blob_id: *id,
                    role: BlobRole::MessageAttachment,
                })])
                .unwrap();
        }
        // The sweeper died between deleting a file and its row…
        std::fs::remove_file(object_path(&d, unreferenced[0])).unwrap();
        // …and between deleting a row and its file.
        assert!(store.metadata().remove(unreferenced[1]).unwrap());
        (referenced, unreferenced)
        // -- killed --
    };

    let pr = start(&d);
    let store = pr.engine.store();
    // The durable rows really did survive the crash (7 seeded, 1 removed).
    assert_eq!(store.metadata().count().unwrap(), 6);
    let report = store.reconcile_metadata().unwrap();
    assert_eq!(
        report,
        ReconcileReport {
            rows_added: 1,
            rows_removed: 1,
            corrupt_quarantined: 0
        }
    );
    let policy = GcPolicy {
        grace_period_millis: 0,
    };
    let none = HashSet::new();
    for now in [1u64, 10, 100] {
        sweep(store, &*pr.refs, &none, &none, now, &policy, 2).unwrap();
        for id in &referenced {
            assert!(store.verify_object(*id).unwrap(), "referenced blob damaged");
        }
    }
    for id in &unreferenced {
        assert!(!store.contains_sync(*id), "unreferenced blob survived");
    }
    assert_eq!(store.metadata().count().unwrap(), 3);

    // And the repaired state is itself durable.
    drop(pr);
    let pr = start(&d);
    assert_eq!(pr.engine.store().metadata().count().unwrap(), 3);
    assert_eq!(
        pr.engine.store().reconcile_metadata().unwrap(),
        ReconcileReport::default()
    );
}
