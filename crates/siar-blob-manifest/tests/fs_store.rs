//! The local blob store on a real filesystem: staging, per-chunk and
//! whole-blob verification, atomic publication, quotas, crash recovery
//! and the bounded streaming path — with real files in real temp
//! directories, real hashes, and "crashes" that drop every in-memory
//! object and reopen the directory.

use bytes::Bytes;
use proptest::prelude::*;
use siar_blob_manifest::blob_io::{BlobReader, BlobStore, BlobWriter, MAX_READ_RANGE_BYTES};
use siar_blob_manifest::bounded_io::stream_chunks;
use siar_blob_manifest::buffer_pool::BufferPool;
use siar_blob_manifest::byte_range::ByteRange;
use siar_blob_manifest::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use siar_blob_manifest::fs_store::{DurabilityPolicy, FsBlobStore, FsStoreConfig, Preallocation};
use siar_blob_manifest::ids::{BlobId, ChunkHash};
use siar_blob_manifest::limits::ManifestLimits;
use siar_blob_manifest::manifest::{build_manifest, BlobManifest, ChunkDescriptor};
use siar_blob_manifest::metadata_store::{BlobMetadataStore, InMemoryBlobMetadata};
use siar_blob_manifest::quota::StoreLimits;
use siar_blob_manifest::schema_rows::BlobLifecycleState;
use siar_blob_manifest::store_error::{BlobError, Recoverability};
use siar_domain::BlobSize;

struct Root(std::path::PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("siar-blobstore-{}", uuid::Uuid::new_v4())))
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic, incompressible-looking bytes.
fn data(len: usize, seed: u8) -> Vec<u8> {
    // Distinct for every seed (`seed | 1` would collide for pairs).
    let mut x = (seed as u32)
        .wrapping_mul(2_654_435_761)
        .wrapping_add(0x9E37_79B9);
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect()
}

struct Fixture {
    bytes: Vec<u8>,
    descriptor: BlobDescriptor,
    manifest: BlobManifest,
}

fn fixture(len: usize, chunk: u32, seed: u8) -> Fixture {
    let bytes = data(len, seed);
    let manifest = build_manifest(&bytes, chunk, &ManifestLimits::default()).unwrap();
    let descriptor = BlobDescriptor {
        blob_id: BlobId::from_ciphertext(&bytes),
        size: BlobSize::parse(len as u64).unwrap(),
        chunking: ChunkingDescriptor { chunk_size: chunk },
        encryption: EncryptionDescriptor {
            algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
            nonce: [0; 12],
        },
        media_type: None,
        manifest_id: None,
    };
    Fixture {
        bytes,
        descriptor,
        manifest,
    }
}

fn cfg() -> FsStoreConfig {
    FsStoreConfig {
        durability: DurabilityPolicy::None, // speed; the policy is tested separately
        ..FsStoreConfig::default()
    }
}

fn open(root: &Root) -> FsBlobStore<InMemoryBlobMetadata> {
    FsBlobStore::open(root.path(), cfg(), InMemoryBlobMetadata::new()).unwrap()
}

fn chunk_of(f: &Fixture, i: u32) -> &[u8] {
    let c = &f.manifest.chunks[i as usize];
    &f.bytes[c.offset as usize..c.offset as usize + c.size as usize]
}

fn write_all_chunks(w: &mut impl BlobWriterSync, f: &Fixture, order: &[u32]) {
    for &i in order {
        w.put(i, chunk_of(f, i)).unwrap();
    }
}

/// Lets helpers drive the concrete writer without `async`.
trait BlobWriterSync {
    fn put(&mut self, i: u32, d: &[u8]) -> Result<(), BlobError>;
}
impl BlobWriterSync for siar_blob_manifest::fs_store::FsBlobWriter<InMemoryBlobMetadata> {
    fn put(&mut self, i: u32, d: &[u8]) -> Result<(), BlobError> {
        self.write_chunk_sync(i, d)
    }
}

fn object_file(root: &Root, id: BlobId) -> std::path::PathBuf {
    let h: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    root.path().join("objects").join(&h[..2]).join(h)
}

