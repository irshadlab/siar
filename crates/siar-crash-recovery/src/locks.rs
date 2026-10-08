//! §90 "Stale Locks".
//!
//! Preferred: an OS advisory lock held through an open file handle
//! ([`ProcessLock`]). The kernel releases it when the process dies by
//! any means, so there is nothing stale to recover. If a lock *file*
//! with recorded owner data must be used, [`lock_is_stale`] encodes
//! the spec's warning that a PID alone is insufficient (PIDs are
//! reused): the owner's process start time must match too, and an
//! undeterminable owner is never treated as stale.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("lock is held by another process")]
    Held,
    #[error("lock I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Exclusive, non-blocking OS file lock. Released on drop and — the
/// point — automatically if the process is killed.
#[derive(Debug)]
pub struct ProcessLock {
    file: File,
}

impl ProcessLock {
    pub fn try_acquire(path: &Path) -> Result<Self, LockError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held),
            Err(TryLockError::Error(e)) => Err(LockError::Io(e)),
        }
    }

    /// Explicit early release; dropping does the same.
    pub fn release(self) -> io::Result<()> {
        self.file.unlock()
    }
}

/// Owner data recorded in a lock *file* (the fallback design).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockOwner {
    pub pid: u32,
    /// Process start time as the OS reports it (any monotone-stable
    /// unit, compared only for equality).
    pub process_start_time: u64,
}

/// What a liveness probe of `LockOwner::pid` found *now*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessProbe {
    NotRunning,
    Running {
        start_time: u64,
    },
    /// Probe failed or is unsupported.
    Unknown,
}

/// Stale iff the recorded process is gone, or the PID now belongs to a
/// *different* process (start time differs → PID reuse). `Unknown` is
/// not stale: stealing a live lock is worse than waiting.
pub const fn lock_is_stale(recorded: LockOwner, probe: ProcessProbe) -> bool {
    match probe {
        ProcessProbe::NotRunning => true,
        ProcessProbe::Running { start_time } => start_time != recorded.process_start_time,
        ProcessProbe::Unknown => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "siar-lock-{}-{}-{}",
            name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn second_acquire_is_refused_until_the_first_is_dropped() {
        let p = tmp("excl");
        let a = ProcessLock::try_acquire(&p).unwrap();
        assert!(matches!(ProcessLock::try_acquire(&p), Err(LockError::Held)));
        drop(a);
        let b = ProcessLock::try_acquire(&p).unwrap();
        b.release().unwrap();
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn leftover_lock_file_does_not_block_a_new_owner() {
        // A crashed process leaves the *file* behind but not the lock.
        let p = tmp("left");
        std::fs::write(&p, b"pid=1234").unwrap();
        assert!(ProcessLock::try_acquire(&p).is_ok());
        std::fs::remove_file(&p).ok();
    }

    const OWNER: LockOwner = LockOwner {
        pid: 4242,
        process_start_time: 1_000,
    };

    #[test]
    fn dead_owner_is_stale() {
        assert!(lock_is_stale(OWNER, ProcessProbe::NotRunning));
    }

    #[test]
    fn same_pid_different_start_time_is_pid_reuse_and_stale() {
        assert!(lock_is_stale(
            OWNER,
            ProcessProbe::Running { start_time: 2_000 }
        ));
    }

    #[test]
    fn same_pid_same_start_time_is_a_live_owner() {
        assert!(!lock_is_stale(
            OWNER,
            ProcessProbe::Running { start_time: 1_000 }
        ));
    }

    #[test]
    fn undeterminable_owner_is_never_stolen_from() {
        assert!(!lock_is_stale(OWNER, ProcessProbe::Unknown));
    }
}
