//! §50 "Temporary Staging", §51 "Atomic Finalization", §52 "Crash
//! Recovery", §49 "Sparse Files", §90 "Directory Layout", §48's
//! enforcement point, and the filesystem implementation of §86-88's
//! traits.
//!
//! ## Layout (§90)
//!
//! ```text
//! blob-root/
//! ├── objects/ab/<64-hex blob id>     finished, verified, immutable
//! ├── staging/<hex>.part              incomplete bytes (sparse)
//! ├── staging/<hex>.meta              descriptor (+ manifest) to resume
//! ├── temp/                           scratch; wiped on recovery
//! └── quarantine/                     objects that failed verification
//! ```
//!
//! Names come **only** from the blob id's hex — a hash the writer
//! declared, 64 characters of `[0-9a-f]` — never from a filename or any
//! other peer-supplied string (§53/§54). Prefix sharding (`ab/`) keeps
//! directories small.
//!
//! ## The write path (§50, §51)
//!
//! ```text
//! begin_write ── reserve space (§48) ─▶ staging/<id>.part (sparse, set_len)
//! write_chunk ── check size, check hash (if manifest) ─▶ pwrite at offset
//! finalize    ── every chunk present?
//!             ── fsync (policy) ── stream-hash the whole file (bounded buffer)
//!             ── hash == declared id?  no ─▶ quarantine/, error
//!             ── rename staging ▶ objects/ab/<id>   ◀── the atomic commit
//!             ── fsync directory (policy) ── metadata row ── commit reservation
//! ```
//!
//! The rename is the only moment a blob becomes visible, and it happens
//! *after* full verification. A reader can therefore never open an
//! incomplete or unverified object: until the rename there is nothing at
//! `objects/ab/<id>`; after it, there is only a verified file. "Never
//! expose incomplete object as complete."
//!
//! ## Crash recovery (§52) — never trust a temp file
//!
//! [`FsBlobStore::recover`] runs at startup. For every `staging/*.meta`
//! it does **not** believe what the `.part` claims. If the sidecar
//! carries a manifest, every chunk in the `.part` is re-hashed against
//! the manifest's per-chunk hash; only chunks that verify count as
//! received, so a torn write, a bit flip or a never-written hole in the
//! sparse file all show up as "missing" and get re-fetched. Without a
//! manifest nothing in the `.part` can be verified, so it is discarded
//! rather than trusted. A `.part` with no `.meta` is an orphan and is
//! deleted; `temp/` is wiped. A crash *after* the rename but before the
//! staging sidecar was removed is recognised (the object exists) and the
//! staging leftovers are cleaned up.
//!
//! No chunk bitmap is persisted: with a manifest the file itself is the
//! ground truth and re-hashing is the verification, so there is no
//! second record that could disagree with it.
//!
//! ## What blocks
//!
//! File I/O here is `std::fs`, called directly from the `async fn`
//! trait methods. That is a deliberate simplification of this crate
//! (which has no async-runtime dependency): on a multi-threaded runtime
//! call the store from a blocking-capable context (e.g.
//! `spawn_blocking`). The synchronous inherent methods below are the
//! same code without the `async` wrapper.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::blob_io::{
    BlobReader, BlobStat, BlobStore, BlobStream, BlobWriter, MAX_READ_RANGE_BYTES,
};
use crate::byte_range::ByteRange;
use crate::descriptor::BlobDescriptor;
use crate::ids::{BlobId, ChunkHash};
use crate::limits::ManifestLimits;
use crate::manifest::BlobManifest;
use crate::metadata_store::BlobMetadataStore;
use crate::quota::{QuotaLedger, Reservation, StoreLimits};
use crate::resume::ResumeBitmap;
use crate::schema_rows::{BlobLifecycleState, BlobRow, FilesystemPermissionPolicy};
use crate::storage_lifecycle::BlobClassification;
use crate::store_error::{io, BlobError};

const HASH_BUF_BYTES: usize = 64 * 1024;
const STAGING_META_VERSION: u16 = 1;

/// §51's "fsync as policy requires".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityPolicy {
    /// fsync the data file and the containing directory before
    /// declaring a blob complete. Survives power loss.
    Fsync,
    /// Rely on the OS. Survives a process crash, not necessarily a
    /// power cut. For caches and tests.
    None,
}

