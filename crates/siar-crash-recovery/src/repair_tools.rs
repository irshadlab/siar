//! §111 "Manual Repair Tools", §112 "`comm doctor`", §113 "Repair
//! Command Safety", §114 "Automatic Repair Policy".
//!
//! The decision layer behind the admin CLI. Users never hand-edit
//! SQLite: every repair is a named [`RepairAction`] with a fixed
//! authority, runs dry-run-capable, bounded, backup-aware and logged.

use crate::recovery_mode::RecoverySeverity;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §111
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminCommand {
    VerifyDatabase,
    VerifyBlobs,
    RebuildProjections,
    ListOrphanStaging,
    ExportDiagnostics,
}

impl AdminCommand {
    pub const ALL: [AdminCommand; 5] = [
        AdminCommand::VerifyDatabase,
        AdminCommand::VerifyBlobs,
        AdminCommand::RebuildProjections,
        AdminCommand::ListOrphanStaging,
        AdminCommand::ExportDiagnostics,
    ];

    /// Everything except a projection rebuild only reads.
    pub const fn is_read_only(self) -> bool {
        !matches!(self, AdminCommand::RebuildProjections)
    }
}

// ---------------------------------------------------------------------
// §112
// ---------------------------------------------------------------------

/// The seven checks `communication-node doctor` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DoctorCheck {
    DbIntegrity,
    EventProjectionLag,
    BlobStoreConsistency,
    DtnStoreConsistency,
    KeyStoreAccessible,
    SchemaVersion,
    OrphanTempFiles,
}

impl DoctorCheck {
    pub const ALL: [DoctorCheck; 7] = [
        DoctorCheck::DbIntegrity,
        DoctorCheck::EventProjectionLag,
        DoctorCheck::BlobStoreConsistency,
        DoctorCheck::DtnStoreConsistency,
        DoctorCheck::KeyStoreAccessible,
        DoctorCheck::SchemaVersion,
        DoctorCheck::OrphanTempFiles,
    ];

