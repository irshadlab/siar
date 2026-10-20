//! §120-125: where a file's bytes come from and where an exported file
//! goes, without tying the core to `std::fs::File` or to POSIX paths.
//!
//! * [`FileSource`] (§124) — `len` and positional `read_at`. Android's
//!   `content://` URIs, iOS security-scoped URLs and a desktop path all
//!   implement it in the platform adapter (§120-123); the core never
//!   assumes a path.
//! * [`FileSink`] (§125) — `write` and `commit`. Nothing is visible at
//!   the destination until `commit`; dropping an uncommitted sink leaves
//!   nothing behind. (`commit` takes `self` by value rather than the
//!   spec's `Box<Self>`: callers here are generic, not `dyn`; a platform
//!   adapter that needs `dyn` can box on its side.)
//! * [`read_all_bounded`] — the one safe way to pull a whole source into
//!   memory: length checked before any allocation (§139), a source that
//!   changes or grows while it is read is refused (§118).
//!
//! Implementations here: [`MemorySource`]/[`MemorySink`] (tests, small
//! payloads) and [`FsFileSource`]/[`FsFileSink`] (desktop). Reads and
//! writes on the filesystem run on tokio's blocking pool so they never
//! stall an async task.

use std::fs::File;
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use thiserror::Error;

/// Largest single read issued against a source.
pub const READ_PIECE: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FileError {
    #[error("i/o error: {0:?}")]
    Io(ErrorKind),
    #[error("not a regular file")]
    NotARegularFile,
    #[error("source is {actual} bytes, over the {max}-byte limit")]
    TooLarge { actual: u64, max: u64 },
    #[error("the source changed while it was being read")]
    Changed,
    #[error("source ended before its declared length")]
    ShortRead,
    #[error("destination already exists")]
    DestinationExists,
    #[error("the blocking task failed")]
    TaskFailed,
}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.kind())
    }
}

/// §124.
pub trait FileSource: Send + Sync {
    fn len(&self) -> impl Future<Output = Result<u64, FileError>> + Send;

    fn is_empty(&self) -> impl Future<Output = Result<bool, FileError>> + Send {
        async move { Ok(self.len().await? == 0) }
    }

    /// Reads up to `buf.len()` bytes at `offset`; `Ok(0)` is end of file.
    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<usize, FileError>> + Send;

    /// A cheap fingerprint of the live source (§118): size plus
    /// mtime/file-id where the platform offers them. The default knows
    /// only the length.
    fn fingerprint(&self) -> impl Future<Output = Result<SourceFingerprint, FileError>> + Send {
        async move {
            Ok(SourceFingerprint {
                size: self.len().await?,
                mtime_millis: None,
                file_id: None,
            })
        }
    }
}

/// What can be observed about a live source without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub size: u64,
    pub mtime_millis: Option<u64>,
    pub file_id: Option<String>,
}

/// §125.
pub trait FileSink: Send {
    fn write(&mut self, data: &[u8]) -> impl Future<Output = Result<(), FileError>> + Send;

    /// Makes the file visible at its destination, atomically.
    fn commit(self) -> impl Future<Output = Result<(), FileError>> + Send;
}