fn staging_files(root: &Root) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root.path().join("staging"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

#[test]
fn a_blob_written_out_of_order_is_verified_published_and_readable() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(10_000, 1_024, 1); // 10 chunks, last one short
    assert_eq!(f.manifest.chunks.len(), 10);

    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[9, 3, 0, 7, 1, 8, 2, 6, 4, 5]);
    assert!(w.missing_ranges().is_empty());
    let id = w.finalize_sync().unwrap();
    assert_eq!(id, f.descriptor.blob_id);

    assert!(store.contains_sync(id));
    assert!(object_file(&root, id).is_file());
    assert!(
        staging_files(&root).is_empty(),
        "staging cleaned after publication"
    );
    let stat = store.stat_sync(id).unwrap();
    assert_eq!(
        (stat.size, stat.state),
        (10_000, BlobLifecycleState::Verified)
    );
    assert_eq!(store.ledger().used_bytes(), 10_000);
    assert_eq!(
        (
            store.ledger().reserved_bytes(),
            store.ledger().active_writes()
        ),
        (0, 0)
    );
    assert!(store.metadata().get(id).unwrap().is_some());

    let r = store.open_sync(id).unwrap();
    assert_eq!(r.size(), 10_000);
    assert_eq!(
        &r.read_range_sync(ByteRange::new(2_000, 3_000)).unwrap()[..],
        &f.bytes[2_000..5_000]
    );
    let streamed: Vec<u8> = r
        .stream_sync(4_096)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect();
    assert_eq!(streamed, f.bytes);
    assert!(store.verify_object(id).unwrap());
}

#[test]
fn nothing_is_visible_to_readers_until_finalize_succeeds() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(5_000, 1_000, 2);
    let id = f.descriptor.blob_id;

    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1, 2, 3]); // chunk 4 missing
    assert!(!store.contains_sync(id), "a partial write is not a blob");
    assert!(matches!(store.open_sync(id), Err(BlobError::NotFound(_))));
    assert!(!object_file(&root, id).exists());
    assert_eq!(w.missing_ranges(), vec![(4, 5)]);

    let err = w.finalize_sync().unwrap_err();
    assert_eq!(err, BlobError::Incomplete { missing_chunks: 1 });
    assert!(!store.contains_sync(id));
    assert_eq!(err.recoverability(), Recoverability::CallerError);
}

#[test]
fn each_chunk_is_checked_before_it_touches_storage() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(4_000, 1_000, 3);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();

    let mut bad = chunk_of(&f, 1).to_vec();
    bad[0] ^= 1;
    assert_eq!(
        w.put(1, &bad),
        Err(BlobError::ChunkHashMismatch { index: 1 })
    );
    assert_eq!(
        w.put(1, &bad[..999]),
        Err(BlobError::ChunkSizeMismatch {
            index: 1,
            expected: 1000,
            actual: 999
        })
    );
    assert_eq!(
        w.put(4, &[0; 1000]),
        Err(BlobError::ChunkIndexOutOfRange {
            index: 4,
            chunk_count: 4
        })
    );
    assert_eq!(w.received_chunks(), 0, "rejected chunks left no trace");

    w.put(1, chunk_of(&f, 1)).unwrap();
    w.put(1, chunk_of(&f, 1)).unwrap(); // idempotent
    assert_eq!(w.received_chunks(), 1);
    assert_eq!(
        BlobError::ChunkHashMismatch { index: 0 }.recoverability(),
        Recoverability::RefetchData
    );
}

#[test]
fn a_blob_that_does_not_match_its_declared_identity_is_quarantined_never_published() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(3_000, 1_000, 4);
    // Declare f's id but deliver different bytes (no manifest, so the
    // per-chunk check can't catch it — only the whole-blob hash can).
    let impostor = data(3_000, 99);
    let mut w = store.begin_write_with_manifest(f.descriptor, None).unwrap();
    for i in 0..3u32 {
        w.write_chunk_sync(i, &impostor[i as usize * 1000..(i as usize + 1) * 1000])
            .unwrap();
    }
    let err = w.finalize_sync().unwrap_err();
    assert_eq!(
        err,
        BlobError::BlobHashMismatch {
            declared: f.descriptor.blob_id
        }
    );

    assert!(!store.contains_sync(f.descriptor.blob_id));
    assert!(!object_file(&root, f.descriptor.blob_id).exists());
    assert!(staging_files(&root).is_empty());
    let quarantined: Vec<_> = std::fs::read_dir(root.path().join("quarantine"))
        .unwrap()
        .collect();
    assert_eq!(quarantined.len(), 1, "kept for diagnostics, not exposed");
    assert_eq!(store.ledger().used_bytes(), 0);
    assert_eq!(store.ledger().reserved_bytes(), 0, "reservation released");
}

