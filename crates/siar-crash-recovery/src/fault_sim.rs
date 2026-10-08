//! §123 "Power-Loss Approximation", §124 "Database Fault Injection",
//! §126 "File Fault Injection".
//!
//! [`SimFs`] is a tiny deterministic in-memory filesystem that models
//! the part of POSIX durability that matters for crash recovery: file
//! *contents* and directory *entries* each become durable only at
//! their own fsync. [`SimFs::power_loss`] throws away everything that
//! was not durable (optionally keeping a prefix of unsynced bytes, the
//! "torn write" approximation). Because it is pure and seedless it can
//! cut power after *every* operation of a sequence — something real
//! power-pulling cannot do — which is how the tests below prove the
//! temp-write → fsync → rename → fsync-dir publish order that
//! `atomic_rename::publish_atomic` relies on, and show what breaks
//! when the order is wrong.
//!
//! It approximates; it does not replace running on target filesystems
//! where practical (§123).

use crate::corruption::{response_for, CorruptionScope};
use crate::fault_policy::{disk_full_response, DiskFullStage};
use crate::recovery_mode::RecoverySeverity;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsFault {
    /// The next append accepts only `accept` bytes and reports it.
    ShortWrite {
        accept: usize,
    },
    /// The next append writes `bytes` then fails with an I/O error.
    FailAfter {
        bytes: usize,
    },
    DiskFull,
    RenameFails,
    PermissionDenied,
    /// The source file vanishes just before the next rename.
    SourceDisappears,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FsError {
    #[error("no space left")]
    DiskFull,
    #[error("I/O error")]
    Io,
    #[error("permission denied")]
    PermissionDenied,
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, Default)]
struct SimFile {
    durable: Vec<u8>,
    pending: Vec<u8>,
}

impl SimFile {
    fn len(&self) -> usize {
        self.durable.len() + self.pending.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SimFs {
    files: Vec<SimFile>,
    names: BTreeMap<String, usize>,
    durable_names: BTreeMap<String, usize>,
    capacity: Option<usize>,
    fault: Option<FsFault>,
}

impl SimFs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            capacity: Some(bytes),
            ..Self::default()
        }
    }

    /// One-shot: consumed by the next operation it applies to.
    pub fn inject(&mut self, f: FsFault) {
        self.fault = Some(f);
    }

    fn used(&self) -> usize {
        self.files.iter().map(SimFile::len).sum()
    }

    pub fn create(&mut self, name: &str) -> Result<(), FsError> {
        if matches!(self.fault, Some(FsFault::PermissionDenied)) {
            self.fault = None;
            return Err(FsError::PermissionDenied);
        }
        self.files.push(SimFile::default());
        self.names.insert(name.to_owned(), self.files.len() - 1);
        Ok(())
    }

    /// Appends; returns how many bytes were actually accepted (which
    /// may be fewer than `data.len()` — a short write).
    pub fn append(&mut self, name: &str, data: &[u8]) -> Result<usize, FsError> {
        let id = *self.names.get(name).ok_or(FsError::NotFound)?;
        let mut take = data.len();
        let mut err = None;
        match self.fault {
            Some(FsFault::ShortWrite { accept }) => {
                take = take.min(accept);
                self.fault = None;
            }
            Some(FsFault::FailAfter { bytes }) => {
                take = take.min(bytes);
                err = Some(FsError::Io);
                self.fault = None;
            }
            Some(FsFault::DiskFull) => {
                self.fault = None;
                return Err(FsError::DiskFull);
            }
            Some(FsFault::PermissionDenied) => {
                self.fault = None;
                return Err(FsError::PermissionDenied);
            }
            _ => {}
        }
        if let Some(cap) = self.capacity {
            let room = cap.saturating_sub(self.used());
            if room == 0 && take > 0 {
                return Err(FsError::DiskFull);
            }
            take = take.min(room);
        }
        self.files[id].pending.extend_from_slice(&data[..take]);
        match err {
            Some(e) => Err(e),
            None => Ok(take),
        }
    }

    /// Makes this file's current contents durable.
    pub fn fsync(&mut self, name: &str) -> Result<(), FsError> {
        let id = *self.names.get(name).ok_or(FsError::NotFound)?;
        let f = &mut self.files[id];
        let p = std::mem::take(&mut f.pending);
        f.durable.extend(p);
        Ok(())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        match self.fault {
            Some(FsFault::RenameFails) => {
                self.fault = None;
                return Err(FsError::Io);
            }
            Some(FsFault::PermissionDenied) => {
                self.fault = None;
                return Err(FsError::PermissionDenied);
            }
            Some(FsFault::SourceDisappears) => {
                self.fault = None;
                self.names.remove(from);
            }
            _ => {}
        }
        let id = self.names.remove(from).ok_or(FsError::NotFound)?;
        self.names.insert(to.to_owned(), id);
        Ok(())
    }

    /// Makes the current directory entries durable.
    pub fn fsync_dir(&mut self) {
        self.durable_names = self.names.clone();
    }

    /// What a reader sees right now (durable + not-yet-durable).
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let f = &self.files[*self.names.get(name)?];
        let mut v = f.durable.clone();
        v.extend_from_slice(&f.pending);
        Some(v)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    /// Cuts power. Directory entries revert to the last `fsync_dir`;
    /// each file keeps its durable bytes plus the first `torn_keep`
    /// bytes of whatever was unsynced (0 = pessimistic, larger =
    /// torn-write approximation).
    pub fn power_loss(&mut self, torn_keep: usize) {
        self.names = self.durable_names.clone();
        for f in &mut self.files {
            let keep = torn_keep.min(f.pending.len());
            let kept: Vec<u8> = f.pending.drain(..keep).collect();
            f.durable.extend(kept);
            f.pending.clear();
        }
    }
}