/// §49: "Not mandatory for correctness."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preallocation {
    /// `set_len` to the final size up front: a sparse file on
    /// filesystems that support holes (no disk consumed until chunks
    /// land), and out-of-order chunk writes are plain positional writes.
    SparseSetLen,
    /// Let the file grow as chunks are written.
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct FsStoreConfig {
    pub limits: StoreLimits,
    pub durability: DurabilityPolicy,
    pub preallocation: Preallocation,
    pub permissions: FilesystemPermissionPolicy,
    /// Classification new blobs get; policy can change it later.
    pub default_class: BlobClassification,
    pub now_millis: fn() -> u64,
}

fn system_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for FsStoreConfig {
    fn default() -> Self {
        Self {
            limits: StoreLimits::default(),
            durability: DurabilityPolicy::Fsync,
            preallocation: Preallocation::SparseSetLen,
            permissions: FilesystemPermissionPolicy::default(),
            default_class: BlobClassification::Durable,
            now_millis: system_now_millis,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn parse_id_hex(name: &str) -> Option<BlobId> {
    if name.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(name.get(i * 2..i * 2 + 2)?, 16).ok()?;
        // Reject uppercase/odd forms: ids are canonical lowercase.
        if !name.as_bytes()[i * 2..i * 2 + 2]
            .iter()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c))
        {
            return None;
        }
    }
    Some(BlobId::from_raw_digest(out))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), BlobError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(io("set permissions"))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), BlobError> {
    Ok(())
}

/// Geometry derived from a descriptor — the only source of truth for
/// "how many chunks" and "how long is chunk *i*".
#[derive(Debug, Clone, Copy)]
struct Geometry {
    size: u64,
    chunk_size: u32,
    chunk_count: u32,
}

impl Geometry {
    fn of(d: &BlobDescriptor) -> Result<Self, BlobError> {
        let size = d.size.bytes();
        let chunk_size = d.chunking.chunk_size;
        if chunk_size == 0 {
            return Err(BlobError::InvalidArgument("chunk size must be non-zero"));
        }
        let count = size.div_ceil(u64::from(chunk_size));
        if count > u64::from(ManifestLimits::default().max_chunks) {
            return Err(BlobError::InvalidArgument(
                "blob would have too many chunks",
            ));
        }
        Ok(Self {
            size,
            chunk_size,
            chunk_count: count as u32,
        })
    }

    fn offset(&self, index: u32) -> u64 {
        u64::from(index) * u64::from(self.chunk_size)
    }

    fn len_of(&self, index: u32) -> u32 {
        let start = self.offset(index);
        (self.size - start).min(u64::from(self.chunk_size)) as u32
    }
}