#[test]
fn a_transfer_that_cannot_fit_is_refused_before_any_bytes_arrive() {
    let root = Root::new();
    let small = FsStoreConfig {
        limits: StoreLimits {
            max_total_bytes: 10_000,
            max_blob_bytes: 10_000,
            max_concurrent_writes: 2,
        },
        ..cfg()
    };
    let store = FsBlobStore::open(root.path(), small, InMemoryBlobMetadata::new()).unwrap();
    let a = fixture(7_000, 1_000, 5);
    let b = fixture(5_000, 1_000, 6);
    let _wa = store
        .begin_write_with_manifest(a.descriptor, Some(a.manifest.clone()))
        .unwrap();
    let err = store
        .begin_write_with_manifest(b.descriptor, Some(b.manifest.clone()))
        .err()
        .unwrap();
    assert_eq!(
        err,
        BlobError::ReservationDenied {
            requested: 5_000,
            available: 3_000
        }
    );
    assert_eq!(err.recoverability(), Recoverability::PolicyDenied);
    // Nothing was created for the refused transfer.
    assert!(!staging_files(&root)
        .iter()
        .any(|n| n.contains(&hex(b.descriptor.blob_id))));
}

fn hex(id: BlobId) -> String {
    id.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn a_second_writer_for_the_same_blob_is_refused_until_the_first_lets_go() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(2_000, 1_000, 7);
    let w1 = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    assert_eq!(
        store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .err()
            .unwrap(),
        BlobError::AlreadyStaging(f.descriptor.blob_id)
    );
    drop(w1);
    assert!(store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .is_ok());
}

#[test]
fn crash_recovery_re_verifies_staging_and_resumes_only_what_is_actually_good() {
    let root = Root::new();
    let f = fixture(8_000, 1_000, 8); // 8 chunks
    {
        let store = open(&root);
        let mut w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        write_all_chunks(&mut w, &f, &[0, 1, 2, 3, 4, 5]); // 6 of 8 written
                                                           // -- process dies: no finalize, no abort --
        drop(w);
    }
    // Damage one of the *claimed-good* chunks on disk (a torn write /
    // bit rot) — recovery must not believe the file.
    let part = root
        .path()
        .join("staging")
        .join(format!("{}.part", hex(f.descriptor.blob_id)));
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
        file.seek(SeekFrom::Start(2_500)).unwrap(); // inside chunk 2
        file.write_all(&[0xFF; 8]).unwrap();
    }

    let store = open(&root); // fresh process
    let report = store.recover().unwrap();
    assert_eq!(report.resumable.len(), 1);
    assert_eq!(
        report.resumable[0].verified_chunks, 5,
        "6 written, 1 corrupted -> 5 trusted"
    );
    assert_eq!(report.resumable[0].total_chunks, 8);

    // Resume: the 5 verified chunks are already counted; fetch the rest.
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    assert_eq!(w.received_chunks(), 5);
    assert_eq!(w.missing_ranges(), vec![(2, 3), (6, 8)]);
    write_all_chunks(&mut w, &f, &[2, 6, 7]);
    let id = w.finalize_sync().unwrap();
    assert_eq!(id, f.descriptor.blob_id);
    let r = store.open_sync(id).unwrap();
    let all: Vec<u8> = r
        .stream_sync(1_000)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect();
    assert_eq!(all, f.bytes, "the repaired blob is byte-identical");
}