/// Correct short-write handling: loop until everything is accepted,
/// and treat zero progress as an error rather than spinning.
pub fn write_all(fs: &mut SimFs, name: &str, mut data: &[u8]) -> Result<(), FsError> {
    while !data.is_empty() {
        let n = fs.append(name, data)?;
        if n == 0 {
            return Err(FsError::Io);
        }
        data = &data[n..];
    }
    Ok(())
}

/// The publish order `atomic_rename::publish_atomic` is built on.
pub fn publish_correct(fs: &mut SimFs, tmp: &str, dest: &str, data: &[u8]) -> Result<(), FsError> {
    fs.create(tmp)?;
    write_all(fs, tmp, data)?;
    fs.fsync(tmp)?;
    fs.rename(tmp, dest)?;
    fs.fsync_dir();
    Ok(())
}

/// Deliberately wrong: renames before the data is durable.
pub fn publish_missing_data_fsync(
    fs: &mut SimFs,
    tmp: &str,
    dest: &str,
    data: &[u8],
) -> Result<(), FsError> {
    fs.create(tmp)?;
    write_all(fs, tmp, data)?;
    fs.rename(tmp, dest)?;
    fs.fsync_dir();
    Ok(())
}

// ---------------------------------------------------------------------
// §124
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFault {
    Busy,
    IoError,
    DiskFull,
    /// The scope tells recovery what was corrupted (§52).
    CorruptPage(CorruptionScope),
    MigrationFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFaultAction {
    RetryWithBackoff,
    /// Do not accept more writes; surface the error and keep state.
    SurfaceErrorKeepState,
    FreeSpaceThenUserRetry,
    /// Hand to §52's per-scope handling.
    TreatAsCorruption(CorruptionScope),
    /// Hand to §81's migration recovery.
    ReconcileMigration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbFaultResponse {
    pub action: DbFaultAction,
    pub severity: RecoverySeverity,
    /// §78: false means "never tell the user it was accepted".
    pub may_claim_accepted: bool,
}

pub fn classify_db_fault(f: DbFault) -> DbFaultResponse {
    match f {
        DbFault::Busy => DbFaultResponse {
            action: DbFaultAction::RetryWithBackoff,
            severity: RecoverySeverity::Clean,
            may_claim_accepted: false,
        },
        DbFault::IoError => DbFaultResponse {
            action: DbFaultAction::SurfaceErrorKeepState,
            severity: RecoverySeverity::ReadOnly,
            may_claim_accepted: false,
        },
        DbFault::DiskFull => {
            let o = disk_full_response(DiskFullStage::DbTransaction);
            DbFaultResponse {
                action: DbFaultAction::FreeSpaceThenUserRetry,
                severity: RecoverySeverity::Degraded,
                may_claim_accepted: o.operation_accepted,
            }
        }
        DbFault::CorruptPage(scope) => DbFaultResponse {
            action: DbFaultAction::TreatAsCorruption(scope),
            severity: response_for(scope).severity,
            may_claim_accepted: false,
        },
        DbFault::MigrationFailure => DbFaultResponse {
            action: DbFaultAction::ReconcileMigration,
            severity: RecoverySeverity::ReadOnly,
            may_claim_accepted: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &[u8] = b"0123456789abcdef";

    #[test]
    fn unsynced_data_is_lost_on_power_loss_and_synced_data_survives() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.fsync_dir();
        fs.append("f", b"abc").unwrap();
        fs.fsync("f").unwrap();
        fs.append("f", b"def").unwrap();
        assert_eq!(fs.read("f").unwrap(), b"abcdef");
        fs.power_loss(0);
        assert_eq!(fs.read("f").unwrap(), b"abc");
    }

    #[test]
    fn torn_write_keeps_a_prefix_of_unsynced_bytes() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.fsync_dir();
        fs.append("f", b"abcdef").unwrap();
        fs.power_loss(2);
        assert_eq!(fs.read("f").unwrap(), b"ab");
    }

    #[test]
    fn unsynced_directory_entry_vanishes_with_power() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.append("f", b"x").unwrap();
        fs.fsync("f").unwrap();
        fs.power_loss(0);
        assert!(!fs.exists("f"), "name was never made durable");
    }

    #[test]
    fn correct_publish_order_is_atomic_at_every_power_loss_point() {
        // Cut power after each of the 5 steps; the destination must be
        // either absent or complete — never partial.
        for cut in 0..=5 {
            for torn in [0usize, 3, 1_000] {
                let mut fs = SimFs::new();
                // establish an old version
                publish_correct(&mut fs, "old.tmp", "dest", b"OLD").unwrap();

                let steps: [&dyn Fn(&mut SimFs); 5] = [
                    &|f| f.create("new.tmp").unwrap(),
                    &|f| write_all(f, "new.tmp", DATA).unwrap(),
                    &|f| f.fsync("new.tmp").unwrap(),
                    &|f| f.rename("new.tmp", "dest").unwrap(),
                    &|f| f.fsync_dir(),
                ];
                for s in steps.iter().take(cut) {
                    s(&mut fs);
                }
                fs.power_loss(torn);
                let got = fs.read("dest").expect("dest must still exist");
                assert!(
                    got == b"OLD" || got == DATA,
                    "cut={cut} torn={torn}: dest = {got:?}"
                );
            }
        }
    }

    #[test]
    fn skipping_the_data_fsync_can_publish_an_empty_file() {
        let mut fs = SimFs::new();
        publish_missing_data_fsync(&mut fs, "t", "dest", DATA).unwrap();
        fs.power_loss(0);
        assert_eq!(
            fs.read("dest").unwrap(),
            b"",
            "name durable, contents not: the classic bug"
        );
    }

    #[test]
    fn short_write_is_handled_by_looping() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.inject(FsFault::ShortWrite { accept: 4 });
        write_all(&mut fs, "f", DATA).unwrap();
        assert_eq!(fs.read("f").unwrap(), DATA);
    }

    #[test]
    fn ignoring_a_short_write_loses_data_silently() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.inject(FsFault::ShortWrite { accept: 4 });
        let n = fs.append("f", DATA).unwrap();
        assert_eq!(n, 4);
        assert_eq!(
            fs.read("f").unwrap().len(),
            4,
            "naive caller kept only a prefix"
        );
    }

    #[test]
    fn partial_write_then_error_leaves_a_prefix_and_reports_failure() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.inject(FsFault::FailAfter { bytes: 5 });
        assert_eq!(write_all(&mut fs, "f", DATA), Err(FsError::Io));
        assert_eq!(fs.read("f").unwrap(), &DATA[..5]);
    }

    #[test]
    fn failed_publish_never_touches_the_destination() {
        for fault in [
            FsFault::RenameFails,
            FsFault::PermissionDenied,
            FsFault::SourceDisappears,
        ] {
            let mut fs = SimFs::new();
            publish_correct(&mut fs, "o", "dest", b"OLD").unwrap();
            fs.create("n").unwrap();
            write_all(&mut fs, "n", DATA).unwrap();
            fs.fsync("n").unwrap();
            fs.inject(fault);
            assert!(fs.rename("n", "dest").is_err(), "{fault:?}");
            assert_eq!(fs.read("dest").unwrap(), b"OLD", "{fault:?}");
        }
    }

    #[test]
    fn source_disappearing_is_not_found() {
        let mut fs = SimFs::new();
        fs.create("n").unwrap();
        fs.inject(FsFault::SourceDisappears);
        assert_eq!(fs.rename("n", "dest"), Err(FsError::NotFound));
    }

    #[test]
    fn disk_full_fault_and_real_capacity_both_refuse_writes() {
        let mut fs = SimFs::new();
        fs.create("f").unwrap();
        fs.inject(FsFault::DiskFull);
        assert_eq!(fs.append("f", b"x"), Err(FsError::DiskFull));

        let mut small = SimFs::with_capacity(4);
        small.create("f").unwrap();
        assert_eq!(write_all(&mut small, "f", DATA), Err(FsError::DiskFull));
        assert_eq!(small.read("f").unwrap().len(), 4);
    }

    #[test]
    fn db_faults_never_claim_acceptance() {
        for f in [
            DbFault::Busy,
            DbFault::IoError,
            DbFault::DiskFull,
            DbFault::CorruptPage(CorruptionScope::OneProjection),
            DbFault::CorruptPage(CorruptionScope::EntireDb),
            DbFault::MigrationFailure,
        ] {
            assert!(!classify_db_fault(f).may_claim_accepted, "{f:?}");
        }
    }

    #[test]
    fn db_fault_responses_match_their_nature() {
        assert_eq!(
            classify_db_fault(DbFault::Busy).action,
            DbFaultAction::RetryWithBackoff
        );
        assert_eq!(
            classify_db_fault(DbFault::Busy).severity,
            RecoverySeverity::Clean
        );
        assert_eq!(
            classify_db_fault(DbFault::DiskFull).action,
            DbFaultAction::FreeSpaceThenUserRetry
        );
        assert_eq!(
            classify_db_fault(DbFault::IoError).severity,
            RecoverySeverity::ReadOnly
        );
        assert_eq!(
            classify_db_fault(DbFault::MigrationFailure).action,
            DbFaultAction::ReconcileMigration
        );
    }

    #[test]
    fn corrupt_page_severity_depends_on_what_was_corrupted() {
        let proj = classify_db_fault(DbFault::CorruptPage(CorruptionScope::OneProjection));
        let whole = classify_db_fault(DbFault::CorruptPage(CorruptionScope::EntireDb));
        assert!(proj.severity < RecoverySeverity::ReadOnly);
        assert_eq!(whole.severity, RecoverySeverity::Fatal);
    }
}
