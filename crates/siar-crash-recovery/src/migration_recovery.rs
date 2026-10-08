//! §81 "Partial Database Migration", §82 "Migration Lock", §83
//! "Migration Backup", §84 "Event Schema vs DB Schema", §85
//! "Recovery Version Compatibility", §86 "Downgrade Compatibility".
//!
//! Pure decisions over persisted migration facts; running SQL is the
//! storage crate's job. Every outcome is deterministic: a crashed
//! migration is *resumed or rolled back*, never left to guesswork.

use crate::platform_recovery::ProcessRole;
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

// ---------------------------------------------------------------------
// §81
// ---------------------------------------------------------------------

/// Persisted next to `schema_version` (§81: "schema version, migration
/// state").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MigrationState {
    Idle,
    InProgress { from: u32, to: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationFacts {
    pub state: MigrationState,
    /// The schema version actually recorded in the database.
    pub db_schema_version: u32,
    /// The in-progress step ran inside one transaction.
    pub step_transactional: bool,
    pub backup_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationRecoveryAction {
    Nothing,
    /// The version bump committed but the marker wasn't cleared.
    MarkComplete,
    /// Nothing partial can exist (transactional): run the step again.
    RerunStep,
    /// Non-transactional step may have left partial changes: restore
    /// the pre-migration backup, then rerun.
    RollbackFromBackup,
    /// Cannot decide safely (version matches neither end, or partial
    /// destructive change with no backup).
    ManualRepairRequired,
}

pub const fn plan_migration_recovery(f: MigrationFacts) -> MigrationRecoveryAction {
    use MigrationRecoveryAction::*;
    match f.state {
        MigrationState::Idle => Nothing,
        MigrationState::InProgress { from, to } => {
            if f.db_schema_version == to {
                MarkComplete
            } else if f.db_schema_version == from {
                if f.step_transactional {
                    RerunStep
                } else if f.backup_exists {
                    RollbackFromBackup
                } else {
                    ManualRepairRequired
                }
            } else {
                ManualRepairRequired
            }
        }
    }
}

// ---------------------------------------------------------------------
// §82
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MigrationDenied {
    #[error("only the daemon may run migrations")]
    NotDaemon,
    #[error("migration lock is held elsewhere")]
    LockNotHeld,
}

/// §82: one process migrates — the daemon, holding the lock (see
/// `crate::locks::ProcessLock`).
pub const fn may_migrate(role: ProcessRole, holds_lock: bool) -> Result<(), MigrationDenied> {
    if !matches!(role, ProcessRole::Daemon) {
        return Err(MigrationDenied::NotDaemon);
    }
    if !holds_lock {
        return Err(MigrationDenied::LockNotHeld);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// §83
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupPolicy {
    /// Largest database for which a full copy is taken.
    pub full_backup_max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreMigrationBackup {
    None,
    Full,
    /// Too large to copy: a lighter checkpoint instead.
    Checkpoint,
}

/// "Backup/checkpoint according to storage size/policy", before risky
/// destructive migrations only.
pub const fn pre_migration_backup(
    destructive: bool,
    db_bytes: u64,
    p: BackupPolicy,
) -> PreMigrationBackup {
    if !destructive {
        PreMigrationBackup::None
    } else if db_bytes <= p.full_backup_max_bytes {
        PreMigrationBackup::Full
    } else {
        PreMigrationBackup::Checkpoint
    }
}

// ---------------------------------------------------------------------
// §84
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaChange {
    Index,
    ProjectionTable,
    MetadataLayout,
    /// Rewriting stored event payloads / their meaning.
    EventContent,
}

/// §84: events are immutable; a DB migration may change only
/// derived/layout structures.
pub const fn db_migration_may_change(c: SchemaChange) -> bool {
    !matches!(c, SchemaChange::EventContent)
}

// ---------------------------------------------------------------------
// §85
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCompatibility {
    Current,
    /// Older durable state the binary can migrate forward.
    NeedsMigration {
        from: u32,
    },
    /// Database is newer than this binary knows (see §86).
    TooNew,
    /// Older than the oldest version this binary still migrates —
    /// i.e. an incompatible persistence change shipped without a
    /// migration path.
    TooOld,
}

pub fn check_open_compatibility(
    db_schema: u32,
    binary_current: u32,
    migratable_from: &RangeInclusive<u32>,
) -> OpenCompatibility {
    if db_schema == binary_current {
        OpenCompatibility::Current
    } else if db_schema > binary_current {
        OpenCompatibility::TooNew
    } else if migratable_from.contains(&db_schema) {
        OpenCompatibility::NeedsMigration { from: db_schema }
    } else {
        OpenCompatibility::TooOld
    }
}

// ---------------------------------------------------------------------
// §86
// ---------------------------------------------------------------------

/// Stored in the database by the newest writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionRequirements {
    pub min_reader_version: u32,
    pub min_writer_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbAccess {
    ReadWrite,
    /// May read but must not write: the binary understands the format
    /// but not every invariant a newer writer maintains.
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("binary version {binary} is older than min_reader_version {min_reader}; unsafe downgrade rejected")]
pub struct DowngradeRejected {
    pub binary: u32,
    pub min_reader: u32,
}

pub const fn check_downgrade(
    binary_version: u32,
    r: VersionRequirements,
) -> Result<DbAccess, DowngradeRejected> {
    if binary_version < r.min_reader_version {
        Err(DowngradeRejected {
            binary: binary_version,
            min_reader: r.min_reader_version,
        })
    } else if binary_version < r.min_writer_version {
        Ok(DbAccess::ReadOnly)
    } else {
        Ok(DbAccess::ReadWrite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MigrationRecoveryAction::*;

    fn facts(state: MigrationState, v: u32, tx: bool, backup: bool) -> MigrationFacts {
        MigrationFacts {
            state,
            db_schema_version: v,
            step_transactional: tx,
            backup_exists: backup,
        }
    }

    const MID: MigrationState = MigrationState::InProgress { from: 3, to: 4 };

    #[test]
    fn idle_needs_nothing() {
        assert_eq!(
            plan_migration_recovery(facts(MigrationState::Idle, 4, true, false)),
            Nothing
        );
    }

    #[test]
    fn committed_bump_with_stale_marker_is_completed() {
        assert_eq!(
            plan_migration_recovery(facts(MID, 4, true, false)),
            MarkComplete
        );
    }

    #[test]
    fn transactional_crash_is_just_rerun() {
        assert_eq!(
            plan_migration_recovery(facts(MID, 3, true, false)),
            RerunStep
        );
    }

    #[test]
    fn non_transactional_crash_rolls_back_or_demands_repair() {
        assert_eq!(
            plan_migration_recovery(facts(MID, 3, false, true)),
            RollbackFromBackup
        );
        assert_eq!(
            plan_migration_recovery(facts(MID, 3, false, false)),
            ManualRepairRequired
        );
    }

    #[test]
    fn unexpected_version_is_never_guessed() {
        assert_eq!(
            plan_migration_recovery(facts(MID, 9, true, true)),
            ManualRepairRequired
        );
        assert_eq!(
            plan_migration_recovery(facts(MID, 2, true, true)),
            ManualRepairRequired
        );
    }

    #[test]
    fn only_the_lock_holding_daemon_migrates() {
        assert_eq!(may_migrate(ProcessRole::Daemon, true), Ok(()));
        assert_eq!(
            may_migrate(ProcessRole::Daemon, false),
            Err(MigrationDenied::LockNotHeld)
        );
        assert_eq!(
            may_migrate(ProcessRole::Gui, true),
            Err(MigrationDenied::NotDaemon)
        );
        assert_eq!(
            may_migrate(ProcessRole::Cli, true),
            Err(MigrationDenied::NotDaemon)
        );
    }

    #[test]
    fn backup_choice_follows_destructiveness_and_size() {
        let p = BackupPolicy {
            full_backup_max_bytes: 1_000,
        };
        assert_eq!(pre_migration_backup(false, 10, p), PreMigrationBackup::None);
        assert_eq!(
            pre_migration_backup(true, 1_000, p),
            PreMigrationBackup::Full
        );
        assert_eq!(
            pre_migration_backup(true, 1_001, p),
            PreMigrationBackup::Checkpoint
        );
    }

    #[test]
    fn event_content_is_immutable_but_layout_may_change() {
        assert!(!db_migration_may_change(SchemaChange::EventContent));
        for c in [
            SchemaChange::Index,
            SchemaChange::ProjectionTable,
            SchemaChange::MetadataLayout,
        ] {
            assert!(db_migration_may_change(c));
        }
    }

    #[test]
    fn open_compatibility_cases() {
        let ok = 2..=5;
        assert_eq!(
            check_open_compatibility(6, 6, &ok),
            OpenCompatibility::Current
        );
        assert_eq!(
            check_open_compatibility(4, 6, &ok),
            OpenCompatibility::NeedsMigration { from: 4 }
        );
        assert_eq!(
            check_open_compatibility(7, 6, &ok),
            OpenCompatibility::TooNew
        );
        assert_eq!(
            check_open_compatibility(1, 6, &ok),
            OpenCompatibility::TooOld
        );
    }

    #[test]
    fn downgrade_is_rejected_read_only_or_allowed() {
        let r = VersionRequirements {
            min_reader_version: 5,
            min_writer_version: 7,
        };
        assert_eq!(
            check_downgrade(4, r),
            Err(DowngradeRejected {
                binary: 4,
                min_reader: 5
            })
        );
        assert_eq!(check_downgrade(5, r), Ok(DbAccess::ReadOnly));
        assert_eq!(check_downgrade(6, r), Ok(DbAccess::ReadOnly));
        assert_eq!(check_downgrade(7, r), Ok(DbAccess::ReadWrite));
        assert_eq!(check_downgrade(9, r), Ok(DbAccess::ReadWrite));
    }
}
