//! §57 "Backup Integration" and §58 "Restore Is Not Ordinary
//! Recovery".
//!
//! §57: a backup may be used for repair only if it is authenticated,
//! versioned, and integrity-checked. §58: a restore can resurrect old
//! pending operations, so nothing restored may be rescheduled until
//! expiry, delivery state, revocation and peer state have been
//! re-evaluated — enforced by [`RestoreGate`], which has no way to
//! report "ready" before all four are recorded.

use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

// ---------------------------------------------------------------------
// §57
// ---------------------------------------------------------------------

/// What recovery needs to know about a backup before trusting it.
/// Each flag is the *result* of a check performed by the backup
/// format's own crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupHeader {
    pub format_version: u32,
    pub authenticated: bool,
    pub integrity_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BackupRejection {
    #[error("backup is not authenticated")]
    Unauthenticated,
    #[error("backup format version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("backup failed its integrity check")]
    IntegrityFailed,
}

/// All three §57 properties must hold. Authentication is checked
/// first: nothing else about an unauthenticated blob is meaningful.
pub fn validate_backup(
    h: &BackupHeader,
    supported: &RangeInclusive<u32>,
) -> Result<(), BackupRejection> {
    if !h.authenticated {
        return Err(BackupRejection::Unauthenticated);
    }
    if !supported.contains(&h.format_version) {
        return Err(BackupRejection::UnsupportedVersion(h.format_version));
    }
    if !h.integrity_ok {
        return Err(BackupRejection::IntegrityFailed);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// §58
// ---------------------------------------------------------------------

/// The four §58 re-evaluations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Reevaluation {
    Expiry = 0,
    DeliveryState = 1,
    Revocation = 2,
    PeerState = 3,
}

impl Reevaluation {
    pub const ALL: [Reevaluation; 4] = [
        Reevaluation::Expiry,
        Reevaluation::DeliveryState,
        Reevaluation::Revocation,
        Reevaluation::PeerState,
    ];
}

/// Blocks effect scheduling after a restore until all four
/// re-evaluations are recorded. `Copy`, one byte.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RestoreGate {
    done: u8,
}

impl RestoreGate {
    pub const fn after_restore() -> Self {
        Self { done: 0 }
    }

    pub fn record(&mut self, r: Reevaluation) {
        self.done |= 1 << (r as u8);
    }

    pub const fn is_done(&self, r: Reevaluation) -> bool {
        self.done & (1 << (r as u8)) != 0
    }

    pub const fn may_reschedule_effects(&self) -> bool {
        self.done == 0b1111
    }

    pub fn pending(&self) -> Vec<Reevaluation> {
        Reevaluation::ALL
            .iter()
            .copied()
            .filter(|r| !self.is_done(*r))
            .collect()
    }
}

/// A pending operation that came back from the backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoredOperation {
    pub expires_at_millis: u64,
    /// The restored state says it's pending, but current knowledge
    /// (newer than the backup) says it was already delivered.
    pub already_delivered: bool,
    pub peer_revoked: bool,
    /// The peer is no longer known at all.
    pub peer_unknown: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoredDecision {
    Reschedule,
    DropExpired,
    DropAlreadyDelivered,
    DropPeerRevoked,
    /// Peer state can't be established; hold rather than send to an
    /// unknown party.
    HoldPeerUnknown,
}

/// Precedence: revocation is a security fact and wins; then
/// delivery; then expiry; then peer knowledge.
pub fn reevaluate_restored(op: &RestoredOperation, now_millis: u64) -> RestoredDecision {
    if op.peer_revoked {
        RestoredDecision::DropPeerRevoked
    } else if op.already_delivered {
        RestoredDecision::DropAlreadyDelivered
    } else if op.expires_at_millis <= now_millis {
        RestoredDecision::DropExpired
    } else if op.peer_unknown {
        RestoredDecision::HoldPeerUnknown
    } else {
        RestoredDecision::Reschedule
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED: RangeInclusive<u32> = 2..=4;

    fn hdr() -> BackupHeader {
        BackupHeader {
            format_version: 3,
            authenticated: true,
            integrity_ok: true,
        }
    }

    #[test]
    fn good_backup_is_accepted() {
        assert_eq!(validate_backup(&hdr(), &SUPPORTED), Ok(()));
    }

    #[test]
    fn each_property_failing_rejects() {
        let mut h = hdr();
        h.authenticated = false;
        assert_eq!(
            validate_backup(&h, &SUPPORTED),
            Err(BackupRejection::Unauthenticated)
        );
        let mut h = hdr();
        h.format_version = 9;
        assert_eq!(
            validate_backup(&h, &SUPPORTED),
            Err(BackupRejection::UnsupportedVersion(9))
        );
        let mut h = hdr();
        h.integrity_ok = false;
        assert_eq!(
            validate_backup(&h, &SUPPORTED),
            Err(BackupRejection::IntegrityFailed)
        );
    }

    #[test]
    fn unauthenticated_is_reported_before_anything_else() {
        let h = BackupHeader {
            format_version: 99,
            authenticated: false,
            integrity_ok: false,
        };
        assert_eq!(
            validate_backup(&h, &SUPPORTED),
            Err(BackupRejection::Unauthenticated)
        );
    }

    #[test]
    fn restore_gate_needs_all_four_reevaluations() {
        let mut g = RestoreGate::after_restore();
        assert!(!g.may_reschedule_effects());
        for r in [
            Reevaluation::Expiry,
            Reevaluation::DeliveryState,
            Reevaluation::Revocation,
        ] {
            g.record(r);
            assert!(!g.may_reschedule_effects());
        }
        assert_eq!(g.pending(), vec![Reevaluation::PeerState]);
        g.record(Reevaluation::PeerState);
        assert!(g.may_reschedule_effects());
        assert!(g.pending().is_empty());
    }

    #[test]
    fn recording_twice_is_harmless() {
        let mut g = RestoreGate::after_restore();
        g.record(Reevaluation::Expiry);
        g.record(Reevaluation::Expiry);
        assert_eq!(g.pending().len(), 3);
    }

    fn op() -> RestoredOperation {
        RestoredOperation {
            expires_at_millis: 1_000,
            already_delivered: false,
            peer_revoked: false,
            peer_unknown: false,
        }
    }

    #[test]
    fn restored_operation_decisions() {
        assert_eq!(
            reevaluate_restored(&op(), 500),
            RestoredDecision::Reschedule
        );
        assert_eq!(
            reevaluate_restored(&op(), 1_000),
            RestoredDecision::DropExpired
        );
        let mut o = op();
        o.already_delivered = true;
        assert_eq!(
            reevaluate_restored(&o, 500),
            RestoredDecision::DropAlreadyDelivered
        );
        let mut o = op();
        o.peer_unknown = true;
        assert_eq!(
            reevaluate_restored(&o, 500),
            RestoredDecision::HoldPeerUnknown
        );
    }

    #[test]
    fn revocation_beats_every_other_outcome() {
        let o = RestoredOperation {
            expires_at_millis: 0,
            already_delivered: true,
            peer_revoked: true,
            peer_unknown: true,
        };
        assert_eq!(
            reevaluate_restored(&o, 5_000),
            RestoredDecision::DropPeerRevoked
        );
    }
}