/// Reads exactly `buf.len()` bytes at `offset` or fails.
pub async fn read_exact_at<S: FileSource + ?Sized>(
    src: &S,
    mut offset: u64,
    mut buf: &mut [u8],
) -> Result<(), FileError> {
    while !buf.is_empty() {
        let n = src.read_at(offset, buf).await?;
        if n == 0 {
            return Err(FileError::ShortRead);
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// The whole source, refusing anything over `max` *before* allocating,
/// and anything that moves underneath the read.
pub async fn read_all_bounded<S: FileSource + ?Sized>(
    src: &S,
    max: u64,
) -> Result<Vec<u8>, FileError> {
    let before = src.fingerprint().await?;
    if before.size > max {
        return Err(FileError::TooLarge {
            actual: before.size,
            max,
        });
    }
    let len = usize::try_from(before.size).map_err(|_| FileError::TooLarge {
        actual: before.size,
        max,
    })?;
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| FileError::TooLarge {
            actual: before.size,
            max,
        })?;
    out.resize(len, 0);
    let mut done = 0usize;
    while done < len {
        let end = (done + READ_PIECE).min(len);
        read_exact_at(src, done as u64, &mut out[done..end]).await?;
        done = end;
    }
    // One byte past the end must not exist, and the fingerprint must
    // not have moved.
    let mut probe = [0u8; 1];
    if src.read_at(before.size, &mut probe).await? != 0 {
        return Err(FileError::Changed);
    }
    if src.fingerprint().await? != before {
        return Err(FileError::Changed);
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// In-memory
// ---------------------------------------------------------------------

impl FileSource for Vec<u8> {
    async fn len(&self) -> Result<u64, FileError> {
        Ok(<[u8]>::len(self) as u64)
    }

    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FileError> {
        let Ok(start) = usize::try_from(offset) else {
            return Ok(0);
        };
        let Some(rest) = self.get(start..) else {
            return Ok(0);
        };
        let n = rest.len().min(buf.len());
        buf[..n].copy_from_slice(&rest[..n]);
        Ok(n)
    }
}

/// Collects everything written; `handle()` reads it after `commit`.
#[derive(Debug, Default)]
pub struct MemorySink {
    buf: Vec<u8>,
    done: Arc<Mutex<Option<Vec<u8>>>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Some(bytes)` once committed, `None` if never committed.
    pub fn handle(&self) -> Arc<Mutex<Option<Vec<u8>>>> {
        Arc::clone(&self.done)
    }
}

impl FileSink for MemorySink {
    async fn write(&mut self, data: &[u8]) -> Result<(), FileError> {
        self.buf.extend_from_slice(data);
        Ok(())
    }

    async fn commit(self) -> Result<(), FileError> {
        *self.done.lock().expect("sink") = Some(self.buf);
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------

/// A regular file opened once; later reads go through that handle, so a
/// rename or replacement of the path cannot swap the content mid-read.
#[derive(Debug, Clone)]
pub struct FsFileSource {
    file: Arc<File>,
}

impl FsFileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FileError> {
        let file = File::open(path.as_ref())?;
        if !file.metadata()?.is_file() {
            return Err(FileError::NotARegularFile);
        }
        Ok(Self {
            file: Arc::new(file),
        })
    }

    fn fingerprint_sync(file: &File) -> Result<SourceFingerprint, FileError> {
        let m = file.metadata()?;
        let mtime_millis = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .and_then(|d| u64::try_from(d.as_millis()).ok());
        #[cfg(unix)]
        let file_id = {
            use std::os::unix::fs::MetadataExt;
            Some(format!("{}:{}", m.dev(), m.ino()))
        };
        #[cfg(not(unix))]
        let file_id = None;
        Ok(SourceFingerprint {
            size: m.len(),
            mtime_millis,
            file_id,
        })
    }
}

#[cfg(unix)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn pread(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

impl FileSource for FsFileSource {
    async fn len(&self) -> Result<u64, FileError> {
        Ok(self.file.metadata()?.len())
    }

    async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FileError> {
        let file = Arc::clone(&self.file);
        let want = buf.len();
        let got = tokio::task::spawn_blocking(move || {
            let mut tmp = vec![0u8; want];
            pread(&file, &mut tmp, offset).map(|n| {
                tmp.truncate(n);
                tmp
            })
        })
        .await
        .map_err(|_| FileError::TaskFailed)??;
        buf[..got.len()].copy_from_slice(&got);
        Ok(got.len())
    }

    async fn fingerprint(&self) -> Result<SourceFingerprint, FileError> {
        Self::fingerprint_sync(&self.file)
    }
}

/// Writes to a temporary file beside the destination and renames it into
/// place on `commit`. Refuses to overwrite. Dropped uncommitted, the
/// temporary file is removed.
#[derive(Debug)]
pub struct FsFileSink {
    file: Arc<Mutex<File>>,
    tmp: PathBuf,
    dest: PathBuf,
    committed: bool,
}

impl FsFileSink {
    pub fn create(dest: impl Into<PathBuf>) -> Result<Self, FileError> {
        let dest = dest.into();
        if dest.exists() {
            return Err(FileError::DestinationExists);
        }
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = dest.with_file_name(format!(".{name}.siar-part-{}", uuid::Uuid::new_v4()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            tmp,
            dest,
            committed: false,
        })
    }
}

impl FileSink for FsFileSink {
    async fn write(&mut self, data: &[u8]) -> Result<(), FileError> {
        let file = Arc::clone(&self.file);
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || {
            use std::io::Write;
            file.lock().expect("sink file").write_all(&data)
        })
        .await
        .map_err(|_| FileError::TaskFailed)??;
        Ok(())
    }

    async fn commit(mut self) -> Result<(), FileError> {
        let file = Arc::clone(&self.file);
        let (tmp, dest) = (self.tmp.clone(), self.dest.clone());
        tokio::task::spawn_blocking(move || -> Result<(), FileError> {
            file.lock().expect("sink file").sync_all()?;
            if dest.exists() {
                return Err(FileError::DestinationExists);
            }
            std::fs::rename(&tmp, &dest)?;
            Ok(())
        })
        .await
        .map_err(|_| FileError::TaskFailed)??;
        self.committed = true;
        Ok(())
    }
}

impl Drop for FsFileSink {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-fileio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn memory_source_reads_at_offsets_and_signals_eof() {
        let v = vec![1u8, 2, 3, 4, 5];
        let mut b = [0u8; 3];
        assert_eq!(v.read_at(1, &mut b).await.unwrap(), 3);
        assert_eq!(b, [2, 3, 4]);
        assert_eq!(v.read_at(4, &mut b).await.unwrap(), 1);
        assert_eq!(v.read_at(5, &mut b).await.unwrap(), 0);
        assert_eq!(v.read_at(u64::MAX, &mut b).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn read_all_is_bounded_before_allocation() {
        let v = vec![7u8; 100];
        assert_eq!(read_all_bounded(&v, 100).await.unwrap().len(), 100);
        assert_eq!(
            read_all_bounded(&v, 99).await,
            Err(FileError::TooLarge {
                actual: 100,
                max: 99
            })
        );
        assert!(read_all_bounded(&Vec::<u8>::new(), 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn read_exact_reports_a_short_source() {
        let v = vec![1u8; 4];
        let mut b = [0u8; 8];
        assert_eq!(
            read_exact_at(&v, 0, &mut b).await,
            Err(FileError::ShortRead)
        );
    }

    #[tokio::test]
    async fn a_file_source_reads_across_piece_boundaries() {
        let d = tmpdir();
        let p = d.join("big");
        let data: Vec<u8> = (0..(READ_PIECE * 2 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&p, &data).unwrap();
        let src = FsFileSource::open(&p).unwrap();
        assert_eq!(src.len().await.unwrap(), data.len() as u64);
        assert_eq!(read_all_bounded(&src, u64::MAX).await.unwrap(), data);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn a_source_that_grows_is_refused_not_silently_truncated() {
        struct Growing;
        impl FileSource for Growing {
            async fn len(&self) -> Result<u64, FileError> {
                Ok(4)
            }
            async fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FileError> {
                // Has a 5th byte the length did not promise.
                let data = [1u8, 2, 3, 4, 5];
                let start = offset as usize;
                let n = data.len().saturating_sub(start).min(buf.len());
                buf[..n].copy_from_slice(&data[start..start + n]);
                Ok(n)
            }
        }
        assert_eq!(
            read_all_bounded(&Growing, 100).await,
            Err(FileError::Changed)
        );
    }

    #[tokio::test]
    async fn a_source_whose_fingerprint_moves_is_refused() {
        use std::sync::atomic::{AtomicU64, Ordering};
        struct Touched(AtomicU64);
        impl FileSource for Touched {
            async fn len(&self) -> Result<u64, FileError> {
                Ok(2)
            }
            async fn read_at(&self, o: u64, buf: &mut [u8]) -> Result<usize, FileError> {
                let n = (2u64.saturating_sub(o) as usize).min(buf.len());
                buf[..n].fill(9);
                Ok(n)
            }
            async fn fingerprint(&self) -> Result<SourceFingerprint, FileError> {
                Ok(SourceFingerprint {
                    size: 2,
                    mtime_millis: Some(self.0.fetch_add(1, Ordering::Relaxed)),
                    file_id: None,
                })
            }
        }
        assert_eq!(
            read_all_bounded(&Touched(AtomicU64::new(0)), 10).await,
            Err(FileError::Changed)
        );
    }

    #[test]
    fn directories_and_missing_paths_do_not_open() {
        assert_eq!(
            FsFileSource::open(std::env::temp_dir()).unwrap_err(),
            FileError::NotARegularFile
        );
        assert!(matches!(
            FsFileSource::open("/no/such/file").unwrap_err(),
            FileError::Io(_)
        ));
    }

    #[tokio::test]
    async fn a_sink_shows_nothing_until_commit_and_cleans_up_after_itself() {
        let d = tmpdir();
        let dest = d.join("out.bin");
        let mut s = FsFileSink::create(&dest).unwrap();
        s.write(b"hello ").await.unwrap();
        s.write(b"world").await.unwrap();
        assert!(!dest.exists(), "invisible before commit");
        s.commit().await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
        // Exactly one file remains: no temp left behind.
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);

        // An abandoned sink leaves nothing.
        let dest2 = d.join("never.bin");
        let mut s = FsFileSink::create(&dest2).unwrap();
        s.write(b"partial").await.unwrap();
        drop(s);
        assert!(!dest2.exists());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn a_sink_never_overwrites() {
        let d = tmpdir();
        let dest = d.join("exists");
        std::fs::write(&dest, b"mine").unwrap();
        assert_eq!(
            FsFileSink::create(&dest).unwrap_err(),
            FileError::DestinationExists
        );
        // Created after the sink was opened: commit refuses too.
        let dest2 = d.join("race");
        let mut s = FsFileSink::create(&dest2).unwrap();
        s.write(b"x").await.unwrap();
        std::fs::write(&dest2, b"appeared").unwrap();
        assert_eq!(s.commit().await, Err(FileError::DestinationExists));
        assert_eq!(std::fs::read(&dest2).unwrap(), b"appeared");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[tokio::test]
    async fn memory_sink_exposes_data_only_after_commit() {
        let mut s = MemorySink::new();
        let h = s.handle();
        s.write(b"abc").await.unwrap();
        assert!(h.lock().unwrap().is_none());
        s.commit().await.unwrap();
        assert_eq!(h.lock().unwrap().as_deref(), Some(&b"abc"[..]));
    }
}
