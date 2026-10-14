//! The state machine moving real bytes: `FileTransferService` +
//! `FsBlobStore` + the reference model + the event log, on real files,
//! including crashes (every in-memory object dropped, directories
//! reopened) and the failure modes the engine's docs promise to handle.

use std::collections::HashSet;
use std::sync::Arc;

use siar_blob_manifest::blob_io::BlobReader;
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
use siar_blob_manifest::metadata_store::InMemoryBlobMetadata;
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::reference_store::{
    release_object_references, BlobRole, InMemoryReferenceStore, ObjectId, ReferenceStore,
};
use siar_blob_manifest::storage_lifecycle::GcPolicy;
use siar_blob_manifest::store_error::BlobError;
use siar_blob_manifest::transfer_state::TransferState;
use siar_domain::{BlobSize, DeviceId};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, EventStore, InMemoryEventStore, LocalLogOffset};
use siar_file_transfer_service::transfer_engine::{
    ChunkProgress, EngineError, ReceiveRequest, TransferEngine,
};
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

async fn recorded(log: &InMemoryEventStore) -> Vec<FileEvent> {
    let mut out = Vec::new();
    for s in log.read_log(LocalLogOffset(0), 1000).await.unwrap() {
        out.push(decode_file_event(s.envelope.schema_version, &s.envelope.payload).unwrap());
    }
    out
}

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

#[tokio::test]
async fn a_transfer_receives_verifies_publishes_records_and_references() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(7_500, 1);
    let t = TransferId::new();

    assert_eq!(
        e.begin_receive(req(&f, t, "msg-1")).await.unwrap(),
        TransferState::InProgress
    );
    // The reference exists before a single byte does (§148).
    assert_eq!(refs.count_for(f.descriptor.blob_id).unwrap(), 1);
    assert!(!e.store().contains_sync(f.descriptor.blob_id));

    let n = f.manifest.chunks.len() as u32;
    let mut last = None;
    for i in (0..n).rev() {
        last = Some(e.receive_chunk(t, i, chunk(&f, i)).await.unwrap());
    }
    assert_eq!(
        last.unwrap(),
        ChunkProgress {
            received: n,
            total: n,
            complete: true
        }
    );

    let id = e.finish_receive(t).await.unwrap();
    assert_eq!(id, f.descriptor.blob_id);
    assert_eq!(e.service().current_state(t), Some(TransferState::Completed));
    assert_eq!(
        kinds(&recorded(&log).await),
        ["created", "accepted", "started", "completed", "verified"]
    );

    let reader = e.store().open_sync(id).unwrap();
    let got: Vec<u8> = reader
        .stream_sync(2_048)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect();
    assert!(got == f.bytes, "stored bytes differ from the source");
    assert_eq!(refs.count_for(id).unwrap(), 1, "the reference stays");
    assert_eq!(reader.size(), 7_500);
}

#[tokio::test]
async fn a_transfer_that_cannot_fit_is_refused_up_front_and_leaves_nothing_behind() {
    let (r, log, refs) = setup();
    let tiny = StoreLimits {
        max_total_bytes: 1_000,
        max_blob_bytes: 1_000,
        max_concurrent_writes: 4,
    };
    let e = engine(&r, &log, &refs, tiny);
    let f = fixture(5_000, 2);
    let t = TransferId::new();

    let err = e.begin_receive(req(&f, t, "msg-1")).await.unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Blob(BlobError::ReservationDenied { .. })
                | EngineError::Blob(BlobError::BlobTooLarge { .. })
        ),
        "{err:?}"
    );
    assert_eq!(e.service().current_state(t), Some(TransferState::Failed));
    assert_eq!(
        refs.count_for(f.descriptor.blob_id).unwrap(),
        0,
        "no leaked reference"
    );
    assert!(e.store().list_staging().unwrap().is_empty());
    assert_eq!(
        kinds(&recorded(&log).await),
        ["created", "accepted", "started", "failed"]
    );
}

