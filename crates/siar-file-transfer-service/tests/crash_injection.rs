//! Spec 05 §193 crash injection: every named crash point, on real
//! files, with every in-memory object dropped and the directories
//! reopened — plus torn writes (§194) and the states a crashed GC sweep
//! leaves behind.
//!
//! A "crash" here is exactly what a killed process leaves: whatever the
//! files and the event log held at that instant, nothing else.

use std::collections::HashSet;
use std::sync::Arc;

use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::events::{decode_file_event, FileEvent};
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::gc::sweep;
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, ManifestId, TransferId};
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::{build_manifest, BlobManifest};
use siar_blob_manifest::metadata_store::{BlobMetadataStore, InMemoryBlobMetadata};
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::transfer_state::TransferState;
use siar_domain::{BlobSize, DeviceId};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, EventStore, InMemoryEventStore, LocalLogOffset};
use siar_file_transfer_service::transfer_engine::{ReceiveRequest, TransferEngine};
use siar_file_transfer_service::FileTransferService;

struct Root(std::path::PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn root() -> Root {
    Root(std::env::temp_dir().join(format!("siar-engine-{}", uuid::Uuid::new_v4())))
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

type Engine = TransferEngine<InMemoryBlobMetadata, InMemoryReferenceStore>;

fn store_at(root: &Root, limits: StoreLimits) -> FsBlobStore<InMemoryBlobMetadata> {
    let cfg = FsStoreConfig {
        limits,
        durability: DurabilityPolicy::None,
        ..FsStoreConfig::default()
    };
    FsBlobStore::open(&root.0, cfg, InMemoryBlobMetadata::new()).unwrap()
}

fn engine(
    root: &Root,
    log: &Arc<InMemoryEventStore>,
    refs: &Arc<InMemoryReferenceStore>,
    limits: StoreLimits,
) -> Engine {
    let service = FileTransferService::new().with_event_log(log.clone());
    TransferEngine::new(service, store_at(root, limits), refs.clone(), ns())
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

#[allow(dead_code)]
async fn recorded(log: &InMemoryEventStore) -> Vec<FileEvent> {
    let mut out = Vec::new();
    for s in log.read_log(LocalLogOffset(0), 1000).await.unwrap() {
        out.push(decode_file_event(s.envelope.schema_version, &s.envelope.payload).unwrap());
    }
    out
}

#[allow(dead_code)]
fn kinds(evs: &[FileEvent]) -> Vec<&'static str> {
    evs.iter()
        .map(|e| match e {
            FileEvent::TransferCreated { .. } => "created",
            FileEvent::TransferAccepted { .. } => "accepted",
            FileEvent::TransferStarted { .. } => "started",
            FileEvent::TransferPaused { .. } => "paused",
            FileEvent::TransferResumed { .. } => "resumed",
            FileEvent::TransferCompleted { .. } => "completed",
            FileEvent::TransferCancelled { .. } => "cancelled",
            FileEvent::TransferFailed { .. } => "failed",
            FileEvent::BlobVerified { .. } => "verified",
        })
        .collect()
}

fn defaults() -> StoreLimits {
    StoreLimits::default()
}

fn setup() -> (Root, Arc<InMemoryEventStore>, Arc<InMemoryReferenceStore>) {
    (
        root(),
        Arc::new(InMemoryEventStore::new()),
        Arc::new(InMemoryReferenceStore::new()),
    )
}

use siar_blob_manifest::fs_store::ReconcileReport;
use siar_blob_manifest::reference_store::{BlobReference, RefOp};

fn hex(id: BlobId) -> String {
    id.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn part_path(r: &Root, id: BlobId) -> std::path::PathBuf {
    r.0.join("staging").join(format!("{}.part", hex(id)))
}

fn object_path(r: &Root, id: BlobId) -> std::path::PathBuf {
    let h = hex(id);
    r.0.join("objects").join(&h[..2]).join(h)
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

/// A fresh process: reopen everything, then run the restart protocol and
/// finish the transfer whichever way the recovered state allows.
async fn restart_and_finish(
    r: &Root,
    log: &Arc<InMemoryEventStore>,
    refs: &Arc<InMemoryReferenceStore>,
    f: &Fx,
    t: TransferId,
    object: &str,
) -> Engine {
    let e = engine(r, log, refs, defaults());
    e.restore(&**log).await.unwrap();
    let out = e.reconcile_after_restart(origin()).await.unwrap();
    if out.resumable.contains(&t) {
        e.resume_after_restart(t, obj(object), BlobRole::MessageAttachment, origin())
            .await
            .unwrap();
        for (s, end) in e.missing_ranges(t).unwrap() {
            for i in s..end {
                e.receive_chunk(t, i, chunk(f, i)).await.unwrap();
            }
        }
        assert_eq!(e.finish_receive(t).await.unwrap(), f.descriptor.blob_id);
    } else if out.completable_late.contains(&t) {
        let id = e
            .complete_late(t, obj(object), BlobRole::MessageAttachment, origin())
            .await
            .unwrap();
        assert_eq!(id, f.descriptor.blob_id);
    } else {
        // Nothing of it survived: the honest outcome is a failed transfer
        // that can be started afresh — never a half-state.
        assert!(
            out.failed_lost_staging.contains(&t) || e.service().current_state(t).is_none(),
            "unrecognised recovery outcome: {out:?}"
        );
        let t2 = TransferId::new();
        e.begin_receive(req(f, t2, object)).await.unwrap();
        for i in 0..f.manifest.chunks.len() as u32 {
            e.receive_chunk(t2, i, chunk(f, i)).await.unwrap();
        }
        e.finish_receive(t2).await.unwrap();
    }
    e
}

// ---- §193: mid chunk, after chunk write before journal --------------

/// The crash lands after *every possible* number of chunks — including
/// none and all-of-them-but-not-finalized — and each time the transfer
/// ends byte-exact, referenced once, with no staging left over.
#[tokio::test]
async fn a_crash_after_any_number_of_chunks_recovers_to_the_exact_file() {
    let f = fixture(7_300, 21); // 8 chunks
    let n = f.manifest.chunks.len() as u32;
    for k in 0..=n {
        let (r, log, refs) = setup();
        let t = TransferId::new();
        {
            let e = engine(&r, &log, &refs, defaults());
            e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
            for i in 0..k {
                e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
            }
            // -- killed --
        }
        let e = restart_and_finish(&r, &log, &refs, &f, t, "msg-1").await;
        assert!(
            read_back(&e, &f) == f.bytes,
            "bytes differ after a crash at k={k}"
        );
        assert!(
            e.store().list_staging().unwrap().is_empty(),
            "staging left over at k={k}"
        );
        assert_eq!(refs.count_for(f.descriptor.blob_id).unwrap(), 1, "k={k}");
    }
}

/// The journal can lag the disk by the chunks written after its last
/// durable write; disk is the truth, the log is advisory (§193 "after
/// chunk write before journal").
#[tokio::test]
async fn a_journal_that_is_behind_the_disk_never_causes_a_rewrite_or_a_loss() {
    let (r, log, refs) = setup();
    let f = fixture(6_000, 22);
    let t = TransferId::new();
    {
        let e = engine(&r, &log, &refs, defaults());
        e.begin_receive(req(&f, t, "m")).await.unwrap();
        for i in 0..5 {
            e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
    }
    let e = engine(&r, &log, &refs, defaults());
    let summary = e.restore(&*log).await.unwrap();
    // The log knows the transfer started; disk knows five verified chunks.
    assert_eq!(summary.staging.resumable[0].verified_chunks, 5);
    e.reconcile_after_restart(origin()).await.unwrap();
    let p = e
        .resume_after_restart(t, obj("m"), BlobRole::MessageAttachment, origin())
        .await
        .unwrap();
    assert_eq!(
        (p.received, p.total),
        (5, 6),
        "only the missing chunk remains"
    );
}

// ---- §194 torn writes ---------------------------------------------

#[tokio::test]
async fn a_torn_staging_file_resumes_from_exactly_the_chunks_that_verify() {
    // (what to do to the .part file, chunks that must survive)
    type Tear = (&'static str, fn(&std::path::Path), u32);
    let tears: [Tear; 3] = [
        (
            "one byte flipped inside chunk 3",
            |p| {
                let mut b = std::fs::read(p).unwrap();
                b[3_500] ^= 0xFF;
                std::fs::write(p, b).unwrap();
            },
            4,
        ),
        (
            "file cut off in the middle of chunk 2",
            |p| {
                let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
                f.set_len(2_500).unwrap();
            },
            2,
        ),
        (
            "chunk 1 zeroed (a lost page)",
            |p| {
                let mut b = std::fs::read(p).unwrap();
                b[1_000..2_000].fill(0);
                std::fs::write(p, b).unwrap();
            },
            4,
        ),
    ];
    for (what, tear, survivors) in tears {
        let (r, log, refs) = setup();
        let f = fixture(8_000, 23);
        let t = TransferId::new();
        {
            let e = engine(&r, &log, &refs, defaults());
            e.begin_receive(req(&f, t, "m")).await.unwrap();
            for i in 0..5 {
                e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
            }
        }
        tear(&part_path(&r, f.descriptor.blob_id));

        let e = engine(&r, &log, &refs, defaults());
        let s = e.restore(&*log).await.unwrap();
        assert_eq!(
            s.staging.resumable[0].verified_chunks, survivors,
            "{what}: recovery must trust only what re-verifies"
        );
        let e = restart_and_finish(&r, &log, &refs, &f, t, "m").await;
        assert!(read_back(&e, &f) == f.bytes, "{what}");
    }
}

// ---- §193: during finalization -------------------------------------

#[tokio::test]
async fn a_published_blob_with_damaged_bytes_is_never_reported_complete() {
    let (r, log, refs) = setup();
    let f = fixture(3_000, 24);
    let t = TransferId::new();
    {
        let e = engine(&r, &log, &refs, defaults());
        e.begin_receive(req(&f, t, "m")).await.unwrap();
        for i in 0..3 {
            e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
    }
    {
        // Crash after the rename…
        let store = store_at(&r, defaults());
        let w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        w.finalize_sync().unwrap();
    }
    // …and the object is damaged (a torn / bit-rotted file).
    let obj_path = object_path(&r, f.descriptor.blob_id);
    let mut b = std::fs::read(&obj_path).unwrap();
    b[1_234] ^= 0x55;
    std::fs::write(&obj_path, b).unwrap();

    let e = engine(&r, &log, &refs, defaults());
    e.restore(&*log).await.unwrap();
    let out = e.reconcile_after_restart(origin()).await.unwrap();
    // Whatever recovery decides, "complete" must not be an unverified claim.
    if out.completable_late.contains(&t) {
        let r = e
            .complete_late(t, obj("m"), BlobRole::MessageAttachment, origin())
            .await;
        assert!(r.is_err(), "damaged bytes must not complete the transfer");
    }
    assert_ne!(e.service().current_state(t), Some(TransferState::Completed));
}

// ---- §193: during GC ------------------------------------------------

fn seed_blob(store: &FsBlobStore<InMemoryBlobMetadata>, n: u8) -> BlobId {
    let bytes = vec![n; 700];
    let m = build_manifest(&bytes, 700, &ManifestLimits::default()).unwrap();
    let d = BlobDescriptor {
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
    let mut w = store.begin_write_with_manifest(d, Some(m)).unwrap();
    w.write_chunk_sync(0, &bytes).unwrap();
    w.finalize_sync().unwrap()
}

/// A sweep killed between "delete the file" and "delete the row" (or the
/// reverse) leaves a row with no object, or an object with no row. The
/// next start reconciles both; a repeat sweep finishes the job; and not
/// one referenced blob is lost at any step.
#[tokio::test]
async fn a_crash_during_gc_leaves_a_state_the_next_start_repairs_without_losing_referenced_blobs() {
    let r = root();
    let refs = Arc::new(InMemoryReferenceStore::new());
    // The metadata store here is in-memory, so it would vanish with a real
    // "process death"; production metadata is durable. To keep the rows
    // alive across the crash, the crashed sweeper's leftovers are applied
    // to the open store (the files are real either way) and the next
    // start's repair, `reconcile_metadata`, runs on them.
    let store = store_at(&r, defaults());
    let referenced: Vec<BlobId> = (1..=3).map(|n| seed_blob(&store, n)).collect();
    let unreferenced: Vec<BlobId> = (10..=13).map(|n| seed_blob(&store, n)).collect();
    for (i, id) in referenced.iter().enumerate() {
        refs.apply(&[RefOp::Add(BlobReference {
            owner_namespace: ns(),
            object_id: obj(&format!("keep-{i}")),
            blob_id: *id,
            role: BlobRole::MessageAttachment,
        })])
        .unwrap();
    }
    // Crashed sweeper, state 1: file deleted, row still there.
    std::fs::remove_file(object_path(&r, unreferenced[0])).unwrap();
    // State 2: row deleted, file still there.
    assert!(store.metadata().remove(unreferenced[1]).unwrap());

    let report = store.reconcile_metadata().unwrap();
    assert_eq!(
        report,
        ReconcileReport {
            rows_added: 1,
            rows_removed: 1,
            corrupt_quarantined: 0
        },
        "one dangling row dropped, one orphan object re-registered"
    );

    let policy = GcPolicy {
        grace_period_millis: 0,
    };
    let none = HashSet::new();
    for now in [1u64, 10, 100] {
        sweep(&store, &*refs, &none, &none, now, &policy, 2).unwrap();
        for id in &referenced {
            assert!(store.verify_object(*id).unwrap(), "referenced blob damaged");
        }
    }
    for id in &unreferenced {
        assert!(!store.contains_sync(*id), "unreferenced blob survived GC");
    }
    assert_eq!(
        store.metadata().count().unwrap(),
        3,
        "only the referenced rows remain"
    );
}

/// The same sweep run twice back to back is a no-op the second time:
/// resuming a half-finished GC is just running it again.
#[tokio::test]
async fn gc_is_idempotent_so_resuming_after_a_crash_is_just_running_it_again() {
    let r = root();
    let refs = Arc::new(InMemoryReferenceStore::new());
    let store = store_at(&r, defaults());
    for n in 20..26 {
        seed_blob(&store, n);
    }
    let policy = GcPolicy {
        grace_period_millis: 0,
    };
    let none = HashSet::new();
    sweep(&store, &*refs, &none, &none, 1, &policy, 3).unwrap();
    sweep(&store, &*refs, &none, &none, 10, &policy, 3).unwrap();
    let after_two = store.metadata().count().unwrap();
    let again = sweep(&store, &*refs, &none, &none, 20, &policy, 3).unwrap();
    assert_eq!(store.metadata().count().unwrap(), after_two);
    assert_eq!(after_two, 0);
    let _ = again;
}
