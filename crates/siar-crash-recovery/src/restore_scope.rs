//! §144 "Disaster Recovery vs Crash Recovery", §145 "Multi-Device
//! Recovery", §146 "Partial History Recovery".
//!
//! Crash recovery assumes the same device with storage mostly intact.
//! Disaster recovery assumes storage is gone and everything comes
//! from a backup or another trusted device — which is a *restore*
//! (§58), with all its re-evaluation duties. No function here mentions
//! or assumes a server.

use crate::backup_restore::RestoreGate;
use crate::corruption::{verify_contiguous, SequenceGap};

// ---------------------------------------------------------------------
// §144
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKind {
    /// Same device, storage mostly intact.
    Crash,
    /// Device or storage lost; restore from elsewhere.
    Disaster,
}

pub const fn classify_recovery(local_storage_intact: bool) -> RecoveryKind {
    if local_storage_intact {
        RecoveryKind::Crash
    } else {
        RecoveryKind::Disaster
    }
}

/// A disaster recovery is a restore and must pass §58's gate before
/// any restored effect is rescheduled; a crash recovery is not.
pub fn restore_gate_for(kind: RecoveryKind) -> Option<RestoreGate> {
    match kind {
        RecoveryKind::Crash => None,
        RecoveryKind::Disaster => Some(RestoreGate::after_restore()),
    }
}