#[tokio::test]
async fn a_corrupt_chunk_is_rejected_unwritten_and_repeated_corruption_fails_the_transfer() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(4_000, 3);
    let t = TransferId::new();
    e.begin_receive(req(&f, t, "msg-1")).await.unwrap();

    let mut bad = chunk(&f, 0).to_vec();
    bad[0] ^= 1;
    for _ in 0..2 {
        let err = e.receive_chunk(t, 0, &bad).await.unwrap_err();
        assert!(matches!(
            err,
            EngineError::Blob(BlobError::ChunkHashMismatch { index: 0 })
        ));
    }
    // Two bad ones are tolerated; the good chunk still goes in.
    assert_eq!(
        e.receive_chunk(t, 0, chunk(&f, 0)).await.unwrap().received,
        1
    );

    // A third corrupt chunk trips the limit.
    let err = e.receive_chunk(t, 1, &[0u8; 1000]).await.unwrap_err();
    assert_eq!(
        err,
        EngineError::TransferFailed {
            id: t,
            reason: "too many corrupt chunks"
        }
    );
    assert_eq!(e.service().current_state(t), Some(TransferState::Failed));
    assert!(
        e.store().list_staging().unwrap().is_empty(),
        "staging discarded"
    );
    assert_eq!(
        refs.count_for(f.descriptor.blob_id).unwrap(),
        0,
        "reference compensated"
    );
    assert!(matches!(
        e.receive_chunk(t, 3, chunk(&f, 3)).await,
        Err(EngineError::NotActive(_))
    ));
}

#[tokio::test]
async fn a_blob_whose_whole_hash_is_wrong_is_quarantined_and_the_transfer_fails() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(3_000, 4);
    // Chunk hashes are right for these bytes, but the *declared*
    // identity is something else: only the whole-blob check can see it.
    let fake = BlobId::from_ciphertext(b"a different blob entirely");
    let mut forged = fixture(3_000, 4);
    forged.descriptor.blob_id = fake;
    forged.manifest.blob_id = fake;
    let t = TransferId::new();
    e.begin_receive(req(&forged, t, "msg-1")).await.unwrap();
    for i in 0..3 {
        e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
    }
    let err = e.finish_receive(t).await.unwrap_err();
    assert_eq!(
        err,
        EngineError::TransferFailed {
            id: t,
            reason: "blob failed identity verification"
        }
    );
    assert_eq!(e.service().current_state(t), Some(TransferState::Failed));
    assert!(!e.store().contains_sync(fake));
    assert_eq!(refs.count_for(fake).unwrap(), 0);
    let q = std::fs::read_dir(r.0.join("quarantine")).unwrap().count();
    assert_eq!(
        q, 1,
        "the bad bytes are kept for diagnostics, never published"
    );
}

#[tokio::test]
async fn finishing_early_is_an_error_not_a_failure_and_the_transfer_carries_on() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(3_000, 5);
    let t = TransferId::new();
    e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
    e.receive_chunk(t, 0, chunk(&f, 0)).await.unwrap();

    let err = e.finish_receive(t).await.unwrap_err();
    assert_eq!(
        err,
        EngineError::Blob(BlobError::Incomplete { missing_chunks: 2 })
    );
    assert_eq!(
        e.service().current_state(t),
        Some(TransferState::InProgress),
        "still going"
    );

    e.receive_chunk(t, 1, chunk(&f, 1)).await.unwrap();
    e.receive_chunk(t, 2, chunk(&f, 2)).await.unwrap();
    assert_eq!(e.finish_receive(t).await.unwrap(), f.descriptor.blob_id);
}

#[tokio::test]
async fn pause_releases_space_keeps_progress_and_resume_re_verifies_it() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(6_000, 6);
    let t = TransferId::new();
    e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
    for i in 0..3 {
        e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
    }
    assert_eq!(e.store().ledger().reserved_bytes(), 6_000);

    assert_eq!(e.pause(t).await.unwrap(), TransferState::Paused);
    assert_eq!(
        e.store().ledger().reserved_bytes(),
        0,
        "a paused transfer holds no reservation"
    );
    assert_eq!(
        e.store().list_staging().unwrap().len(),
        1,
        "but its progress is kept"
    );
    assert!(matches!(
        e.receive_chunk(t, 3, chunk(&f, 3)).await,
        Err(EngineError::NotActive(_))
    ));

    assert_eq!(e.resume(t).await.unwrap(), TransferState::InProgress);
    let p = e.receive_chunk(t, 3, chunk(&f, 3)).await.unwrap();
    assert_eq!(
        (p.received, p.total),
        (4, 6),
        "the first three chunks were re-verified, not re-sent"
    );
    for i in 4..6 {
        e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
    }
    assert_eq!(e.finish_receive(t).await.unwrap(), f.descriptor.blob_id);
    assert_eq!(
        kinds(&recorded(&log).await),
        [
            "created",
            "accepted",
            "started",
            "paused",
            "resumed",
            "completed",
            "verified"
        ]
    );
}

