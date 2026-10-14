//! §148-157 against the real store: references decide what lives, a
//! grace period protects what just became unreferenced, shared blobs
//! survive a single message's deletion, and staging (orphaned or
//! partial) is cleaned up by policy — with real files.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig};
use siar_blob_manifest::gc::{reclaim_staging, sweep, sweep_staging, PartialRetentionPolicy};
use siar_blob_manifest::ids::BlobId;
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::{build_manifest, BlobManifest};
use siar_blob_manifest::metadata_store::{BlobMetadataStore, InMemoryBlobMetadata};
use siar_blob_manifest::reference_store::{
    disappearing_release_at, release_object_references, user_save_reference, BlobReference,
    BlobRole, InMemoryReferenceStore, ObjectId, RefOp, ReferenceStore,
};
use siar_blob_manifest::storage_lifecycle::{BlobClassification, GcPolicy};
use siar_domain::BlobSize;
use siar_event_log::namespace::NamespaceId;

struct Root(std::path::PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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

fn setup() -> (
    Root,
    FsBlobStore<InMemoryBlobMetadata>,
    InMemoryReferenceStore,
) {
    let root = Root(std::env::temp_dir().join(format!("siar-gc-{}", uuid::Uuid::new_v4())));
    let cfg = FsStoreConfig {
        durability: DurabilityPolicy::None,
        ..FsStoreConfig::default()
    };
    let store = FsBlobStore::open(&root.0, cfg, InMemoryBlobMetadata::new()).unwrap();
    (root, store, InMemoryReferenceStore::new())
}

fn put(store: &FsBlobStore<InMemoryBlobMetadata>, f: &Fx) -> BlobId {
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    for c in &f.manifest.chunks {
        w.write_chunk_sync(
            c.index,
            &f.bytes[c.offset as usize..(c.offset + c.size as u64) as usize],
        )
        .unwrap();
    }
    w.finalize_sync().unwrap()
}

fn ns() -> NamespaceId {
    NamespaceId::new("personal").unwrap()
}
fn obj(s: &str) -> ObjectId {
    ObjectId::new(s).unwrap()
}
fn reference(o: &str, b: BlobId, role: BlobRole) -> BlobReference {
    BlobReference {
        owner_namespace: ns(),
        object_id: obj(o),
        blob_id: b,
        role,
    }
}

const HOUR: u64 = 3_600_000;
fn policy() -> GcPolicy {
    GcPolicy {
        grace_period_millis: HOUR,
    }
}

fn run(
    store: &FsBlobStore<InMemoryBlobMetadata>,
    refs: &InMemoryReferenceStore,
    now: u64,
) -> siar_blob_manifest::gc::GcReport {
    sweep(
        store,
        refs,
        &HashSet::new(),
        &HashSet::new(),
        now,
        &policy(),
        100,
    )
    .unwrap()
}

#[test]
fn an_unreferenced_blob_is_deleted_only_after_its_grace_period() {
    let (_r, store, refs) = setup();
    let f = fixture(3_000, 1);
    let id = put(&store, &f);
    assert_eq!(store.ledger().used_bytes(), 3_000);

    // First sweep: nothing references it. The clock starts; nothing is deleted.
    let r = run(&store, &refs, 1_000);
    assert_eq!((r.grace_started, r.deleted), (1, 0));
    assert!(store.contains_sync(id));

    // Inside the grace period: still there.
    let r = run(&store, &refs, 1_000 + HOUR - 1);
    assert_eq!((r.deferred_in_grace, r.deleted), (1, 0));
    assert!(store.contains_sync(id));

    // Grace elapsed and still unreferenced: gone, space returned.
    let r = run(&store, &refs, 1_000 + HOUR);
    assert_eq!((r.deleted, r.bytes_freed), (1, 3_000));
    assert!(!store.contains_sync(id));
    assert!(store.metadata().get(id).unwrap().is_none());
    assert_eq!(store.ledger().used_bytes(), 0);
    assert_eq!(
        refs.unreferenced_since(id).unwrap(),
        None,
        "mark cleaned up"
    );
}

#[test]
fn age_alone_never_deletes_a_referenced_blob() {
    let (_r, store, refs) = setup();
    let id = put(&store, &fixture(2_000, 2));
    refs.apply(&[RefOp::Add(reference(
        "msg-1",
        id,
        BlobRole::MessageAttachment,
    ))])
    .unwrap();
    // Sweep "ten years later", repeatedly.
    for years in [1u64, 5, 10] {
        let r = run(&store, &refs, years * 365 * 24 * HOUR);
        assert_eq!((r.referenced, r.deleted), (1, 0));
    }
    assert!(store.contains_sync(id));
}

#[test]
fn a_reference_added_during_grace_saves_the_blob_and_a_later_removal_restarts_the_clock() {
    let (_r, store, refs) = setup();
    let id = put(&store, &fixture(1_500, 3));
    run(&store, &refs, 0); // grace starts at 0
    assert_eq!(refs.unreferenced_since(id).unwrap(), Some(0));

    // Re-shared 30 minutes in: the mark is cleared.
    refs.apply(&[RefOp::Add(reference(
        "msg-new",
        id,
        BlobRole::MessageAttachment,
    ))])
    .unwrap();
    let r = run(&store, &refs, HOUR / 2);
    assert_eq!(r.referenced, 1);
    assert_eq!(refs.unreferenced_since(id).unwrap(), None);

    // Later the reference goes away again: a FULL new grace period,
    // not what was left of the old one.
    refs.apply(&[RefOp::Remove(reference(
        "msg-new",
        id,
        BlobRole::MessageAttachment,
    ))])
    .unwrap();
    run(&store, &refs, 10 * HOUR); // new clock starts here
    assert_eq!(run(&store, &refs, 10 * HOUR + HOUR - 1).deleted, 0);
    assert_eq!(run(&store, &refs, 11 * HOUR).deleted, 1);
}

#[test]
fn deleting_messages_end_to_end_a_shared_blob_survives_until_the_last_one_goes() {
    let (_r, store, refs) = setup();
    let shared = put(&store, &fixture(2_000, 4));
    let solo = put(&store, &fixture(2_500, 5));
    refs.apply(&[
        RefOp::Add(reference("msg-a", shared, BlobRole::MessageAttachment)),
        RefOp::Add(reference("msg-b", shared, BlobRole::MessageAttachment)),
        RefOp::Add(reference("msg-b", solo, BlobRole::MessageAttachment)),
    ])
    .unwrap();

    // Delete msg-b: `solo` becomes a candidate, `shared` does not.
    let rel = release_object_references(&refs, &ns(), &obj("msg-b")).unwrap();
    assert_eq!(rel.now_unreferenced, vec![solo]);

    run(&store, &refs, 0);
    let r = run(&store, &refs, HOUR);
    assert_eq!(r.deleted, 1);
    assert!(!store.contains_sync(solo));
    assert!(store.contains_sync(shared), "still referenced by msg-a");

    // Delete msg-a: now `shared` goes too, after ITS grace.
    release_object_references(&refs, &ns(), &obj("msg-a")).unwrap();
    run(&store, &refs, 2 * HOUR);
    assert!(store.contains_sync(shared), "grace just started");
    run(&store, &refs, 3 * HOUR);
    assert!(!store.contains_sync(shared));
    assert_eq!(store.ledger().used_bytes(), 0);
}

#[test]
fn a_disappearing_message_releases_after_expiry_plus_grace_unless_the_user_saved_it() {
    let (_r, store, refs) = setup();
    let plain = put(&store, &fixture(1_200, 6));
    let saved = put(&store, &fixture(1_300, 7));
    refs.apply(&[
        RefOp::Add(reference("vanish-1", plain, BlobRole::MessageAttachment)),
        RefOp::Add(reference("vanish-2", saved, BlobRole::MessageAttachment)),
        RefOp::Add(user_save_reference(ns(), obj("saved-copy"), saved)),
    ])
    .unwrap();

    let message_expiry = 10_000;
    let release_at = disappearing_release_at(message_expiry, &policy());
    assert_eq!(release_at, 10_000 + HOUR);
    // At release time the app drops both disappearing references…
    release_object_references(&refs, &ns(), &obj("vanish-1")).unwrap();
    release_object_references(&refs, &ns(), &obj("vanish-2")).unwrap();
    // …and GC then acts on the blobs only if nothing else holds them.
    run(&store, &refs, release_at);
    let r = run(&store, &refs, release_at + HOUR);
    assert_eq!(r.deleted, 1);
    assert!(!store.contains_sync(plain));
    assert!(
        store.contains_sync(saved),
        "the user's own reference keeps it"
    );
}

#[test]
fn a_drifted_cached_count_is_repaired_from_the_real_references() {
    let (_r, store, refs) = setup();
    let id = put(&store, &fixture(1_000, 8));
    refs.apply(&[
        RefOp::Add(reference("m1", id, BlobRole::MessageAttachment)),
        RefOp::Add(reference("m2", id, BlobRole::MessageAttachment)),
    ])
    .unwrap();
    // The cache is wrong in both directions over time; make it wrong now.
    let mut row = store.metadata().get(id).unwrap().unwrap();
    row.reference_count = 17;
    store.metadata().put(row).unwrap();

    let r = run(&store, &refs, 0);
    assert_eq!(r.counts_repaired, 1);
    assert_eq!(
        store.metadata().get(id).unwrap().unwrap().reference_count,
        2
    );
    assert_eq!(
        run(&store, &refs, 1).counts_repaired,
        0,
        "stable once repaired"
    );
}

#[test]
fn pinned_active_cache_and_relay_blobs_are_never_collected() {
    let (_r, store, refs) = setup();
    let ids: Vec<BlobId> = (10..14u8)
        .map(|s| put(&store, &fixture(1_000, s)))
        .collect();
    let (pinned_id, active_id, cache_id, relay_id) = (ids[0], ids[1], ids[2], ids[3]);
    for (id, class) in [
        (cache_id, BlobClassification::Cache),
        (relay_id, BlobClassification::Relay),
    ] {
        let mut row = store.metadata().get(id).unwrap().unwrap();
        row.storage_class = class;
        store.metadata().put(row).unwrap();
    }
    let pinned: HashSet<_> = [pinned_id].into();
    let active: HashSet<_> = [active_id].into();

    let go = |now| sweep(&store, &refs, &pinned, &active, now, &policy(), 100).unwrap();
    go(0);
    let r = go(100 * HOUR);
    assert_eq!(r.deleted, 0);
    assert_eq!(
        (r.protected_pinned, r.protected_active, r.skipped_by_class),
        (1, 1, 2)
    );
    for id in &ids {
        assert!(store.contains_sync(*id));
    }
    // Protection also clears any half-run clock, so unpinning later
    // starts a fresh grace period rather than collecting immediately.
    assert_eq!(refs.unreferenced_since(pinned_id).unwrap(), None);
}

#[test]
fn a_zero_grace_policy_deletes_in_the_same_pass() {
    let (_r, store, refs) = setup();
    let id = put(&store, &fixture(1_000, 20));
    let r = sweep(
        &store,
        &refs,
        &HashSet::new(),
        &HashSet::new(),
        5,
        &GcPolicy {
            grace_period_millis: 0,
        },
        10,
    )
    .unwrap();
    assert_eq!(r.deleted, 1);
    assert!(!store.contains_sync(id));
}

#[test]
fn paging_does_not_change_the_outcome() {
    let go = |page: usize| {
        let (_r, store, refs) = setup();
        let ids: Vec<BlobId> = (30..40u8)
            .map(|s| put(&store, &fixture(1_000, s)))
            .collect();
        // Reference the even-indexed ones.
        for (i, id) in ids.iter().enumerate() {
            if i % 2 == 0 {
                refs.apply(&[RefOp::Add(reference(
                    &format!("m{i}"),
                    *id,
                    BlobRole::MessageAttachment,
                ))])
                .unwrap();
            }
        }
        sweep(
            &store,
            &refs,
            &HashSet::new(),
            &HashSet::new(),
            0,
            &policy(),
            page,
        )
        .unwrap();
        let r = sweep(
            &store,
            &refs,
            &HashSet::new(),
            &HashSet::new(),
            HOUR,
            &policy(),
            page,
        )
        .unwrap();
        (
            r.scanned,
            r.referenced,
            r.deleted,
            store.metadata().count().unwrap(),
        )
    };
    assert_eq!(go(1), go(1000));
    assert_eq!(go(3), (10, 5, 5, 5));
}

#[test]
fn references_to_blobs_not_yet_downloaded_are_reported_not_treated_as_errors() {
    let (_r, store, refs) = setup();
    let not_here = BlobId::from_ciphertext(b"still downloading");
    refs.apply(&[RefOp::Add(reference(
        "m1",
        not_here,
        BlobRole::MessageAttachment,
    ))])
    .unwrap();
    let r = run(&store, &refs, 0);
    assert_eq!(r.references_without_blob, 1);
}

// ------------------------------------------------------------ staging

fn stage(store: &FsBlobStore<InMemoryBlobMetadata>, f: &Fx, chunks: usize) {
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    for c in f.manifest.chunks.iter().take(chunks) {
        w.write_chunk_sync(
            c.index,
            &f.bytes[c.offset as usize..(c.offset + c.size as u64) as usize],
        )
        .unwrap();
    }
    // dropped without finalize: a crash / interrupted transfer
}

fn age_staging(root: &Root, id: BlobId, hours_old: u64) {
    let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let t = SystemTime::now() - Duration::from_secs(hours_old * 3600);
    for ext in ["part", "meta"] {
        let p = root.0.join("staging").join(format!("{hex}.{ext}"));
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
            f.set_modified(t).unwrap();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn partial_blobs_are_kept_for_resume_until_their_transfer_expires_and_orphans_after_grace() {
    let (root, store, _refs) = setup();
    let resumable_live = fixture(4_000, 50);
    let resumable_expired = fixture(4_000, 51);
    let orphan_old = fixture(4_000, 52);
    let orphan_young = fixture(4_000, 53);
    let active = fixture(4_000, 54);
    for (f, h) in [
        (&resumable_live, 5u64),
        (&resumable_expired, 5),
        (&orphan_old, 30),
        (&orphan_young, 1),
    ] {
        stage(&store, f, 2);
        age_staging(&root, f.descriptor.blob_id, h);
    }
    // A writer that is open RIGHT NOW (and so must never be touched),
    // even though its files look ancient.
    let mut live_writer = store
        .begin_write_with_manifest(active.descriptor, Some(active.manifest.clone()))
        .unwrap();
    live_writer
        .write_chunk_sync(0, &active.bytes[..1000])
        .unwrap();
    age_staging(&root, active.descriptor.blob_id, 500);

    let now = now_ms();
    let resumable: HashMap<BlobId, TransferExpiry> = [
        (
            resumable_live.descriptor.blob_id,
            TransferExpiry(now + 10 * HOUR),
        ),
        (
            resumable_expired.descriptor.blob_id,
            TransferExpiry(now - 1),
        ),
    ]
    .into();
    let r = sweep_staging(&store, &resumable, now, &PartialRetentionPolicy::default()).unwrap();
    assert_eq!(r.kept_active, 1);
    assert_eq!(r.kept_for_resume, 1);
    assert_eq!(r.removed_expired_transfer, 1);
    assert_eq!(r.removed_orphan, 1, "30h-old orphan, past the 24h grace");
    assert_eq!(
        r.kept_orphan_in_grace, 1,
        "1h-old orphan is still within grace"
    );
    assert!(r.bytes_freed >= 8_000);

    let remaining: HashSet<BlobId> = store
        .list_staging()
        .unwrap()
        .into_iter()
        .map(|s| s.blob_id)
        .collect();
    assert!(remaining.contains(&resumable_live.descriptor.blob_id));
    assert!(remaining.contains(&orphan_young.descriptor.blob_id));
    assert!(remaining.contains(&active.descriptor.blob_id));
    assert!(!remaining.contains(&resumable_expired.descriptor.blob_id));
    assert!(!remaining.contains(&orphan_old.descriptor.blob_id));

    // The kept-for-resume transfer really can resume.
    drop(live_writer);
    let w = store
        .begin_write_with_manifest(
            resumable_live.descriptor,
            Some(resumable_live.manifest.clone()),
        )
        .unwrap();
    assert_eq!(w.received_chunks(), 2);
}

#[test]
fn a_staging_with_an_open_writer_cannot_be_discarded() {
    let (_root, store, _refs) = setup();
    let f = fixture(2_000, 60);
    let w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    assert!(store.discard_staging(f.descriptor.blob_id).is_err());
    drop(w);
    assert!(store.discard_staging(f.descriptor.blob_id).is_ok());
    assert_eq!(
        store.discard_staging(f.descriptor.blob_id).unwrap(),
        0,
        "already gone: nothing to free"
    );
}

#[test]
fn storage_pressure_reclaims_orphans_first_then_resumable_oldest_first_and_stops_when_enough() {
    let (root, store, _refs) = setup();
    let orphan_old = fixture(5_000, 70);
    let orphan_new = fixture(5_000, 71);
    let resumable_old = fixture(5_000, 72);
    let resumable_new = fixture(5_000, 73);
    for (f, h) in [
        (&orphan_old, 40u64),
        (&orphan_new, 2),
        (&resumable_old, 30),
        (&resumable_new, 3),
    ] {
        stage(&store, f, 1);
        age_staging(&root, f.descriptor.blob_id, h);
    }
    let now = now_ms();
    let resumable: HashMap<BlobId, TransferExpiry> = [
        (
            resumable_old.descriptor.blob_id,
            TransferExpiry(now + 100 * HOUR),
        ),
        (
            resumable_new.descriptor.blob_id,
            TransferExpiry(now + 100 * HOUR),
        ),
    ]
    .into();

    // Need 7 000 bytes: the two orphans (5 000 each) cover it; no
    // resumable transfer should be sacrificed.
    let (n, freed) = reclaim_staging(&store, &resumable, 7_000).unwrap();
    assert_eq!((n, freed), (2, 10_000));
    let left: HashSet<BlobId> = store
        .list_staging()
        .unwrap()
        .into_iter()
        .map(|s| s.blob_id)
        .collect();
    assert_eq!(
        left,
        [
            resumable_old.descriptor.blob_id,
            resumable_new.descriptor.blob_id
        ]
        .into()
    );

    // More pressure now takes the OLDEST resumable first, and only one.
    let (n, _) = reclaim_staging(&store, &resumable, 1).unwrap();
    assert_eq!(n, 1);
    let left: HashSet<BlobId> = store
        .list_staging()
        .unwrap()
        .into_iter()
        .map(|s| s.blob_id)
        .collect();
    assert_eq!(left, [resumable_new.descriptor.blob_id].into());

    // Nothing needed: nothing removed.
    assert_eq!(reclaim_staging(&store, &resumable, 0).unwrap(), (0, 0));
}