fn check_manifest(g: &Geometry, d: &BlobDescriptor, m: &BlobManifest) -> Result<(), BlobError> {
    if m.blob_id != d.blob_id {
        return Err(BlobError::InvalidManifest(
            "manifest blob id differs from descriptor",
        ));
    }
    if m.total_size != g.size {
        return Err(BlobError::InvalidManifest(
            "manifest size differs from descriptor",
        ));
    }
    if m.chunk_size != g.chunk_size {
        return Err(BlobError::InvalidManifest(
            "manifest chunk size differs from descriptor",
        ));
    }
    if m.chunks.len() != g.chunk_count as usize {
        return Err(BlobError::InvalidManifest(
            "manifest chunk count differs from geometry",
        ));
    }
    for (i, c) in m.chunks.iter().enumerate() {
        let i = i as u32;
        if c.index != i || c.offset != g.offset(i) || c.size != g.len_of(i) {
            return Err(BlobError::InvalidManifest(
                "manifest chunk layout is inconsistent",
            ));
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct StagingMeta {
    version: u16,
    descriptor: BlobDescriptor,
    manifest: Option<BlobManifest>,
}

/// Streams a file through a bounded buffer into a blake3 hasher.
fn hash_file(file: &mut File) -> Result<BlobId, BlobError> {
    file.seek(SeekFrom::Start(0))
        .map_err(io("seek for hashing"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; HASH_BUF_BYTES];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io("hash file")(e)),
        }
    }
    Ok(BlobId::from_streaming_hasher(&hasher))
}

struct Shared<M> {
    root: PathBuf,
    cfg: FsStoreConfig,
    ledger: Arc<QuotaLedger>,
    active: Mutex<HashSet<BlobId>>,
    meta: M,
}

impl<M> Shared<M> {
    fn objects(&self) -> PathBuf {
        self.root.join("objects")
    }
    fn object_path(&self, id: BlobId) -> PathBuf {
        let h = hex(id.as_bytes());
        self.objects().join(&h[..2]).join(h)
    }
    fn part_path(&self, id: BlobId) -> PathBuf {
        self.root
            .join("staging")
            .join(format!("{}.part", hex(id.as_bytes())))
    }
    fn meta_path(&self, id: BlobId) -> PathBuf {
        self.root
            .join("staging")
            .join(format!("{}.meta", hex(id.as_bytes())))
    }
    fn mkdir(&self, p: &Path) -> Result<(), BlobError> {
        fs::create_dir_all(p).map_err(io("create directory"))?;
        set_mode(p, self.cfg.permissions.directory_mode)
    }
    fn sync_dir(&self, p: &Path) -> Result<(), BlobError> {
        if self.cfg.durability == DurabilityPolicy::Fsync {
            #[cfg(unix)]
            {
                File::open(p)
                    .and_then(|d| d.sync_all())
                    .map_err(io("sync directory"))?;
            }
            #[cfg(not(unix))]
            {
                let _ = p;
            }
        }
        Ok(())
    }
    fn quarantine(&self, part: &Path, id: BlobId) {
        let dest = self.root.join("quarantine").join(format!(
            "{}.{}",
            hex(id.as_bytes()),
            (self.cfg.now_millis)()
        ));
        // Best effort: if even the move fails, delete rather than leave
        // an unverified file where recovery might later trust it.
        if fs::rename(part, &dest).is_err() {
            let _ = fs::remove_file(part);
        }
    }
}

/// See the module doc.
pub struct FsBlobStore<M: BlobMetadataStore> {
    shared: Arc<Shared<M>>,
}

impl<M: BlobMetadataStore> Clone for FsBlobStore<M> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableStaging {
    pub blob_id: BlobId,
    pub total_chunks: u32,
    pub verified_chunks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    /// Staging whose manifest let chunks be re-verified.
    pub resumable: Vec<ResumableStaging>,
    /// `.part` files with no usable `.meta`.
    pub discarded_orphans: u32,
    /// Sidecars with no manifest: nothing in the `.part` is verifiable.
    pub discarded_unverifiable: u32,
    /// Staging left behind by a crash after the rename: object exists.
    pub completed_late: u32,
    pub temp_files_removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReconcileReport {
    pub rows_added: u32,
    pub rows_removed: u32,
    pub corrupt_quarantined: u32,
}

/// One incomplete transfer's staging data, as seen on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingInfo {
    pub blob_id: BlobId,
    /// Bytes the `.part` file logically holds (its length, which for a
    /// sparse file is the *declared* size, not what is allocated).
    pub part_bytes: u64,
    /// When the staging data last changed. Chunk writes bump the
    /// `.part`'s modification time, so this is "last progress".
    pub modified_millis: u64,
}

fn mtime_millis(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

impl<M: BlobMetadataStore> FsBlobStore<M> {
    /// Creates the directory layout (restrictive modes per §92) and
    /// scans `objects/` once to learn how much space is already used.
    pub fn open(root: impl Into<PathBuf>, cfg: FsStoreConfig, meta: M) -> Result<Self, BlobError> {
        let root = root.into();
        let shared = Shared {
            ledger: QuotaLedger::new(cfg.limits, 0),
            root,
            cfg,
            active: Mutex::new(HashSet::new()),
            meta,
        };
        shared.mkdir(&shared.root)?;
        for d in ["objects", "staging", "temp", "quarantine"] {
            shared.mkdir(&shared.root.join(d))?;
        }
        let mut used = 0u64;
        for_each_object(&shared.objects(), |_, len| used += len)?;
        let ledger = QuotaLedger::new(cfg.limits, used);
        Ok(Self {
            shared: Arc::new(Shared { ledger, ..shared }),
        })
    }

    pub fn ledger(&self) -> &Arc<QuotaLedger> {
        &self.shared.ledger
    }

    pub fn metadata(&self) -> &M {
        &self.shared.meta
    }

    pub fn root(&self) -> &Path {
        &self.shared.root
    }

    /// Opens a write; with a manifest each chunk is verified against its
    /// own hash as it arrives (§14) and an interrupted write can resume
    /// (§52). See [`BlobStore::begin_write`] for the manifest-less form.
    pub fn begin_write_with_manifest(
        &self,
        descriptor: BlobDescriptor,
        manifest: Option<BlobManifest>,
    ) -> Result<FsBlobWriter<M>, BlobError> {
        let sh = &self.shared;
        let g = Geometry::of(&descriptor)?;
        if let Some(m) = &manifest {
            check_manifest(&g, &descriptor, m)?;
        }
        let id = descriptor.blob_id;

        // §59: already stored — a no-op writer, not an error.
        if sh.object_path(id).is_file() {
            return Ok(FsBlobWriter::already_present(Arc::clone(sh), descriptor, g));
        }

        if !sh.active.lock().expect("active poisoned").insert(id) {
            return Err(BlobError::AlreadyStaging(id));
        }
        let guard = ActiveGuard {
            shared: Arc::clone(sh),
            id,
        };

        // §48: promise the space first.
        let reservation = sh.ledger.reserve(g.size)?;

        let part = sh.part_path(id);
        let mut bitmap = ResumeBitmap::new(g.chunk_count as usize);
        let resuming = part.is_file() && manifest.is_some();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(!resuming)
            .open(&part)
            .map_err(io("open staging"))?;
        set_mode(&part, sh.cfg.permissions.file_mode)?;

        if resuming {
            // §52: do not trust the old bytes — re-verify every chunk.
            scan_verified_chunks(
                &mut file,
                &g,
                manifest.as_ref().expect("resuming implies manifest"),
                &mut bitmap,
            )?;
        } else if sh.cfg.preallocation == Preallocation::SparseSetLen {
            file.set_len(g.size).map_err(io("preallocate staging"))?;
        }

        // Sidecar so recovery can resume after a crash. Written via
        // temp + rename so a torn sidecar is never read.
        let meta = StagingMeta {
            version: STAGING_META_VERSION,
            descriptor,
            manifest: manifest.clone(),
        };
        let bytes = postcard::to_allocvec(&meta)
            .map_err(|_| BlobError::CorruptStaging("cannot encode sidecar"))?;
        let tmp = sh
            .root
            .join("temp")
            .join(format!("{}.meta.tmp", hex(id.as_bytes())));
        fs::write(&tmp, &bytes).map_err(io("write sidecar"))?;
        fs::rename(&tmp, sh.meta_path(id)).map_err(io("publish sidecar"))?;

        Ok(FsBlobWriter {
            shared: Arc::clone(sh),
            descriptor,
            geometry: g,
            manifest,
            bitmap,
            file: Some(file),
            reservation: Some(reservation),
            _guard: Some(guard),
            already_present: false,
        })
    }

    /// §52. See the module doc. Safe to call repeatedly.
    pub fn recover(&self) -> Result<RecoveryReport, BlobError> {
        let sh = &self.shared;
        let mut report = RecoveryReport::default();

        // temp/ is scratch by definition.
        for e in fs::read_dir(sh.root.join("temp")).map_err(io("read temp"))? {
            let e = e.map_err(io("read temp entry"))?;
            if fs::remove_file(e.path()).is_ok() {
                report.temp_files_removed += 1;
            }
        }

        let staging = sh.root.join("staging");
        let mut metas = Vec::new();
        let mut parts = Vec::new();
        for e in fs::read_dir(&staging).map_err(io("read staging"))? {
            let p = e.map_err(io("read staging entry"))?.path();
            match p.extension().and_then(|x| x.to_str()) {
                Some("meta") => metas.push(p),
                Some("part") => parts.push(p),
                _ => {
                    let _ = fs::remove_file(&p);
                }
            }
        }

        let mut claimed_parts: HashSet<PathBuf> = HashSet::new();
        for meta_path in metas {
            let part_path = meta_path.with_extension("part");
            let parsed = fs::read(&meta_path)
                .ok()
                .and_then(|b| postcard::from_bytes::<StagingMeta>(&b).ok())
                .filter(|m| m.version == STAGING_META_VERSION);
            let Some(m) = parsed else {
                let _ = fs::remove_file(&meta_path);
                let _ = fs::remove_file(&part_path);
                report.discarded_orphans += 1;
                continue;
            };
            let id = m.descriptor.blob_id;
            // Crash after the rename: the object is already there.
            if sh.object_path(id).is_file() {
                let _ = fs::remove_file(&meta_path);
                let _ = fs::remove_file(&part_path);
                report.completed_late += 1;
                continue;
            }
            let Some(manifest) = m.manifest else {
                // Nothing to verify the bytes against: do not trust them.
                let _ = fs::remove_file(&meta_path);
                let _ = fs::remove_file(&part_path);
                report.discarded_unverifiable += 1;
                continue;
            };
            let Ok(g) = Geometry::of(&m.descriptor) else {
                let _ = fs::remove_file(&meta_path);
                let _ = fs::remove_file(&part_path);
                report.discarded_orphans += 1;
                continue;
            };
            let mut bitmap = ResumeBitmap::new(g.chunk_count as usize);
            match OpenOptions::new().read(true).open(&part_path) {
                Ok(mut f) => {
                    scan_verified_chunks(&mut f, &g, &manifest, &mut bitmap)?;
                }
                Err(_) => { /* no bytes at all: everything missing */ }
            }
            claimed_parts.insert(part_path);
            report.resumable.push(ResumableStaging {
                blob_id: id,
                total_chunks: g.chunk_count,
                verified_chunks: bitmap.received_count() as u32,
            });
        }

        // A `.part` nobody claims is an orphan.
        for p in parts {
            if !claimed_parts.contains(&p) && p.is_file() {
                let _ = fs::remove_file(&p);
                report.discarded_orphans += 1;
            }
        }
        report.resumable.sort_by_key(|r| *r.blob_id.as_bytes());
        Ok(report)
    }

    /// The descriptor (and manifest, if one was supplied) a staged
    /// transfer was started with — what a restarted process needs to
    /// resume it. `None` if there is no readable sidecar.
    pub fn staging_meta(
        &self,
        id: BlobId,
    ) -> Result<Option<(BlobDescriptor, Option<BlobManifest>)>, BlobError> {
        let bytes = match fs::read(self.shared.meta_path(id)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io("read sidecar")(e)),
        };
        match postcard::from_bytes::<StagingMeta>(&bytes) {
            Ok(m) if m.version == STAGING_META_VERSION && m.descriptor.blob_id == id => {
                Ok(Some((m.descriptor, m.manifest)))
            }
            _ => Ok(None),
        }
    }

    /// `true` if a writer for `id` is open right now in this process.
    pub fn is_active(&self, id: BlobId) -> bool {
        self.shared
            .active
            .lock()
            .expect("active poisoned")
            .contains(&id)
    }

    /// Every incomplete transfer's staging data (§152/§153's raw
    /// material). Entries are keyed by the blob id parsed from the file
    /// *name*; anything not named like one is ignored.
    pub fn list_staging(&self) -> Result<Vec<StagingInfo>, BlobError> {
        let mut by_id: std::collections::BTreeMap<BlobId, StagingInfo> = Default::default();
        for e in fs::read_dir(self.shared.root.join("staging")).map_err(io("read staging"))? {
            let e = e.map_err(io("read staging entry"))?;
            let path = e.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(parse_id_hex)
            else {
                continue;
            };
            let Ok(md) = e.metadata() else { continue };
            let is_part = path.extension().and_then(|x| x.to_str()) == Some("part");
            let entry = by_id.entry(id).or_insert(StagingInfo {
                blob_id: id,
                part_bytes: 0,
                modified_millis: 0,
            });
            if is_part {
                entry.part_bytes = md.len();
            }
            entry.modified_millis = entry.modified_millis.max(mtime_millis(&md));
        }
        Ok(by_id.into_values().collect())
    }

    /// Deletes one transfer's staging data; returns the bytes it held.
    /// Refuses ([`BlobError::AlreadyStaging`]) while a writer for the
    /// blob is open — GC must never pull a file out from under a
    /// transfer that is writing to it.
    pub fn discard_staging(&self, id: BlobId) -> Result<u64, BlobError> {
        if self.is_active(id) {
            return Err(BlobError::AlreadyStaging(id));
        }
        let part = self.shared.part_path(id);
        let bytes = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(&part);
        let _ = fs::remove_file(self.shared.meta_path(id));
        Ok(bytes)
    }

    /// Deep check: re-hash the stored file and compare to its id.
    pub fn verify_object(&self, id: BlobId) -> Result<bool, BlobError> {
        let path = self.shared.object_path(id);
        let mut f = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobError::NotFound(id))
            }
            Err(e) => return Err(io("open object")(e)),
        };
        Ok(hash_file(&mut f)? == id)
    }

    /// §89: make metadata agree with the files. Files are the truth; a
    /// missing row is rebuilt (after re-hashing the file — a corrupt
    /// object is quarantined instead), a row with no file is removed.
    pub fn reconcile_metadata(&self) -> Result<ReconcileReport, BlobError> {
        let sh = &self.shared;
        let mut report = ReconcileReport::default();
        let mut on_disk: Vec<(BlobId, u64)> = Vec::new();
        for_each_object(&sh.objects(), |id, len| on_disk.push((id, len)))?;
        let present: HashSet<BlobId> = on_disk.iter().map(|(i, _)| *i).collect();

        for (id, len) in on_disk {
            if sh.meta.get(id)?.is_some() {
                continue;
            }
            if !self.verify_object(id)? {
                let p = sh.object_path(id);
                sh.quarantine(&p, id);
                sh.ledger.release_used(len);
                report.corrupt_quarantined += 1;
                continue;
            }
            let now = (sh.cfg.now_millis)();
            sh.meta.put(BlobRow {
                blob_id: id,
                size: len,
                state: BlobLifecycleState::Verified,
                created_at_millis: now,
                last_accessed_millis: now,
                storage_class: sh.cfg.default_class,
                reference_count: 0,
            })?;
            report.rows_added += 1;
        }
        for row in sh.meta.list(usize::MAX)? {
            if !present.contains(&row.blob_id) && sh.meta.remove(row.blob_id)? {
                report.rows_removed += 1;
            }
        }
        Ok(report)
    }

    pub fn contains_sync(&self, id: BlobId) -> bool {
        self.shared.object_path(id).is_file()
    }

    pub fn stat_sync(&self, id: BlobId) -> Result<BlobStat, BlobError> {
        let path = self.shared.object_path(id);
        let len = match fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobError::NotFound(id))
            }
            Err(e) => return Err(io("stat object")(e)),
        };
        let row = self.shared.meta.get(id)?;
        Ok(BlobStat {
            blob_id: id,
            size: len,
            state: row.map_or(BlobLifecycleState::Verified, |r| r.state),
            last_accessed_millis: row.map_or(0, |r| r.last_accessed_millis),
        })
    }

    pub fn open_sync(&self, id: BlobId) -> Result<FsBlobReader, BlobError> {
        let path = self.shared.object_path(id);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(BlobError::NotFound(id))
            }
            Err(e) => return Err(io("open object")(e)),
        };
        let size = file.metadata().map_err(io("stat object"))?.len();
        self.shared.meta.touch(id, (self.shared.cfg.now_millis)())?;
        Ok(FsBlobReader {
            file: Mutex::new(file),
            size,
        })
    }

    pub fn delete_sync(&self, id: BlobId) -> Result<(), BlobError> {
        let path = self.shared.object_path(id);
        let len = match fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.shared.meta.remove(id)?;
                return Ok(());
            }
            Err(e) => return Err(io("stat object")(e)),
        };
        fs::remove_file(&path).map_err(io("delete object"))?;
        self.shared.meta.remove(id)?;
        self.shared.ledger.release_used(len);
        Ok(())
    }
}

