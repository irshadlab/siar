//! §76 "Process-Wide Panic Strategy", §77 "OOM Considerations", §78
//! "Disk Full During Transaction", §79 "Disk Full During Blob
//! Write", §80 "Disk Full During Finalization".
//!
//! §76-77 are about what the *next* start may assume. §78-80 share one
//! rule — never claim progress that did not reach durable storage —
//! expressed as [`disk_full_response`], one decision per stage.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §76
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationScope {
    /// Only one worker's private state is affected.
    WorkerLocal,
    /// Shared in-memory state other tasks rely on may be inconsistent.
    SharedMutableState,
    /// Continuing could write inconsistent data to durable storage.
    DurableWriteRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicStrategy {
    /// Supervisor (§73-74) restarts just the worker.
    RestartWorker,
    /// Abort the process; persistent recovery restores from durable
    /// state on the next start.
    AbortProcess,
}

/// §76: for an unrecoverable invariant violation, abort/restart is
/// safer than continuing on corrupted shared state.
pub const fn strategy_for(scope: ViolationScope) -> PanicStrategy {
    match scope {
        ViolationScope::WorkerLocal => PanicStrategy::RestartWorker,
        ViolationScope::SharedMutableState | ViolationScope::DurableWriteRisk => {
            PanicStrategy::AbortProcess
        }
    }
}

/// Immediate abort with a diagnostic on stderr. Skips unwinding and
/// destructors on purpose: after an invariant violation, running
/// cleanup code over suspect state is the thing to avoid. Not unit
/// tested for the obvious reason that it ends the process.
pub fn fail_fast(reason: &str) -> ! {
    eprintln!("siar: fatal invariant violation, aborting: {reason}");
    std::process::abort()
}

// ---------------------------------------------------------------------
// §77
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    Clean,
    Crash,
    /// Killed for memory (OOM killer, allocation failure abort).
    OomKilled,
}

/// §77: only a clean exit runs cleanup. An OOM exit is a crash — "no
/// cleanup happened".
pub const fn cleanup_ran(e: ExitKind) -> bool {
    matches!(e, ExitKind::Clean)
}

pub const fn recovery_required(e: ExitKind) -> bool {
    !cleanup_ran(e)
}

// ---------------------------------------------------------------------
// §78 - §80
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiskFullStage {
    DbTransaction,
    BlobWrite,
    Finalization,
}

/// Things recovery may do to make room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpaceAction {
    EvictCache,
    FreeTemp,
    RetryOnUserRequest,
}

/// Where a transfer sits after a disk-full during finalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStorageState {
    /// Unchanged — the stage never started durably.
    Unchanged,
    /// §80: stays `Finalizing`.
    Finalizing,
    /// §80: or explicitly failed for storage; either way the user can
    /// act (free space, retry).
    FailedStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskFullOutcome {
    /// §78: false means the caller must tell the user it was *not*
    /// accepted.
    pub operation_accepted: bool,
    /// §79: whether the chunk/progress may be recorded as done.
    pub progress_may_be_recorded: bool,
    pub transfer_state: TransferStorageState,
    pub user_action_available: bool,
    pub space_actions: Vec<SpaceAction>,
}

pub fn disk_full_response(stage: DiskFullStage) -> DiskFullOutcome {
    let space_actions = vec![
        SpaceAction::EvictCache,
        SpaceAction::FreeTemp,
        SpaceAction::RetryOnUserRequest,
    ];
    match stage {
        DiskFullStage::DbTransaction => DiskFullOutcome {
            operation_accepted: false,
            progress_may_be_recorded: false,
            transfer_state: TransferStorageState::Unchanged,
            user_action_available: true,
            space_actions,
        },
        DiskFullStage::BlobWrite => DiskFullOutcome {
            operation_accepted: false,
            progress_may_be_recorded: false,
            transfer_state: TransferStorageState::Unchanged,
            user_action_available: true,
            space_actions,
        },
        DiskFullStage::Finalization => DiskFullOutcome {
            operation_accepted: false,
            progress_may_be_recorded: false,
            transfer_state: TransferStorageState::FailedStorage,
            user_action_available: true,
            space_actions,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkWriteResult {
    /// Written *and* flushed durably.
    Durable,
    DiskFull,
    OtherError,
}

/// §79: a chunk is complete only after its write is durable.
pub const fn chunk_may_be_marked_complete(r: ChunkWriteResult) -> bool {
    matches!(r, ChunkWriteResult::Durable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_worker_local_violations_keep_the_process_alive() {
        assert_eq!(
            strategy_for(ViolationScope::WorkerLocal),
            PanicStrategy::RestartWorker
        );
        assert_eq!(
            strategy_for(ViolationScope::SharedMutableState),
            PanicStrategy::AbortProcess
        );
        assert_eq!(
            strategy_for(ViolationScope::DurableWriteRisk),
            PanicStrategy::AbortProcess
        );
    }

    #[test]
    fn oom_is_treated_as_a_crash_with_no_cleanup() {
        assert!(cleanup_ran(ExitKind::Clean));
        assert!(!cleanup_ran(ExitKind::OomKilled));
        assert!(!cleanup_ran(ExitKind::Crash));
        assert!(recovery_required(ExitKind::OomKilled));
        assert!(!recovery_required(ExitKind::Clean));
    }

    #[test]
    fn disk_full_never_claims_acceptance_or_progress() {
        for s in [
            DiskFullStage::DbTransaction,
            DiskFullStage::BlobWrite,
            DiskFullStage::Finalization,
        ] {
            let o = disk_full_response(s);
            assert!(!o.operation_accepted, "{s:?}");
            assert!(!o.progress_may_be_recorded, "{s:?}");
            assert!(o.user_action_available, "{s:?}");
            assert!(o.space_actions.contains(&SpaceAction::RetryOnUserRequest));
        }
    }

    #[test]
    fn finalization_disk_full_lands_in_failed_storage() {
        assert_eq!(
            disk_full_response(DiskFullStage::Finalization).transfer_state,
            TransferStorageState::FailedStorage
        );
        assert_eq!(
            disk_full_response(DiskFullStage::BlobWrite).transfer_state,
            TransferStorageState::Unchanged
        );
    }

    #[test]
    fn space_recovery_options_match_the_spec() {
        let a = disk_full_response(DiskFullStage::DbTransaction).space_actions;
        assert_eq!(
            a,
            vec![
                SpaceAction::EvictCache,
                SpaceAction::FreeTemp,
                SpaceAction::RetryOnUserRequest
            ]
        );
    }

    #[test]
    fn chunk_is_complete_only_after_a_durable_write() {
        assert!(chunk_may_be_marked_complete(ChunkWriteResult::Durable));
        assert!(!chunk_may_be_marked_complete(ChunkWriteResult::DiskFull));
        assert!(!chunk_may_be_marked_complete(ChunkWriteResult::OtherError));
    }
}
