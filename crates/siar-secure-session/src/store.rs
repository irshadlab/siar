//! A minimal blob store the secure-session layer persists into.
//!
//! Deliberately small (`get`/`put`/`delete`) so it can sit on a file, the
//! application database, or a platform secure store. [`FileBlobStore`] is
//! crash-safe: write to a temp file, `fsync`, rename, `fsync` the directory —
//! a reader sees the old blob or the new one, never half of one.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

pub trait BlobStore: Send + Sync {
    fn get(&self, key: &str) -> io::Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, value: &[u8]) -> io::Result<()>;
    fn delete(&self, key: &str) -> io::Result<()>;
}

/// Keys become file names, so they are validated hard: `[a-z0-9._-]` segments
/// joined by `/`, no empty segment, no `.`/`..`, no leading slash.
#[must_use]
pub fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 128
        && k.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
        })
}

fn bad_key() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid blob key")
}

#[derive(Debug)]
pub struct FileBlobStore {
    root: PathBuf,
}

static TMP: AtomicU64 = AtomicU64::new(0);

impl FileBlobStore {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { root })
    }

    fn path(&self, key: &str) -> io::Result<PathBuf> {
        if valid_key(key) {
            Ok(self.root.join(key))
        } else {
            Err(bad_key())
        }
    }
}

impl BlobStore for FileBlobStore {
    fn get(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.path(key)?) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn put(&self, key: &str, value: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let path = self.path(key)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TMP.fetch_add(1, Ordering::Relaxed)
        ));
        let result = (|| {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            f.write_all(value)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)?;
            // Make the rename itself durable.
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Default)]
pub struct MemBlobStore {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemBlobStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A copy of everything (for "restore from an old backup" tests).
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, Vec<u8>> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn restore(&self, snap: HashMap<String, Vec<u8>>) {
        *self.inner.lock().unwrap_or_else(PoisonError::into_inner) = snap;
    }
}

impl BlobStore for MemBlobStore {
    fn get(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        if !valid_key(key) {
            return Err(bad_key());
        }
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned())
    }
    fn put(&self, key: &str, value: &[u8]) -> io::Result<()> {
        if !valid_key(key) {
            return Err(bad_key());
        }
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> io::Result<()> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
        Ok(())
    }
}

/// Fault injection for crash tests: fails the next `n` writes whose key
/// contains `needle`, then behaves normally.
#[doc(hidden)]
pub struct FaultyStore<S: BlobStore> {
    pub inner: S,
    needle: String,
    remaining: AtomicU64,
}

impl<S: BlobStore> FaultyStore<S> {
    pub fn new(inner: S, needle: &str) -> Self {
        Self {
            inner,
            needle: needle.to_owned(),
            remaining: AtomicU64::new(0),
        }
    }
    pub fn fail_next(&self, n: u64) {
        self.remaining.store(n, Ordering::SeqCst);
    }
}

impl<S: BlobStore> BlobStore for FaultyStore<S> {
    fn get(&self, key: &str) -> io::Result<Option<Vec<u8>>> {
        self.inner.get(key)
    }
    fn put(&self, key: &str, value: &[u8]) -> io::Result<()> {
        if key.contains(&self.needle)
            && self
                .remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return Err(io::Error::other("injected write failure"));
        }
        self.inner.put(key, value)
    }
    fn delete(&self, key: &str) -> io::Result<()> {
        self.inner.delete(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_cannot_escape_the_store() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "A",
            "a b",
            ".",
            "a/./b",
            &"x".repeat(129),
            "a\0b",
        ] {
            assert!(!valid_key(bad), "{bad:?}");
        }
        for ok in ["sessions/ab12.state", "hw/ff", "prekeys.state", "a-b_c.d"] {
            assert!(valid_key(ok), "{ok:?}");
        }
    }

    #[test]
    fn file_store_round_trips_overwrites_and_is_private() {
        let d = std::env::temp_dir().join(format!("siar-blob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let s = FileBlobStore::new(&d).unwrap();
        assert_eq!(s.get("sessions/x.state").unwrap(), None);
        s.put("sessions/x.state", b"one").unwrap();
        s.put("sessions/x.state", b"two").unwrap();
        assert_eq!(
            s.get("sessions/x.state").unwrap().as_deref(),
            Some(&b"two"[..])
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(d.join("sessions/x.state"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&d).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(
            std::fs::read_dir(d.join("sessions")).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp")),
            "no temp files left"
        );
        s.delete("sessions/x.state").unwrap();
        s.delete("sessions/x.state").unwrap(); // idempotent
        assert!(s.put("../escape", b"x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn injected_failures_fire_once_per_count() {
        let f = FaultyStore::new(MemBlobStore::new(), "state");
        f.fail_next(1);
        assert!(f.put("sessions/a.state", b"x").is_err());
        assert!(f.put("sessions/a.state", b"x").is_ok());
        assert!(f.put("hw/a", b"x").is_ok(), "only matching keys fail");
    }
}