/// Walks `objects/<xx>/<hex>` calling `f(id, len)` for each well-formed
/// entry. Anything not named like a blob id is ignored, never trusted.
fn for_each_object(objects: &Path, mut f: impl FnMut(BlobId, u64)) -> Result<(), BlobError> {
    for shard in fs::read_dir(objects).map_err(io("read objects"))? {
        let shard = shard.map_err(io("read shard"))?.path();
        if !shard.is_dir() {
            continue;
        }
        for obj in fs::read_dir(&shard).map_err(io("read shard"))? {
            let obj = obj.map_err(io("read object entry"))?;
            let name = obj.file_name();
            let Some(id) = name.to_str().and_then(parse_id_hex) else {
                continue;
            };
            if let Ok(m) = obj.metadata() {
                if m.is_file() {
                    f(id, m.len());
                }
            }
        }
    }
    Ok(())
}

/// Re-hashes every chunk the file contains against `manifest`; chunks
/// that verify are marked received. A short file, a hole, or a flipped
/// bit all simply fail to verify and stay missing.
fn scan_verified_chunks(
    file: &mut File,
    g: &Geometry,
    manifest: &BlobManifest,
    bitmap: &mut ResumeBitmap,
) -> Result<(), BlobError> {
    let file_len = file.metadata().map_err(io("stat staging"))?.len();
    let mut buf = vec![0u8; g.chunk_size as usize];
    for c in &manifest.chunks {
        let end = c.offset + u64::from(c.size);
        if end > file_len {
            continue;
        }
        file.seek(SeekFrom::Start(c.offset))
            .map_err(io("seek staging"))?;
        let slice = &mut buf[..c.size as usize];
        if file.read_exact(slice).is_err() {
            continue;
        }
        if ChunkHash::from_ciphertext_chunk(slice) == c.hash {
            bitmap.mark_received(c.index);
        }
    }
    Ok(())
}