#[test]
fn recovery_distrusts_what_it_cannot_verify_and_cleans_up_after_itself() {
    let root = Root::new();
    let f = fixture(2_000, 1_000, 9);
    let g = fixture(2_000, 1_000, 10);
    {
        let store = open(&root);
        // (1) manifest-less staging: nothing verifiable.
        let mut w1 = store.begin_write_with_manifest(f.descriptor, None).unwrap();
        w1.write_chunk_sync(0, chunk_of(&f, 0)).unwrap();
        drop(w1);
        // (2) a finished blob whose staging sidecar was left behind
        // (crash after the rename).
        let mut w2 = store
            .begin_write_with_manifest(g.descriptor, Some(g.manifest.clone()))
            .unwrap();
        write_all_chunks(&mut w2, &g, &[0, 1]);
        w2.finalize_sync().unwrap();
        std::fs::write(
            root.path()
                .join("staging")
                .join(format!("{}.meta", hex(g.descriptor.blob_id))),
            // a sidecar that parses is needed; reuse a real one:
            std::fs::read(root.path().join("staging").join("nonexistent")).unwrap_or_default(),
        )
        .unwrap();
    }
    // (3) orphan .part, (4) junk file in staging, (5) temp debris.
    std::fs::write(root.path().join("staging").join("deadbeef.part"), b"orphan").unwrap();
    std::fs::write(root.path().join("staging").join("junk.txt"), b"junk").unwrap();
    std::fs::write(root.path().join("temp").join("scratch.tmp"), b"x").unwrap();

    let store = open(&root);
    let r = store.recover().unwrap();
    assert!(r.resumable.is_empty());
    assert!(r.discarded_unverifiable >= 1, "{r:?}");
    assert!(r.discarded_orphans >= 2, "{r:?}");
    assert_eq!(r.temp_files_removed, 1);
    assert!(
        staging_files(&root).is_empty(),
        "everything untrusted is gone: {:?}",
        staging_files(&root)
    );
    assert!(
        store.contains_sync(g.descriptor.blob_id),
        "the finished blob is untouched"
    );
    // Idempotent.
    assert_eq!(store.recover().unwrap(), Default::default());
}

#[test]
fn rewriting_a_blob_already_stored_is_a_no_op_not_a_second_copy() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(3_000, 1_000, 11);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1, 2]);
    w.finalize_sync().unwrap();
    let used = store.ledger().used_bytes();

    let mut again = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    assert!(again.was_already_present());
    assert!(again.missing_ranges().is_empty());
    // Whatever a (possibly hostile) sender pushes at a dedup'd blob is
    // ignored — the stored object is immutable.
    again.write_chunk_sync(0, &[0xEE; 1000]).unwrap();
    assert_eq!(again.finalize_sync().unwrap(), f.descriptor.blob_id);
    assert_eq!(store.ledger().used_bytes(), used);
    assert!(store.verify_object(f.descriptor.blob_id).unwrap());
}

#[test]
fn delete_frees_the_space_and_is_idempotent() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(4_000, 1_000, 12);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1, 2, 3]);
    let id = w.finalize_sync().unwrap();
    assert_eq!(store.ledger().used_bytes(), 4_000);

    store.delete_sync(id).unwrap();
    assert!(!store.contains_sync(id));
    assert_eq!(store.ledger().used_bytes(), 0);
    assert!(store.metadata().get(id).unwrap().is_none());
    store.delete_sync(id).unwrap(); // already gone: fine
    assert_eq!(store.ledger().used_bytes(), 0, "no double-release");
}

#[test]
fn usage_is_rebuilt_from_disk_when_the_store_reopens() {
    let root = Root::new();
    let f = fixture(6_000, 1_000, 13);
    {
        let store = open(&root);
        let mut w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        write_all_chunks(&mut w, &f, &[0, 1, 2, 3, 4, 5]);
        w.finalize_sync().unwrap();
    }
    let store = open(&root);
    assert_eq!(
        store.ledger().used_bytes(),
        6_000,
        "quota is not reset by a restart"
    );
}

