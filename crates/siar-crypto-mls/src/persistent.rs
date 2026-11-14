//! Durable MLS group state in **pure Rust** — no SQLite, no C.
//!
//! `OpenMlsRustCrypto`'s bundled storage (`openmls_memory_storage::MemoryStorage`)
//! is in-memory only, so a restart lost every group's live crypto state.
//! [`FileStorage`] wraps that same store and writes a snapshot to disk after
//! **every successful mutation, before the call returns** (the delegating
//! `impl StorageProvider` is generated from openmls's own trait by
//! `cargo xtask gen-mls-storage`; `--check` fails CI on drift).
//!
//! Why write-through and not "save on shutdown": an MLS ratchet advance that
//! is not on disk when a message is sent means, after a crash, the *same*
//! key can be derived and used for a different plaintext. Nothing is
//! acknowledged to openmls that is not durable.
//!
//! ## Format
//!
//! One RON file (`Snapshot { version, entries: [(key_hex, value_hex)] }`),
//! entries sorted by key so the file is deterministic. The stored *values*
//! are opaque bytes produced by `openmls_memory_storage`, which serialises
//! them with `serde_json` internally — that is upstream's encoding of its own
//! values and is not ours to change; everything this crate authors is RON.
//! Writes are atomic (temp file → `fsync` → rename → `fsync` the directory),
//! mode 0600.
//!
//! ## Costs and limits, stated plainly
//!
//! * Every mutation rewrites the **whole** snapshot and fsyncs it. That is
//!   O(state) per write and openmls issues many writes per operation. Fine
//!   for the handful of small groups a chat client holds; it is not a
//!   database, and a very large group will feel it.
//! * If persisting fails *after* the in-memory change succeeded, the error is
//!   returned to openmls (the operation fails) but memory has advanced; the
//!   next successful write persists everything. That errs toward skipping a
//!   ratchet generation, never toward reusing one.
//! * A snapshot that does not parse is **refused** (`PersistentProviderError::Corrupt`),
//!   never silently replaced by an empty state — that would orphan every group.
//! * No wiring into `GroupService` (see `lib.rs`): where the file lives and how
//!   a restarting service finds its conversations is an application decision.
//! * No migration from the earlier SQLite files — this crate is pre-release.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openmls::prelude::OpenMlsProvider;
use openmls_memory_storage::{MemoryStorage, MemoryStorageError};
use openmls_rust_crypto::RustCrypto;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[path = "persistent_delegate.rs"]
mod delegate;

const SNAPSHOT_VERSION: u32 = 1;
/// A snapshot larger than this is refused rather than read into memory.
pub const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum FileStorageError {
    #[error("MLS storage: {0}")]
    Memory(MemoryStorageError),
    #[error("could not write the MLS state to disk: {0}")]
    Persist(io::Error),
}

#[derive(Debug, Error)]
pub enum PersistentProviderError {
    #[error("failed to open the MLS state file: {0}")]
    Open(String),
    #[error("the MLS state file is corrupt and was NOT replaced: {0}")]
    Corrupt(String),
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    /// `(key, value)` as lowercase hex, sorted by key.
    entries: Vec<(String, String)>,
}

fn to_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

/// The in-memory MLS store plus the file it is mirrored to.
pub struct FileStorage {
    inner: MemoryStorage,
    path: PathBuf,
}

impl FileStorage {
    /// Open (or start) the store at `path`. A missing file is an empty store;
    /// an unreadable or malformed one is an error.
    pub fn open(path: &Path) -> Result<Self, PersistentProviderError> {
        let inner = MemoryStorage::default();
        match fs::metadata(path) {
            Ok(meta) => {
                if meta.len() > MAX_SNAPSHOT_BYTES {
                    return Err(PersistentProviderError::Corrupt("file is too large".into()));
                }
                let bytes =
                    fs::read(path).map_err(|e| PersistentProviderError::Open(e.to_string()))?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| PersistentProviderError::Corrupt("not valid UTF-8".into()))?;
                let snap: Snapshot = ron::from_str(&text)
                    .map_err(|e| PersistentProviderError::Corrupt(e.to_string()))?;
                if snap.version != SNAPSHOT_VERSION {
                    return Err(PersistentProviderError::Corrupt(format!(
                        "unsupported snapshot version {}",
                        snap.version
                    )));
                }
                let mut values = inner
                    .values
                    .write()
                    .map_err(|_| PersistentProviderError::Open("lock poisoned".into()))?;
                for (k, v) in snap.entries {
                    let (Some(k), Some(v)) = (from_hex(&k), from_hex(&v)) else {
                        return Err(PersistentProviderError::Corrupt("entry is not hex".into()));
                    };
                    values.insert(k, v);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(PersistentProviderError::Open(e.to_string())),
        }
        Ok(Self {
            inner,
            path: path.to_owned(),
        })
    }

    /// Write the whole store atomically and durably.
    fn persist(&self) -> io::Result<()> {
        let mut entries: Vec<(String, String)> = {
            let values = self
                .inner
                .values
                .read()
                .map_err(|_| io::Error::other("lock poisoned"))?;
            values.iter().map(|(k, v)| (to_hex(k), to_hex(v))).collect()
        };
        entries.sort();
        let text = ron::to_string(&Snapshot {
            version: SNAPSHOT_VERSION,
            entries,
        })
        .map_err(io::Error::other)?;

        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(dir)?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut opts = OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        // Make the rename itself durable.
        #[cfg(unix)]
        File::open(dir)?.sync_all()?;
        Ok(())
    }

    /// Number of stored entries (for tests and diagnostics).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.values.read().map_or(0, |v| v.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An `OpenMlsProvider` whose storage survives a restart. Crypto and
/// randomness are `openmls_rust_crypto`'s `RustCrypto` — pure Rust, reused
/// unchanged, since swapping the storage backend has nothing to do with them.
pub struct FilePersistentProvider {
    crypto: RustCrypto,
    storage: FileStorage,
}

impl FilePersistentProvider {
    pub fn open(path: &Path) -> Result<Self, PersistentProviderError> {
        Ok(Self {
            crypto: RustCrypto::default(),
            storage: FileStorage::open(path)?,
        })
    }
}

impl OpenMlsProvider for FilePersistentProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = FileStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}