struct ActiveGuard<M: BlobMetadataStore> {
    shared: Arc<Shared<M>>,
    id: BlobId,
}

impl<M: BlobMetadataStore> Drop for ActiveGuard<M> {
    fn drop(&mut self) {
        self.shared
            .active
            .lock()
            .expect("active poisoned")
            .remove(&self.id);
    }
}

/// An in-progress write. Dropping it **without** finalizing releases the
/// reservation and the concurrency slot but keeps the staging data, so
/// the transfer can resume (§52); [`BlobWriter::abort`] deletes it.
pub struct FsBlobWriter<M: BlobMetadataStore> {
    shared: Arc<Shared<M>>,
    descriptor: BlobDescriptor,
    geometry: Geometry,
    manifest: Option<BlobManifest>,
    bitmap: ResumeBitmap,
    file: Option<File>,
    reservation: Option<Reservation>,
    _guard: Option<ActiveGuard<M>>,
    already_present: bool,
}

impl<M: BlobMetadataStore> FsBlobWriter<M> {
    fn already_present(
        shared: Arc<Shared<M>>,
        descriptor: BlobDescriptor,
        geometry: Geometry,
    ) -> Self {
        let mut bitmap = ResumeBitmap::new(geometry.chunk_count as usize);
        for i in 0..geometry.chunk_count {
            bitmap.mark_received(i);
        }
        Self {
            shared,
            descriptor,
            geometry,
            manifest: None,
            bitmap,
            file: None,
            reservation: None,
            _guard: None,
            already_present: true,
        }
    }

