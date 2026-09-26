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

use crate::events::EmergencyEvent;
use crate::ids::ReportId;

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

/// §29 "Pure Decision Functions," this domain's own instance — same
/// adapted shape `siar_blob_manifest::transfer_state::decide` already
/// uses (state alongside events, not events alone; see that
/// function's own doc comment for why). Simpler than that sibling in
/// one real way: every [`ReportEvent`] variant here is bare and maps
/// to exactly one [`EmergencyEvent`] with no missing data to refuse
/// on the way (no `Fail`-needs-a-reason case in this domain), so this
/// returns [`InvalidReportTransition`] directly rather than needing
/// its own wrapping error type.
///
/// Pure, synchronous, no IO, no async, no event log. Deliberately
/// does NOT cover `TrustReclassified`/`ReportAcknowledged` — see this
/// module's own top doc comment for why those were never `ReportEvent`
/// variants to begin with — nor `ReportCreated`, which isn't a
/// `(state, command)` pair (no "current status" to decide from for a
/// report's own first event); the real caller's own `create_report`
/// handles that directly, same reason `siar_blob_manifest::
/// transfer_state::decide` doesn't cover `TransferCreated` either.
pub fn decide(
    state: ReportStatus,
    command: ReportEvent,
    report_id: ReportId,
) -> Result<(ReportStatus, Vec<EmergencyEvent>), InvalidReportTransition> {
    let next = state.transition(command)?;
    let event = match command {
        ReportEvent::Resolve => EmergencyEvent::ReportResolved { report_id },
        ReportEvent::Cancel => EmergencyEvent::ReportCancelled { report_id },
        ReportEvent::Expire => EmergencyEvent::ReportExpired { report_id },
    };
    Ok((next, vec![event]))
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

    #[test]
    fn decide_returns_the_matching_event_and_the_same_next_state_transition_would() {
        let report_id = ReportId::new();
        let (next, events) = decide(ReportStatus::Active, ReportEvent::Resolve, report_id).unwrap();
        assert_eq!(
            next,
            ReportStatus::Active
                .transition(ReportEvent::Resolve)
                .unwrap()
        );
        assert_eq!(events, vec![EmergencyEvent::ReportResolved { report_id }]);
    }

    #[test]
    fn decide_rejects_a_second_lifecycle_event_on_an_already_terminal_report() {
        let report_id = ReportId::new();
        let result = decide(
            ReportStatus::ReportCancelled,
            ReportEvent::Resolve,
            report_id,
        );
        assert!(result.is_err());
    }

    #[test]
    fn decide_covers_all_three_lifecycle_events_from_active() {
        let report_id = ReportId::new();
        for (command, expected) in [
            (
                ReportEvent::Resolve,
                EmergencyEvent::ReportResolved { report_id },
            ),
            (
                ReportEvent::Cancel,
                EmergencyEvent::ReportCancelled { report_id },
            ),
            (
                ReportEvent::Expire,
                EmergencyEvent::ReportExpired { report_id },
            ),
        ] {
            let (_, events) = decide(ReportStatus::Active, command, report_id).unwrap();
            assert_eq!(events, vec![expected]);
        }
    }
}