#[test]
fn metadata_is_reconciled_against_the_files_which_are_the_truth() {
    let root = Root::new();
    let store = open(&root);
    let (a, b, c) = (
        fixture(1_000, 500, 14),
        fixture(1_000, 500, 15),
        fixture(1_000, 500, 16),
    );
    for f in [&a, &b, &c] {
        let mut w = store
            .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
            .unwrap();
        write_all_chunks(&mut w, f, &[0, 1]);
        w.finalize_sync().unwrap();
    }
    // Metadata loses a row; a stale row exists for a vanished file; one
    // object is silently corrupted on disk.
    store.metadata().remove(a.descriptor.blob_id).unwrap();
    std::fs::remove_file(object_file(&root, b.descriptor.blob_id)).unwrap();
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(object_file(&root, c.descriptor.blob_id))
            .unwrap();
        f.seek(SeekFrom::Start(10)).unwrap();
        f.write_all(&[0xAB]).unwrap();
    }
    store.metadata().remove(c.descriptor.blob_id).unwrap(); // so it must be re-verified

    let r = store.reconcile_metadata().unwrap();
    assert_eq!(r.rows_added, 1, "a's row rebuilt after re-hashing its file");
    assert_eq!(r.rows_removed, 1, "b's row removed: no file");
    assert_eq!(
        r.corrupt_quarantined, 1,
        "c failed its hash and was quarantined"
    );
    assert!(store
        .metadata()
        .get(a.descriptor.blob_id)
        .unwrap()
        .is_some());
    assert!(store
        .metadata()
        .get(b.descriptor.blob_id)
        .unwrap()
        .is_none());
    assert!(!object_file(&root, c.descriptor.blob_id).exists());
    assert!(!store
        .verify_object(a.descriptor.blob_id)
        .map(|v| !v)
        .unwrap());
}

#[test]
fn reads_are_bounded_and_validated() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(5_000, 1_000, 17);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1, 2, 3, 4]);
    let id = w.finalize_sync().unwrap();
    let r = store.open_sync(id).unwrap();

    assert!(matches!(
        r.read_range_sync(ByteRange::new(4_900, 200)),
        Err(BlobError::RangeOutOfBounds { .. })
    ));
    assert!(matches!(
        r.read_range_sync(ByteRange::new(u64::MAX, 5)),
        Err(BlobError::RangeOutOfBounds { .. })
    ));
    assert_eq!(
        r.read_range_sync(ByteRange::new(5_000, 0)).unwrap().len(),
        0
    );
    assert!(r.stream_sync(0).is_err());
    assert!(r.stream_sync(usize::MAX).is_err());
    const _: () = assert!(MAX_READ_RANGE_BYTES >= 1024 * 1024);
    // Each streamed piece respects the requested bound.
    for piece in r.stream_sync(700).unwrap() {
        assert!(piece.unwrap().len() <= 700);
    }
}

#[test]
fn a_manifest_that_disagrees_with_its_descriptor_is_rejected_up_front() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(4_000, 1_000, 18);
    let other = fixture(4_000, 1_000, 19);

    // Different blob id.
    assert!(matches!(
        store.begin_write_with_manifest(f.descriptor, Some(other.manifest.clone())),
        Err(BlobError::InvalidManifest(_))
    ));
    // Tampered chunk layout.
    let mut m = f.manifest.clone();
    m.chunks[1] = ChunkDescriptor {
        offset: 999,
        ..m.chunks[1]
    };
    assert!(matches!(
        store.begin_write_with_manifest(f.descriptor, Some(m)),
        Err(BlobError::InvalidManifest(_))
    ));
    // Wrong chunk count.
    let mut m = f.manifest.clone();
    m.chunks.pop();
    assert!(matches!(
        store.begin_write_with_manifest(f.descriptor, Some(m)),
        Err(BlobError::InvalidManifest(_))
    ));
    // None of that reserved space or left files.
    assert_eq!(store.ledger().reserved_bytes(), 0);
    assert!(staging_files(&root).is_empty());
}

#[cfg(unix)]
#[test]
fn directories_and_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let root = Root::new();
    let store = open(&root);
    let f = fixture(2_000, 1_000, 20);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1]);
    let id = w.finalize_sync().unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(root.path()), 0o700);
    for d in ["objects", "staging", "temp", "quarantine"] {
        assert_eq!(mode(&root.path().join(d)), 0o700, "{d}");
    }
    let obj = object_file(&root, id);
    assert_eq!(mode(obj.parent().unwrap()), 0o700);
    assert_eq!(mode(&obj), 0o600);
}