    /// Chunks already verified (e.g. after a resume).
    pub fn received_chunks(&self) -> usize {
        self.bitmap.received_count()
    }

    /// `true` if the blob was already stored when this write began.
    pub fn was_already_present(&self) -> bool {
        self.already_present
    }

    pub fn write_chunk_sync(&mut self, index: u32, data: &[u8]) -> Result<(), BlobError> {
        if self.already_present {
            return Ok(());
        }
        let g = self.geometry;
        if index >= g.chunk_count {
            return Err(BlobError::ChunkIndexOutOfRange {
                index,
                chunk_count: g.chunk_count,
            });
        }
        let expected = g.len_of(index);
        if data.len() != expected as usize {
            return Err(BlobError::ChunkSizeMismatch {
                index,
                expected,
                actual: data.len(),
            });
        }
        if let Some(m) = &self.manifest {
            if ChunkHash::from_ciphertext_chunk(data) != m.chunks[index as usize].hash {
                return Err(BlobError::ChunkHashMismatch { index });
            }
            if self.bitmap.is_received(index) {
                return Ok(()); // identical by hash: idempotent
            }
        }
        let file = self.file.as_mut().expect("file present until finalize");
        file.seek(SeekFrom::Start(g.offset(index)))
            .map_err(io("seek staging"))?;
        file.write_all(data).map_err(io("write chunk"))?;
        self.bitmap.mark_received(index);
        Ok(())
    }