// ---------------------------------------------------------------------
// §145
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapFacts {
    /// Identity authorization of this new/recovered device has
    /// completed (Part 02). Without it nothing may be pulled.
    pub device_authorized: bool,
    pub own_trusted_device_reachable: bool,
    pub local_backup_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapPlan {
    /// Cannot bootstrap from anyone yet.
    AwaitIdentityAuthorization,
    FromOwnTrustedDevice,
    FromLocalBackup,
    /// Nothing to restore from: begin empty (and say so).
    StartEmpty,
}

pub const fn bootstrap_plan(f: BootstrapFacts) -> BootstrapPlan {
    if !f.device_authorized {
        BootstrapPlan::AwaitIdentityAuthorization
    } else if f.own_trusted_device_reachable {
        BootstrapPlan::FromOwnTrustedDevice
    } else if f.local_backup_available {
        BootstrapPlan::FromLocalBackup
    } else {
        BootstrapPlan::StartEmpty
    }
}

// ---------------------------------------------------------------------
// §146
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobRestore {
    OnDemand,
    Eager,
}

/// What a restore brings back. Full history is policy-dependent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestorePolicy {
    /// Restore only events with sequence ≥ this; `0` means everything.
    pub first_seq: u64,
    pub metadata: bool,
    pub blobs: BlobRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryCompleteness {
    Full,
    Partial { first_seq: u64 },
}

pub const fn completeness(p: RestorePolicy) -> HistoryCompleteness {
    if p.first_seq == 0 {
        HistoryCompleteness::Full
    } else {
        HistoryCompleteness::Partial {
            first_seq: p.first_seq,
        }
    }
}

/// Whether to fetch a blob during the restore itself, or leave it for
/// the first time the user opens it.
pub const fn fetch_blob_now(p: RestorePolicy, user_requested: bool) -> bool {
    user_requested || matches!(p.blobs, BlobRestore::Eager)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAccess {
    Present,
    /// Older than the restored window: fetch on demand, **not**
    /// corruption.
    BeforeWindowFetchOnDemand,
}

pub const fn history_access(c: HistoryCompleteness, seq: u64) -> HistoryAccess {
    match c {
        HistoryCompleteness::Full => HistoryAccess::Present,
        HistoryCompleteness::Partial { first_seq } => {
            if seq >= first_seq {
                HistoryAccess::Present
            } else {
                HistoryAccess::BeforeWindowFetchOnDemand
            }
        }
    }
}

/// Integrity check of a restored stream that starts mid-history: the
/// window's own start is the expected first sequence, so the deliberate
/// absence of older events is not reported as a gap (§55), while a hole
/// *inside* the window still is.
pub fn verify_restored_window(
    c: HistoryCompleteness,
    seqs: impl IntoIterator<Item = u64>,
) -> Result<u64, SequenceGap> {
    let first = match c {
        HistoryCompleteness::Full => 1,
        HistoryCompleteness::Partial { first_seq } => first_seq,
    };
    verify_contiguous(first, seqs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intact_storage_is_crash_recovery_lost_storage_is_disaster() {
        assert_eq!(classify_recovery(true), RecoveryKind::Crash);
        assert_eq!(classify_recovery(false), RecoveryKind::Disaster);
    }

    #[test]
    fn only_disaster_recovery_is_a_gated_restore() {
        assert!(restore_gate_for(RecoveryKind::Crash).is_none());
        let g = restore_gate_for(RecoveryKind::Disaster).unwrap();
        assert!(!g.may_reschedule_effects());
    }

    fn f(auth: bool, dev: bool, backup: bool) -> BootstrapFacts {
        BootstrapFacts {
            device_authorized: auth,
            own_trusted_device_reachable: dev,
            local_backup_available: backup,
        }
    }

    #[test]
    fn nothing_is_pulled_before_identity_authorization() {
        for dev in [false, true] {
            for backup in [false, true] {
                assert_eq!(
                    bootstrap_plan(f(false, dev, backup)),
                    BootstrapPlan::AwaitIdentityAuthorization
                );
            }
        }
    }

    #[test]
    fn bootstrap_prefers_own_device_then_local_backup_then_empty() {
        assert_eq!(
            bootstrap_plan(f(true, true, true)),
            BootstrapPlan::FromOwnTrustedDevice
        );
        assert_eq!(
            bootstrap_plan(f(true, false, true)),
            BootstrapPlan::FromLocalBackup
        );
        assert_eq!(
            bootstrap_plan(f(true, false, false)),
            BootstrapPlan::StartEmpty
        );
    }

    #[test]
    fn partial_history_is_policy_not_corruption() {
        let full = RestorePolicy {
            first_seq: 0,
            metadata: true,
            blobs: BlobRestore::Eager,
        };
        let recent = RestorePolicy {
            first_seq: 500,
            metadata: true,
            blobs: BlobRestore::OnDemand,
        };
        assert_eq!(completeness(full), HistoryCompleteness::Full);
        assert_eq!(
            completeness(recent),
            HistoryCompleteness::Partial { first_seq: 500 }
        );
        let c = completeness(recent);
        assert_eq!(
            history_access(c, 499),
            HistoryAccess::BeforeWindowFetchOnDemand
        );
        assert_eq!(history_access(c, 500), HistoryAccess::Present);
        assert_eq!(
            history_access(HistoryCompleteness::Full, 1),
            HistoryAccess::Present
        );
    }

    #[test]
    fn on_demand_blobs_wait_for_the_user_eager_ones_do_not() {
        let od = RestorePolicy {
            first_seq: 0,
            metadata: true,
            blobs: BlobRestore::OnDemand,
        };
        let eager = RestorePolicy {
            blobs: BlobRestore::Eager,
            ..od
        };
        assert!(!fetch_blob_now(od, false));
        assert!(fetch_blob_now(od, true));
        assert!(fetch_blob_now(eager, false));
    }

    #[test]
    fn a_partial_window_is_verified_from_its_own_start() {
        let c = HistoryCompleteness::Partial { first_seq: 500 };
        assert_eq!(verify_restored_window(c, [500, 501, 502]), Ok(503));
        // a full-history check would call this a gap; the window doesn't
        assert!(verify_contiguous(1, [500, 501]).is_err());
        // but a hole inside the window is still caught
        assert_eq!(
            verify_restored_window(c, [500, 502]),
            Err(SequenceGap {
                expected: 501,
                found: 502
            })
        );
    }

    #[test]
    fn full_history_must_start_at_one() {
        assert_eq!(
            verify_restored_window(HistoryCompleteness::Full, [1, 2, 3]),
            Ok(4)
        );
        assert!(verify_restored_window(HistoryCompleteness::Full, [2, 3]).is_err());
    }
}