#[cfg(unix)]
#[test]
fn preallocation_is_sparse_where_the_filesystem_supports_it() {
    use std::os::unix::fs::MetadataExt;
    let root = Root::new();
    let store = open(&root);
    let size = 16 * 1024 * 1024;
    let f = fixture(size, 1024 * 1024, 21);
    let _w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    let part = root
        .path()
        .join("staging")
        .join(format!("{}.part", hex(f.descriptor.blob_id)));
    let md = std::fs::metadata(&part).unwrap();
    assert_eq!(md.len(), size as u64, "logical size is reserved up front");
    let allocated = md.blocks() * 512;
    if allocated >= size as u64 {
        eprintln!("filesystem does not support sparse files; allocation check skipped");
    } else {
        assert!(
            allocated < size as u64 / 4,
            "sparse: {allocated} bytes actually allocated"
        );
    }
}

#[test]
fn preallocation_none_still_produces_the_same_verified_blob() {
    let root = Root::new();
    let c = FsStoreConfig {
        preallocation: Preallocation::None,
        ..cfg()
    };
    let store = FsBlobStore::open(root.path(), c, InMemoryBlobMetadata::new()).unwrap();
    let f = fixture(5_000, 1_000, 22);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[4, 2, 0, 1, 3]);
    assert_eq!(w.finalize_sync().unwrap(), f.descriptor.blob_id);
}

#[test]
fn the_fsync_policy_path_works_end_to_end() {
    let root = Root::new();
    let store = FsBlobStore::open(
        root.path(),
        FsStoreConfig::default(),
        InMemoryBlobMetadata::new(),
    )
    .unwrap();
    let f = fixture(3_000, 1_000, 23);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0, 1, 2]);
    assert_eq!(w.finalize_sync().unwrap(), f.descriptor.blob_id);
}

#[test]
fn abort_discards_staging_and_frees_the_reservation() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(3_000, 1_000, 24);
    let mut w = store
        .begin_write_with_manifest(f.descriptor, Some(f.manifest.clone()))
        .unwrap();
    write_all_chunks(&mut w, &f, &[0]);
    w.abort_sync().unwrap();
    assert!(staging_files(&root).is_empty());
    assert_eq!(store.ledger().reserved_bytes(), 0);
    assert_eq!(store.ledger().active_writes(), 0);
}