    pub fn finalize_sync(mut self) -> Result<BlobId, BlobError> {
        let id = self.descriptor.blob_id;
        if self.already_present {
            return Ok(id);
        }
        let missing = self.geometry.chunk_count as usize - self.bitmap.received_count();
        if missing > 0 {
            return Err(BlobError::Incomplete {
                missing_chunks: missing as u32,
            });
        }
        let sh = Arc::clone(&self.shared);
        let mut file = self.file.take().expect("file present until finalize");
        file.flush().map_err(io("flush staging"))?;
        // A file longer than declared (SparseSetLen off + bad writes) is
        // wrong by construction.
        let len = file.metadata().map_err(io("stat staging"))?.len();
        if len != self.geometry.size {
            return Err(BlobError::SizeMismatch {
                expected: self.geometry.size,
                actual: len,
            });
        }
        if sh.cfg.durability == DurabilityPolicy::Fsync {
            file.sync_all().map_err(io("fsync staging"))?;
        }

        // Full verification before anything becomes visible.
        let actual = hash_file(&mut file)?;
        drop(file);
        let part = sh.part_path(id);
        if actual != id {
            sh.quarantine(&part, id);
            let _ = fs::remove_file(sh.meta_path(id));
            return Err(BlobError::BlobHashMismatch { declared: id });
        }

        // Atomic publish.
        let final_path = sh.object_path(id);
        let shard = final_path
            .parent()
            .expect("object path has a shard dir")
            .to_path_buf();
        sh.mkdir(&shard)?;
        if final_path.is_file() {
            // Raced a concurrent identical write: same bytes by hash.
            let _ = fs::remove_file(&part);
        } else {
            fs::rename(&part, &final_path).map_err(io("publish object"))?;
        }
        set_mode(&final_path, sh.cfg.permissions.file_mode)?;
        sh.sync_dir(&shard)?;
        let _ = fs::remove_file(sh.meta_path(id));

        if let Some(r) = self.reservation.take() {
            r.commit();
        }
        let now = (sh.cfg.now_millis)();
        sh.meta.put(BlobRow {
            blob_id: id,
            size: self.geometry.size,
            state: BlobLifecycleState::Verified,
            created_at_millis: now,
            last_accessed_millis: now,
            storage_class: sh.cfg.default_class,
            reference_count: 0,
        })?;
        Ok(id)
    }

