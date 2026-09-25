//! §37's own report lifecycle, made a real state machine — a gap
//! found while wiring a real `EventStore::append` caller for this
//! crate's `events.rs` (in the sibling `siar-emergency-service`
//! crate): unlike `siar_blob_manifest::transfer_state`/
//! `siar_dtn_bundle::state`, this crate had `EmergencyEvent`'s six
//! variants but no decide layer of its own to gate them — nothing
//! stopped a caller from recording `ReportResolved` for a report
//! that was already `Cancelled`. This module closes that gap, same
//! shape those two sibling modules already use
//! (`state`/`event`/`transition`/`InvalidTransition`).
//!
//! Only three of `EmergencyEvent`'s six variants are real lifecycle
//! transitions — [`ReportEvent::Resolve`]/[`ReportEvent::Cancel`]/
//! [`ReportEvent::Expire`], each terminal and mutually exclusive.
//! `TrustReclassified`/`ReportAcknowledged` deliberately have NO
//! corresponding [`ReportEvent`] variant: a report's trust
//! classification can change, and acknowledgments can arrive, at any
//! point regardless of status — an acknowledgment can legitimately
//! arrive over a delayed mesh path after the report is already
//! `Resolved`, and gating it on `Active` would just drop a real,
//! late-arriving fact. A real caller records those two directly,
//! gated only by "is this report known to the caller at all," never
//! by [`ReportStatus`].

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportStatus {
    /// Created, not yet resolved/cancelled/expired. The only
    /// non-terminal status — see [`Self::is_terminal`].
    Active,
    ReportResolved,
    ReportCancelled,
    ReportExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportEvent {
    Resolve,
    Cancel,
    Expire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{event:?} is not a valid transition from {state:?}")]
pub struct InvalidReportTransition {
    pub state: ReportStatus,
    pub event: ReportEvent,
}

impl ReportStatus {
    /// A real state machine, same discipline
    /// `siar_blob_manifest::transfer_state::TransferState::transition`/
    /// `siar_dtn_bundle::state::BundleState::transition` both already
    /// apply: every terminal status rejects every further event,
    /// rather than silently re-resolving/re-cancelling/re-expiring an
    /// already-closed report.
    pub fn transition(self, event: ReportEvent) -> Result<ReportStatus, InvalidReportTransition> {
        use ReportEvent as E;
        use ReportStatus as S;
        let next = match (self, event) {
            (S::Active, E::Resolve) => S::ReportResolved,
            (S::Active, E::Cancel) => S::ReportCancelled,
            (S::Active, E::Expire) => S::ReportExpired,
            _ => return Err(InvalidReportTransition { state: self, event }),
        };
        Ok(next)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::ReportResolved | Self::ReportCancelled | Self::ReportExpired
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_can_be_resolved_cancelled_or_expired_from_active() {
        assert_eq!(
            ReportStatus::Active
                .transition(ReportEvent::Resolve)
                .unwrap(),
            ReportStatus::ReportResolved
        );
        assert_eq!(
            ReportStatus::Active
                .transition(ReportEvent::Cancel)
                .unwrap(),
            ReportStatus::ReportCancelled
        );
        assert_eq!(
            ReportStatus::Active
                .transition(ReportEvent::Expire)
                .unwrap(),
            ReportStatus::ReportExpired
        );
    }

    #[test]
    fn a_terminal_status_rejects_every_further_event() {
        for status in [
            ReportStatus::ReportResolved,
            ReportStatus::ReportCancelled,
            ReportStatus::ReportExpired,
        ] {
            assert!(status.is_terminal());
            for event in [
                ReportEvent::Resolve,
                ReportEvent::Cancel,
                ReportEvent::Expire,
            ] {
                assert!(status.transition(event).is_err());
            }
        }
    }

    #[test]
    fn active_is_not_terminal() {
        assert!(!ReportStatus::Active.is_terminal());
    }
}