#[tokio::test]
async fn cancel_discards_the_staging_and_releases_the_reference() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(4_000, 7);
    let t = TransferId::new();
    e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
    e.receive_chunk(t, 0, chunk(&f, 0)).await.unwrap();

    assert_eq!(e.cancel(t).await.unwrap(), TransferState::Cancelled);
    assert!(e.store().list_staging().unwrap().is_empty());
    assert_eq!(refs.count_for(f.descriptor.blob_id).unwrap(), 0);
    assert_eq!(e.store().ledger().reserved_bytes(), 0);
    assert!(matches!(
        e.finish_receive(t).await,
        Err(EngineError::NotActive(_))
    ));
}

#[tokio::test]
async fn a_blob_already_stored_completes_instantly_without_receiving_a_byte() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(3_000, 8);
    let t1 = TransferId::new();
    e.begin_receive(req(&f, t1, "msg-1")).await.unwrap();
    for i in 0..3 {
        e.receive_chunk(t1, i, chunk(&f, i)).await.unwrap();
    }
    e.finish_receive(t1).await.unwrap();

    // The same attachment forwarded in a second message.
    let t2 = TransferId::new();
    assert_eq!(
        e.begin_receive(req(&f, t2, "msg-2")).await.unwrap(),
        TransferState::Completed
    );
    assert_eq!(
        refs.count_for(f.descriptor.blob_id).unwrap(),
        2,
        "two messages, one blob"
    );
    assert_eq!(e.store().ledger().used_bytes(), 3_000, "stored once");
    assert!(matches!(
        e.receive_chunk(t2, 0, chunk(&f, 0)).await,
        Err(EngineError::NotActive(_))
    ));
}