    pub fn abort_sync(mut self) -> Result<(), BlobError> {
        if self.already_present {
            return Ok(());
        }
        drop(self.file.take());
        let id = self.descriptor.blob_id;
        let _ = fs::remove_file(self.shared.part_path(id));
        let _ = fs::remove_file(self.shared.meta_path(id));
        Ok(())
    }
}

impl<M: BlobMetadataStore> BlobWriter for FsBlobWriter<M> {
    async fn write_chunk(&mut self, index: u32, data: Bytes) -> Result<(), BlobError> {
        self.write_chunk_sync(index, &data)
    }

    fn missing_ranges(&self) -> Vec<(u32, u32)> {
        self.bitmap.missing_ranges()
    }

    async fn finalize(self) -> Result<BlobId, BlobError> {
        self.finalize_sync()
    }

    async fn abort(self) -> Result<(), BlobError> {
        self.abort_sync()
    }
}

pub struct FsBlobReader {
    file: Mutex<File>,
    size: u64,
}

impl FsBlobReader {
    pub fn read_range_sync(&self, range: ByteRange) -> Result<Bytes, BlobError> {
        range.validate(self.size, MAX_READ_RANGE_BYTES)?;
        let mut buf = vec![0u8; range.len() as usize];
        let mut f = self.file.lock().expect("reader poisoned");
        f.seek(SeekFrom::Start(range.start()))
            .map_err(io("seek object"))?;
        f.read_exact(&mut buf).map_err(io("read object"))?;
        Ok(Bytes::from(buf))
    }

    pub fn stream_sync(&self, chunk_size: usize) -> Result<BlobStream, BlobError> {
        let f = self.file.lock().expect("reader poisoned");
        let mut dup = f.try_clone().map_err(io("clone handle"))?;
        dup.seek(SeekFrom::Start(0)).map_err(io("seek object"))?;
        BlobStream::new(dup, self.size, chunk_size)
    }
}

impl BlobReader for FsBlobReader {
    fn size(&self) -> u64 {
        self.size
    }

    async fn read_range(&self, range: ByteRange) -> Result<Bytes, BlobError> {
        self.read_range_sync(range)
    }

    async fn stream(&self, chunk_size: usize) -> Result<BlobStream, BlobError> {
        self.stream_sync(chunk_size)
    }
}

impl<M: BlobMetadataStore> BlobStore for FsBlobStore<M> {
    type Reader = FsBlobReader;
    type Writer = FsBlobWriter<M>;

    async fn contains(&self, id: BlobId) -> Result<bool, BlobError> {
        Ok(self.contains_sync(id))
    }

    async fn stat(&self, id: BlobId) -> Result<BlobStat, BlobError> {
        self.stat_sync(id)
    }

    async fn open(&self, id: BlobId) -> Result<FsBlobReader, BlobError> {
        self.open_sync(id)
    }

    /// Without a manifest, chunks cannot be individually verified and an
    /// interrupted write cannot be resumed — only the final whole-blob
    /// hash protects integrity. Use
    /// [`FsBlobStore::begin_write_with_manifest`] when a manifest is
    /// known.
    async fn begin_write(&self, descriptor: BlobDescriptor) -> Result<FsBlobWriter<M>, BlobError> {
        self.begin_write_with_manifest(descriptor, None)
    }

    async fn delete(&self, id: BlobId) -> Result<(), BlobError> {
        self.delete_sync(id)
    }
}
