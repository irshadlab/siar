//! §18 "Bundle State Machine", §19 "Delivery Semantics": "Do not equate
//! forwarded with delivered."

use crate::events::DtnEvent;
use crate::types::BundleId;

/// §18's named sequence. `ForwardedAgain` from the spec's own diagram
/// is deliberately collapsed into re-entering [`BundleState::Forwarded`]
/// here rather than modeled as a distinct state — the spec shows it as
/// the same state repeating for additional hops, not a semantically
/// different one, and [`BundleState::transition`] already allows
/// `Forwarded → Forwarded` for that reason (flagged as a real
/// simplification, not an oversight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleState {
    Created,
    Stored,
    Eligible,
    Forwarded,
    DestinationReached,
    Acknowledged,
    Completed,
    Expired,
    Evicted,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleEvent {
    PersistDurably,
    BecomeEligible,
    Forward,
    ReachDestination,
    Acknowledge,
    Complete,
    Expire,
    Evict,
    Cancel,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{event:?} is not a valid transition from {state:?}")]
pub struct InvalidBundleTransition {
    pub state: BundleState,
    pub event: BundleEvent,
}

impl BundleState {
    /// §19: forwarded and delivered are kept as genuinely different
    /// states here (`Forwarded` vs `DestinationReached`/`Acknowledged`)
    /// rather than collapsed — a caller checking "is this delivered"
    /// must check for the latter two specifically, never treat
    /// `Forwarded` as good enough, which [`BundleState::is_delivered`]
    /// makes impossible to get wrong by accident.
    pub fn transition(self, event: BundleEvent) -> Result<BundleState, InvalidBundleTransition> {
        use BundleEvent as E;
        use BundleState as S;
        let next = match (self, event) {
            (S::Created, E::PersistDurably) => S::Stored,
            (S::Created, E::Reject) => S::Rejected,
            (S::Stored, E::BecomeEligible) => S::Eligible,
            (S::Stored, E::Expire) => S::Expired,
            (S::Stored, E::Evict) => S::Evicted,
            (S::Stored, E::Cancel) => S::Cancelled,
            (S::Eligible, E::Forward) => S::Forwarded,
            (S::Eligible, E::Expire) => S::Expired,
            (S::Eligible, E::Evict) => S::Evicted,
            (S::Eligible, E::Cancel) => S::Cancelled,
            (S::Forwarded, E::Forward) => S::Forwarded, // "ForwardedAgain" — see this type's own doc comment
            (S::Forwarded, E::ReachDestination) => S::DestinationReached,
            (S::Forwarded, E::Expire) => S::Expired,
            (S::Forwarded, E::Evict) => S::Evicted,
            (S::DestinationReached, E::Acknowledge) => S::Acknowledged,
            (S::Acknowledged, E::Complete) => S::Completed,
            _ => return Err(InvalidBundleTransition { state: self, event }),
        };
        Ok(next)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Expired | Self::Evicted | Self::Cancelled | Self::Rejected
        )
    }

    /// §19's own distinction, made a real, unambiguous check.
    pub fn is_delivered(self) -> bool {
        matches!(
            self,
            Self::DestinationReached | Self::Acknowledged | Self::Completed
        )
    }
}