    /// Severity if this check *fails* outright.
    pub const fn fail_severity(self) -> RecoverySeverity {
        match self {
            DoctorCheck::DbIntegrity => RecoverySeverity::Fatal,
            DoctorCheck::KeyStoreAccessible | DoctorCheck::SchemaVersion => {
                RecoverySeverity::ReadOnly
            }
            DoctorCheck::BlobStoreConsistency | DoctorCheck::DtnStoreConsistency => {
                RecoverySeverity::Degraded
            }
            DoctorCheck::EventProjectionLag | DoctorCheck::OrphanTempFiles => {
                RecoverySeverity::Repaired
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorFinding {
    pub check: DoctorCheck,
    pub status: CheckStatus,
}

#[derive(Debug, Clone, Default)]
pub struct DoctorReport {
    findings: Vec<DoctorFinding>,
}

impl DoctorReport {
    pub fn new(findings: Vec<DoctorFinding>) -> Self {
        Self { findings }
    }

    /// Checks that did not report. A doctor run with a missing check
    /// is incomplete, never "healthy".
    pub fn missing_checks(&self) -> Vec<DoctorCheck> {
        DoctorCheck::ALL
            .iter()
            .copied()
            .filter(|c| !self.findings.iter().any(|f| f.check == *c))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.missing_checks().is_empty()
    }

    /// Worst outcome. `Warn` is capped at `Degraded`: noteworthy, but
    /// not the check's full failure severity.
    pub fn severity(&self) -> RecoverySeverity {
        self.findings
            .iter()
            .map(|f| match f.status {
                CheckStatus::Pass => RecoverySeverity::Clean,
                CheckStatus::Warn => f.check.fail_severity().min(RecoverySeverity::Degraded),
                CheckStatus::Fail => f.check.fail_severity(),
            })
            .max()
            .unwrap_or(RecoverySeverity::Clean)
    }
}

// ---------------------------------------------------------------------
// §114 (defined first: §113 gates on it)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepairAction {
    RebuildCache,
    RebuildProjection,
    DeleteExpiredTemp,
    ReverifyPartialChunk,
    DropCorruptAuthoritativeEvent,
    ResetIdentityChain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairAuthority {
    /// Safe to run without asking.
    AutomaticSafe,
    /// Only on explicit administrator command.
    ManualAdmin,
}

pub const fn repair_authority(a: RepairAction) -> RepairAuthority {
    match a {
        RepairAction::RebuildCache
        | RepairAction::RebuildProjection
        | RepairAction::DeleteExpiredTemp
        | RepairAction::ReverifyPartialChunk => RepairAuthority::AutomaticSafe,
        RepairAction::DropCorruptAuthoritativeEvent | RepairAction::ResetIdentityChain => {
            RepairAuthority::ManualAdmin
        }
    }
}

pub const fn may_run_automatically(a: RepairAction) -> bool {
    matches!(repair_authority(a), RepairAuthority::AutomaticSafe)
}

// ---------------------------------------------------------------------
// §113
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairRequest {
    pub dry_run: bool,
    /// Required: an unbounded repair is refused.
    pub max_items: Option<usize>,
    pub backup_exists: bool,
    /// Whether an administrator explicitly issued this command.
    pub admin_initiated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RepairRefusal {
    #[error("repair must specify a bound on items touched")]
    Unbounded,
    #[error("{0:?} requires explicit administrator action")]
    AdminOnly(RepairAction),
    #[error("{0:?} is destructive and needs a verified backup first")]
    BackupRequired(RepairAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairMode {
    DryRun,
    Apply { max_items: usize },
}

/// Gate for every repair: bounded; manual-authority actions need an
/// admin *and* a backup (a dry run needs neither — it changes
/// nothing).
pub const fn authorize_repair(
    action: RepairAction,
    req: &RepairRequest,
) -> Result<RepairMode, RepairRefusal> {
    let Some(max) = req.max_items else {
        return Err(RepairRefusal::Unbounded);
    };
    if req.dry_run {
        return Ok(RepairMode::DryRun);
    }
    if matches!(repair_authority(action), RepairAuthority::ManualAdmin) {
        if !req.admin_initiated {
            return Err(RepairRefusal::AdminOnly(action));
        }
        if !req.backup_exists {
            return Err(RepairRefusal::BackupRequired(action));
        }
    }
    Ok(RepairMode::Apply { max_items: max })
}

/// One line of the mandatory repair log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairLogEntry {
    pub action: RepairAction,
    pub items_considered: usize,
    pub items_applied: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RepairLog {
    pub entries: Vec<RepairLogEntry>,
}

/// Decides how many of `item_count` items to touch and records it —
/// always, including for dry runs and refusals' counterparts (a
/// refusal is returned to the caller, who logs it as an error).
pub fn plan_repair(
    action: RepairAction,
    item_count: usize,
    req: &RepairRequest,
    log: &mut RepairLog,
) -> Result<usize, RepairRefusal> {
    let mode = authorize_repair(action, req)?;
    let (applied, dry) = match mode {
        RepairMode::DryRun => (0, true),
        RepairMode::Apply { max_items } => (item_count.min(max_items), false),
    };
    log.entries.push(RepairLogEntry {
        action,
        items_considered: item_count,
        items_applied: applied,
        dry_run: dry,
    });
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_projection_rebuild_mutates_among_the_admin_commands() {
        let mutating: Vec<_> = AdminCommand::ALL
            .iter()
            .filter(|c| !c.is_read_only())
            .collect();
        assert_eq!(mutating, vec![&AdminCommand::RebuildProjections]);
        assert_eq!(AdminCommand::ALL.len(), 5);
    }

    fn all_pass() -> Vec<DoctorFinding> {
        DoctorCheck::ALL
            .iter()
            .map(|c| DoctorFinding {
                check: *c,
                status: CheckStatus::Pass,
            })
            .collect()
    }

    #[test]
    fn doctor_runs_exactly_the_seven_spec_checks() {
        assert_eq!(DoctorCheck::ALL.len(), 7);
        let r = DoctorReport::new(all_pass());
        assert!(r.is_complete());
        assert_eq!(r.severity(), RecoverySeverity::Clean);
    }

    #[test]
    fn missing_checks_make_the_report_incomplete() {
        let mut f = all_pass();
        f.retain(|x| x.check != DoctorCheck::KeyStoreAccessible);
        let r = DoctorReport::new(f);
        assert!(!r.is_complete());
        assert_eq!(r.missing_checks(), vec![DoctorCheck::KeyStoreAccessible]);
    }

    #[test]
    fn doctor_severity_is_the_worst_failing_check() {
        let mut f = all_pass();
        f[4].status = CheckStatus::Fail; // key store
        f[6].status = CheckStatus::Fail; // orphan temp
        assert_eq!(
            DoctorReport::new(f.clone()).severity(),
            RecoverySeverity::ReadOnly
        );
        f[0].status = CheckStatus::Fail; // db integrity
        assert_eq!(DoctorReport::new(f).severity(), RecoverySeverity::Fatal);
    }

    #[test]
    fn warnings_never_exceed_degraded() {
        let mut f = all_pass();
        f[0].status = CheckStatus::Warn; // db integrity warn
        assert_eq!(DoctorReport::new(f).severity(), RecoverySeverity::Degraded);
    }

    #[test]
    fn spec_automatic_vs_manual_split() {
        use RepairAction::*;
        for a in [
            RebuildCache,
            RebuildProjection,
            DeleteExpiredTemp,
            ReverifyPartialChunk,
        ] {
            assert!(may_run_automatically(a), "{a:?}");
        }
        for a in [DropCorruptAuthoritativeEvent, ResetIdentityChain] {
            assert!(!may_run_automatically(a), "{a:?}");
        }
    }

    fn req(dry: bool, max: Option<usize>, backup: bool, admin: bool) -> RepairRequest {
        RepairRequest {
            dry_run: dry,
            max_items: max,
            backup_exists: backup,
            admin_initiated: admin,
        }
    }

    #[test]
    fn unbounded_repair_is_always_refused() {
        assert_eq!(
            authorize_repair(RepairAction::RebuildCache, &req(false, None, true, true)),
            Err(RepairRefusal::Unbounded)
        );
        assert_eq!(
            authorize_repair(RepairAction::RebuildCache, &req(true, None, true, true)),
            Err(RepairRefusal::Unbounded)
        );
    }

    #[test]
    fn destructive_actions_need_admin_and_backup() {
        let a = RepairAction::DropCorruptAuthoritativeEvent;
        assert_eq!(
            authorize_repair(a, &req(false, Some(1), true, false)),
            Err(RepairRefusal::AdminOnly(a))
        );
        assert_eq!(
            authorize_repair(a, &req(false, Some(1), false, true)),
            Err(RepairRefusal::BackupRequired(a))
        );
        assert_eq!(
            authorize_repair(a, &req(false, Some(1), true, true)),
            Ok(RepairMode::Apply { max_items: 1 })
        );
    }

    #[test]
    fn dry_run_needs_neither_backup_nor_admin() {
        assert_eq!(
            authorize_repair(
                RepairAction::ResetIdentityChain,
                &req(true, Some(5), false, false)
            ),
            Ok(RepairMode::DryRun)
        );
    }

    #[test]
    fn automatic_repairs_run_without_a_backup() {
        assert_eq!(
            authorize_repair(
                RepairAction::RebuildProjection,
                &req(false, Some(100), false, false)
            ),
            Ok(RepairMode::Apply { max_items: 100 })
        );
    }

    #[test]
    fn plan_repair_is_bounded_logged_and_dry_run_applies_nothing() {
        let mut log = RepairLog::default();
        let n = plan_repair(
            RepairAction::DeleteExpiredTemp,
            500,
            &req(false, Some(50), false, false),
            &mut log,
        )
        .unwrap();
        assert_eq!(n, 50);
        let d = plan_repair(
            RepairAction::DeleteExpiredTemp,
            500,
            &req(true, Some(50), false, false),
            &mut log,
        )
        .unwrap();
        assert_eq!(d, 0);
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[0].items_applied, 50);
        assert!(log.entries[1].dry_run && log.entries[1].items_considered == 500);
    }

    #[test]
    fn refused_repairs_are_not_logged_as_applied() {
        let mut log = RepairLog::default();
        assert!(plan_repair(
            RepairAction::ResetIdentityChain,
            1,
            &req(false, Some(1), false, true),
            &mut log
        )
        .is_err());
        assert!(log.entries.is_empty());
    }
}