#[tokio::test]
async fn after_a_crash_the_transfer_resumes_from_what_verifiably_survived() {
    let (r, log, refs) = setup();
    let f = fixture(8_000, 9);
    let t = TransferId::new();
    {
        let e = engine(&r, &log, &refs, defaults());
        e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
        for i in [0, 1, 2, 3, 4] {
            e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
        // -- process dies: engine, service, writers all dropped --
    }

    // A fresh process: empty service, reopened store, same log.
    let e = engine(&r, &log, &refs, defaults());
    assert_eq!(e.service().current_state(t), None, "nothing in memory");
    let summary = e.restore(&*log).await.unwrap();
    assert_eq!(summary.replayed.len(), 1);
    assert_eq!(summary.replayed[0].state, TransferState::InProgress);
    assert_eq!(summary.staging.resumable.len(), 1);
    assert_eq!(summary.staging.resumable[0].verified_chunks, 5);

    let outcome = e.reconcile_after_restart(origin()).await.unwrap();
    assert_eq!(outcome.resumable, vec![t]);
    assert!(outcome.failed_lost_staging.is_empty() && outcome.completable_late.is_empty());

    let p = e
        .resume_after_restart(t, obj("msg-1"), BlobRole::MessageAttachment, origin())
        .await
        .unwrap();
    assert_eq!((p.received, p.total), (5, 8));
    for i in 5..8 {
        e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
    }
    assert_eq!(e.finish_receive(t).await.unwrap(), f.descriptor.blob_id);
    assert_eq!(e.service().current_state(t), Some(TransferState::Completed));
    let got: Vec<u8> = e
        .store()
        .open_sync(f.descriptor.blob_id)
        .unwrap()
        .stream_sync(1_000)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect();
    assert!(got == f.bytes);
}

#[tokio::test]
async fn a_log_that_lags_behind_disk_is_reconciled_not_trusted() {
    // The best-effort `TransferStarted` was lost in the crash: the log
    // only reaches `Accepted`, though staging exists on disk.
    let (r, log, refs) = setup();
    let f = fixture(4_000, 10);
    let t = TransferId::new();
    let o = origin();
    {
        let store = store_at(&r, defaults());
        let mut w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        w.write_chunk_sync(0, chunk(&f, 0)).unwrap();
        w.write_chunk_sync(1, chunk(&f, 1)).unwrap();
    }
    for ev in [
        FileEvent::TransferCreated {
            transfer_id: t,
            manifest_id: ManifestId::new(),
            logical_attachment_id: LogicalAttachmentId::new(),
            blob_id: f.descriptor.blob_id,
            total_size_bytes: 4_000,
            chunk_count: 4,
        },
        FileEvent::TransferAccepted { transfer_id: t },
    ] {
        let stream = ev.stream_id();
        log.append(siar_event_log::AppendRequest {
            stream_id: stream,
            expected_version: log.stream_head(stream).await.unwrap(),
            events: vec![ev.into_new_event(o)],
        })
        .await
        .unwrap();
    }

    let e = engine(&r, &log, &refs, defaults());
    let s = e.restore(&*log).await.unwrap();
    assert_eq!(
        s.replayed[0].state,
        TransferState::Accepted,
        "log is behind"
    );
    assert_eq!(
        e.reconcile_after_restart(o).await.unwrap().resumable,
        vec![t]
    );

    let p = e
        .resume_after_restart(t, obj("msg-1"), BlobRole::MessageAttachment, o)
        .await
        .unwrap();
    assert_eq!(p.received, 2);
    assert_eq!(
        e.service().current_state(t),
        Some(TransferState::InProgress),
        "brought forward by applying Start"
    );
}

#[tokio::test]
async fn a_transfer_whose_staging_is_gone_is_failed_honestly_not_left_in_progress() {
    let (r, log, refs) = setup();
    let f = fixture(3_000, 11);
    let t = TransferId::new();
    {
        let e = engine(&r, &log, &refs, defaults());
        e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
        e.receive_chunk(t, 0, chunk(&f, 0)).await.unwrap();
    }
    // Staging lost (disk cleaned, user cleared cache…).
    for ent in std::fs::read_dir(r.0.join("staging")).unwrap() {
        std::fs::remove_file(ent.unwrap().path()).unwrap();
    }
    let e = engine(&r, &log, &refs, defaults());
    e.restore(&*log).await.unwrap();
    let out = e.reconcile_after_restart(origin()).await.unwrap();
    assert_eq!(out.failed_lost_staging, vec![t]);
    assert_eq!(e.service().current_state(t), Some(TransferState::Failed));
    assert!(e
        .resume_after_restart(t, obj("msg-1"), BlobRole::MessageAttachment, origin())
        .await
        .is_err());
}

#[tokio::test]
async fn a_blob_published_just_before_the_crash_is_completed_not_re_downloaded() {
    let (r, log, refs) = setup();
    let f = fixture(3_000, 12);
    let t = TransferId::new();
    {
        let e = engine(&r, &log, &refs, defaults());
        e.begin_receive(req(&f, t, "msg-1")).await.unwrap();
        for i in 0..3 {
            e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
        }
        // -- crash: all chunks staged, nothing finalized or recorded --
    }
    {
        // The state a crash *after the rename* leaves: the blob is
        // published, but the engine never recorded it.
        let store = store_at(&r, defaults());
        let w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        assert_eq!(w.received_chunks(), 3, "the staging was complete");
        assert_eq!(w.finalize_sync().unwrap(), f.descriptor.blob_id);
    }

    let e = engine(&r, &log, &refs, defaults());
    e.restore(&*log).await.unwrap();
    let out = e.reconcile_after_restart(origin()).await.unwrap();
    assert_eq!(out.completable_late, vec![t]);

    let id = e
        .complete_late(t, obj("msg-1"), BlobRole::MessageAttachment, origin())
        .await
        .unwrap();
    assert_eq!(id, f.descriptor.blob_id);
    assert_eq!(e.service().current_state(t), Some(TransferState::Completed));
    assert_eq!(refs.count_for(id).unwrap(), 1);
}

#[tokio::test]
async fn deleting_the_message_eventually_frees_the_blob_end_to_end() {
    let (r, log, refs) = setup();
    let e = engine(&r, &log, &refs, defaults());
    let f = fixture(5_000, 13);
    let t = TransferId::new();
    e.begin_receive(req(&f, t, "msg-gone")).await.unwrap();
    for i in 0..5 {
        e.receive_chunk(t, i, chunk(&f, i)).await.unwrap();
    }
    let id = e.finish_receive(t).await.unwrap();
    assert_eq!(e.store().ledger().used_bytes(), 5_000);

    // User deletes the message: its reference goes; GC then waits out
    // the grace period before touching the bytes.
    let rel = release_object_references(&*refs, &ns(), &obj("msg-gone")).unwrap();
    assert_eq!(rel.now_unreferenced, vec![id]);
    let pol = GcPolicy {
        grace_period_millis: 1_000,
    };
    let none = HashSet::new();
    sweep(e.store(), &*refs, &none, &none, 0, &pol, 100).unwrap();
    assert!(e.store().contains_sync(id), "still inside grace");
    let r2 = sweep(e.store(), &*refs, &none, &none, 1_000, &pol, 100).unwrap();
    assert_eq!(r2.deleted, 1);
    assert!(!e.store().contains_sync(id));
    assert_eq!(e.store().ledger().used_bytes(), 0);
}