/// §37 end to end: a "2 GB" file is never in memory — the sender reads
/// it through a bounded pool, hashing incrementally and building chunk
/// descriptors as it goes; the receiver writes chunk by chunk.
#[test]
fn a_file_is_sent_and_received_without_ever_being_buffered_whole() {
    let root = Root::new();
    let store = open(&root);
    let size = 3 * 1024 * 1024 + 12_345;
    let chunk = 256 * 1024;
    let source_bytes = data(size, 31);

    // The "file on disk" on the sender.
    let src_path = std::env::temp_dir().join(format!("siar-src-{}", uuid::Uuid::new_v4()));
    std::fs::write(&src_path, &source_bytes).unwrap();

    // Sender: stream through a pool of 4 chunk-sized buffers.
    let pool = BufferPool::new(4, chunk);
    let mut descriptors = Vec::new();
    let mut whole = blake3::Hasher::new();
    let mut chunks_for_receiver: Vec<Bytes> = Vec::new(); // stands in for "the network"
    let summary = stream_chunks(
        std::fs::File::open(&src_path).unwrap(),
        &pool,
        2,
        |i, piece| {
            whole.update(piece);
            descriptors.push(ChunkDescriptor {
                index: i as u32,
                offset: i * chunk as u64,
                size: piece.len() as u32,
                hash: ChunkHash::from_ciphertext_chunk(piece),
            });
            chunks_for_receiver.push(Bytes::copy_from_slice(piece));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(summary.bytes, size as u64);
    let blob_id = BlobId::from_ciphertext_incremental(source_bytes.chunks(chunk));
    assert_eq!(
        blob_id.as_bytes(),
        whole.finalize().as_bytes(),
        "incremental == whole-buffer identity"
    );

    let manifest = BlobManifest {
        version: 1,
        blob_id,
        total_size: size as u64,
        chunk_size: chunk as u32,
        chunks: descriptors,
    };
    let descriptor = BlobDescriptor {
        blob_id,
        size: BlobSize::parse(size as u64).unwrap(),
        chunking: ChunkingDescriptor {
            chunk_size: chunk as u32,
        },
        encryption: EncryptionDescriptor {
            algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
            nonce: [0; 12],
        },
        media_type: None,
        manifest_id: None,
    };

    // Receiver: write chunks (reversed: the network reorders).
    let mut w = store
        .begin_write_with_manifest(descriptor, Some(manifest))
        .unwrap();
    for (i, c) in chunks_for_receiver.iter().enumerate().rev() {
        w.write_chunk_sync(i as u32, c).unwrap();
    }
    let id = w.finalize_sync().unwrap();
    assert_eq!(id, blob_id);
    let got: Vec<u8> = store
        .open_sync(id)
        .unwrap()
        .stream_sync(chunk)
        .unwrap()
        .flat_map(|c| c.unwrap().to_vec())
        .collect();
    assert_eq!(got, source_bytes);
    assert_eq!(pool.available(), 4, "all sender buffers returned");
    let _ = std::fs::remove_file(src_path);
}

async fn roundtrip_through_the_traits<S: BlobStore>(store: &S, f: &Fixture) -> BlobId {
    let mut w = store.begin_write(f.descriptor).await.unwrap();
    for i in (0..f.manifest.chunks.len() as u32).rev() {
        w.write_chunk(i, Bytes::copy_from_slice(chunk_of(f, i)))
            .await
            .unwrap();
    }
    let id = w.finalize().await.unwrap();
    assert!(store.contains(id).await.unwrap());
    assert_eq!(store.stat(id).await.unwrap().size, f.bytes.len() as u64);
    let r = store.open(id).await.unwrap();
    let head = r.read_range(ByteRange::new(0, 10)).await.unwrap();
    assert_eq!(&head[..], &f.bytes[..10]);
    let n: usize = r
        .stream(1_000)
        .await
        .unwrap()
        .map(|c| c.unwrap().len())
        .sum();
    assert_eq!(n, f.bytes.len());
    store.delete(id).await.unwrap();
    assert!(!store.contains(id).await.unwrap());
    id
}

#[tokio::test]
async fn the_async_trait_surface_works_generically() {
    let root = Root::new();
    let store = open(&root);
    let f = fixture(7_777, 1_000, 40);
    assert_eq!(
        roundtrip_through_the_traits(&store, &f).await,
        f.descriptor.blob_id
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20))]
    /// Any size, any chunk size, any arrival order, with or without a
    /// manifest: the stored blob is byte-identical to the source.
    #[test]
    fn any_blob_in_any_order_round_trips(
        len in 1usize..20_000,
        chunk in 1u32..4_000,
        use_manifest in any::<bool>(),
        seed in any::<u8>(),
        shuffle in any::<u64>(),
    ) {
        let root = Root::new();
        let store = open(&root);
        let f = fixture(len, chunk, seed);
        let n = f.manifest.chunks.len() as u32;
        let mut order: Vec<u32> = (0..n).collect();
        // Deterministic Fisher-Yates from `shuffle`.
        let mut s = shuffle | 1;
        for i in (1..order.len()).rev() {
            s ^= s << 13; s ^= s >> 7; s ^= s << 17;
            order.swap(i, (s % (i as u64 + 1)) as usize);
        }
        let mut w = store
            .begin_write_with_manifest(f.descriptor, use_manifest.then(|| f.manifest.clone()))
            .unwrap();
        write_all_chunks(&mut w, &f, &order);
        let id = w.finalize_sync().unwrap();
        let got: Vec<u8> = store.open_sync(id).unwrap().stream_sync(3_000).unwrap()
            .flat_map(|c| c.unwrap().to_vec()).collect();
        prop_assert_eq!(got, f.bytes);
        prop_assert_eq!(store.ledger().used_bytes(), len as u64);
    }
}