/// §29 "Pure Decision Functions," this domain's own instance — same
/// adapted shape `siar_blob_manifest::transfer_state::decide`/
/// `siar_emergency::decide` already use (state alongside events, not
/// events alone; see the first of those two for why). This one is the
/// clearest case for `Vec`'s own shape in the spec's original
/// signature: `BecomeEligible`/`Reject` are real, valid transitions
/// that produce ZERO [`DtnEvent`]s (see `siar_dtn_bundle_service`'s
/// own doc comment for exactly why those two specifically), so this
/// function's return type genuinely varies between an empty `Vec` and
/// a one-element one, not always the latter the way its two siblings
/// do.
///
/// Pure, synchronous, no IO, no async, no event log. Doesn't cover
/// `BundleCreated`, for the same reason neither sibling `decide`
/// covers ITS domain's own creation event: no "current state" to
/// decide from — the real caller's own `create_bundle` handles it
/// directly.
pub fn decide(
    state: BundleState,
    command: BundleEvent,
    bundle_id: BundleId,
) -> Result<(BundleState, Vec<DtnEvent>), InvalidBundleTransition> {
    use BundleEvent as E;
    let next = state.transition(command)?;
    let events = match command {
        E::PersistDurably => vec![DtnEvent::BundleStored { bundle_id }],
        E::BecomeEligible => vec![],
        E::Forward => vec![DtnEvent::BundleForwarded { bundle_id }],
        E::ReachDestination => vec![DtnEvent::BundleDestinationReached { bundle_id }],
        E::Acknowledge => vec![DtnEvent::BundleAcknowledged { bundle_id }],
        E::Complete => vec![DtnEvent::BundleCompleted { bundle_id }],
        E::Expire => vec![DtnEvent::BundleExpired { bundle_id }],
        E::Evict => vec![DtnEvent::BundleEvicted { bundle_id }],
        E::Cancel => vec![DtnEvent::BundleCancelled { bundle_id }],
        E::Reject => vec![],
    };
    Ok((next, events))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_bundle_walks_from_creation_to_completion() {
        let s = BundleState::Created;
        let s = s.transition(BundleEvent::PersistDurably).unwrap();
        let s = s.transition(BundleEvent::BecomeEligible).unwrap();
        let s = s.transition(BundleEvent::Forward).unwrap();
        assert!(!s.is_delivered()); // forwarded is NOT delivered — §19
        let s = s.transition(BundleEvent::ReachDestination).unwrap();
        assert!(s.is_delivered());
        let s = s.transition(BundleEvent::Acknowledge).unwrap();
        let s = s.transition(BundleEvent::Complete).unwrap();
        assert!(s.is_terminal());
    }

    #[test]
    fn multiple_hops_stay_in_forwarded_without_a_separate_state() {
        let s = BundleState::Forwarded;
        let s = s.transition(BundleEvent::Forward).unwrap();
        assert_eq!(s, BundleState::Forwarded);
    }

    #[test]
    fn a_completed_bundle_is_terminal_and_rejects_further_events() {
        let s = BundleState::Completed;
        assert!(s.is_terminal());
        assert!(s.transition(BundleEvent::Forward).is_err());
    }

    #[test]
    fn forwarded_is_never_mistaken_for_delivered() {
        assert!(!BundleState::Forwarded.is_delivered());
        assert!(BundleState::Acknowledged.is_delivered());
    }

    #[test]
    fn decide_returns_the_matching_event_and_the_same_next_state_transition_would() {
        let bundle_id = BundleId::new();
        let (next, events) =
            decide(BundleState::Created, BundleEvent::PersistDurably, bundle_id).unwrap();
        assert_eq!(
            next,
            BundleState::Created
                .transition(BundleEvent::PersistDurably)
                .unwrap()
        );
        assert_eq!(events, vec![DtnEvent::BundleStored { bundle_id }]);
    }

    #[test]
    fn decide_produces_zero_events_for_become_eligible_and_reject() {
        let bundle_id = BundleId::new();
        let (_, events) =
            decide(BundleState::Stored, BundleEvent::BecomeEligible, bundle_id).unwrap();
        assert_eq!(events, vec![]);
        let (_, events) = decide(BundleState::Created, BundleEvent::Reject, bundle_id).unwrap();
        assert_eq!(events, vec![]);
    }

    #[test]
    fn decide_produces_one_event_for_every_other_valid_transition() {
        let bundle_id = BundleId::new();
        for (state, command) in [
            (BundleState::Created, BundleEvent::PersistDurably),
            (BundleState::Eligible, BundleEvent::Forward),
            (BundleState::Forwarded, BundleEvent::ReachDestination),
            (BundleState::DestinationReached, BundleEvent::Acknowledge),
            (BundleState::Acknowledged, BundleEvent::Complete),
            (BundleState::Stored, BundleEvent::Expire),
            (BundleState::Stored, BundleEvent::Evict),
            (BundleState::Stored, BundleEvent::Cancel),
        ] {
            let (_, events) = decide(state, command, bundle_id).unwrap();
            assert_eq!(events.len(), 1, "{state:?} + {command:?}");
        }
    }

    #[test]
    fn decide_rejects_an_illegal_transition_exactly_like_transition_does() {
        let bundle_id = BundleId::new();
        let result = decide(BundleState::Created, BundleEvent::Forward, bundle_id);
        assert!(result.is_err());
    }
}
