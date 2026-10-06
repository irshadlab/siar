//! §22 "Atomic Rename".
//!
//! §22 has three rules: use atomic rename when the filesystem supports
//! it; never assume atomicity across filesystems/mounts; keep staging
//! and the final store on the same filesystem when atomic rename is
//! required. This module makes the second and third rules a *check
//! that must pass before the rename is attempted*, so a misconfigured
//! staging directory is refused up front instead of silently degrading
//! into a non-atomic copy-then-delete.
//!
//! The decision logic ([`rename_viability`]) is pure and takes opaque
//! [`FilesystemId`]s so it is testable without real mounts.
//! [`filesystem_id_of`] and [`publish_atomic`] are the thin real-OS
//! layer; on platforms where a filesystem identity cannot be obtained
//! the answer is [`RenameViability::Unknown`], which is refused —
//! §22's "do not assume" applied literally.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Opaque identity of the filesystem/mount a path lives on (the
/// `st_dev` value on Unix). Equality is the only operation that means
/// anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FilesystemId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameViability {
    /// Both paths are known to be on the same filesystem.
    Atomic,
    /// Both identities are known and differ — a rename would be a
    /// copy+delete, which is not atomic.
    CrossFilesystem {
        staging: FilesystemId,
        final_store: FilesystemId,
    },
    /// At least one identity could not be determined. Not assumed
    /// safe.
    Unknown,
}

impl RenameViability {
    #[inline]
    pub const fn is_atomic(self) -> bool {
        matches!(self, RenameViability::Atomic)
    }
}

/// Pure decision for §22's rules 2 and 3.
#[inline]
pub const fn rename_viability(
    staging: Option<FilesystemId>,
    final_store: Option<FilesystemId>,
) -> RenameViability {
    match (staging, final_store) {
        (Some(a), Some(b)) if a.0 == b.0 => RenameViability::Atomic,
        (Some(a), Some(b)) => RenameViability::CrossFilesystem {
            staging: a,
            final_store: b,
        },
        _ => RenameViability::Unknown,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("atomic rename refused: {0:?}")]
    NotAtomic(RenameViability),
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
}

/// Filesystem identity of `path` (Unix `st_dev`); `None` where the
/// platform offers no such notion or the path cannot be inspected.
pub fn filesystem_id_of(path: &Path) -> Option<FilesystemId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| FilesystemId(m.dev()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Atomically publishes `staged` as `final_path`.
///
/// Refuses unless both live on the same known filesystem (the
/// destination's *parent directory* is inspected, since the
/// destination itself usually does not exist yet). After the rename
/// the parent directory is fsynced on Unix so the rename itself is
/// durable, not only visible.
pub fn publish_atomic(staged: &Path, final_path: &Path) -> Result<(), PublishError> {
    let parent = final_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let v = rename_viability(filesystem_id_of(staged), filesystem_id_of(parent));
    if !v.is_atomic() {
        return Err(PublishError::NotAtomic(v));
    }
    std::fs::rename(staged, final_path)?;
    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_filesystem_is_atomic() {
        let v = rename_viability(Some(FilesystemId(7)), Some(FilesystemId(7)));
        assert!(v.is_atomic());
    }

    #[test]
    fn different_filesystems_are_refused() {
        let v = rename_viability(Some(FilesystemId(1)), Some(FilesystemId(2)));
        assert_eq!(
            v,
            RenameViability::CrossFilesystem {
                staging: FilesystemId(1),
                final_store: FilesystemId(2)
            }
        );
        assert!(!v.is_atomic());
    }

    #[test]
    fn unknown_identity_is_never_assumed_atomic() {
        assert_eq!(
            rename_viability(None, Some(FilesystemId(1))),
            RenameViability::Unknown
        );
        assert_eq!(
            rename_viability(Some(FilesystemId(1)), None),
            RenameViability::Unknown
        );
        assert_eq!(rename_viability(None, None), RenameViability::Unknown);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "siar-atomic-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn publish_moves_file_on_same_filesystem() {
        let d = scratch("ok");
        let staged = d.join("staging-1");
        let dest = d.join("final-1");
        std::fs::write(&staged, b"payload").unwrap();
        publish_atomic(&staged, &dest).unwrap();
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn publish_of_missing_source_is_refused_not_assumed() {
        let d = scratch("missing");
        let err = publish_atomic(&d.join("nope"), &d.join("dest")).unwrap_err();
        // Unknown identity for the source → refused before any rename.
        assert!(matches!(
            err,
            PublishError::NotAtomic(RenameViability::Unknown)
        ));
        std::fs::remove_dir_all(&d).unwrap();
    }
}
