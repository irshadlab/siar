//! §26 "Transfer State Machine".

use serde::{Deserialize, Serialize};

use crate::events::FileEvent;
use crate::ids::TransferId;

/// §26's own named states — the spec's section shows the states
/// conceptually rather than an exhaustive transition table, so
/// [`TransferState::transition`]'s specific edges below are this
/// crate's own reasonable reading of the obvious lifecycle (offer →
/// accept → transfer → complete, with pause/cancel/fail available from
/// the right states), not a transcription of an explicit table in the
/// source document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    Offered,
    Accepted,
    InProgress,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferEvent {
    Accept,
    Decline,
    Start,
    Pause,
    Resume,
    ChunksComplete,
    Fail,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{event:?} is not a valid transition from {state:?}")]
pub struct InvalidTransition {
    pub state: TransferState,
    pub event: TransferEvent,
}

impl TransferState {
    /// A real state machine — most `(state, event)` pairs are rejected,
    /// not silently accepted. `Failed`/`Cancelled` are terminal:
    /// nothing transitions out of them here, matching an immutable
    /// event log's own append-only spirit (Part 04 §10) applied to
    /// transfer lifecycle instead of event storage.
    pub fn transition(self, event: TransferEvent) -> Result<TransferState, InvalidTransition> {
        use TransferEvent as E;
        use TransferState as S;
        let next = match (self, event) {
            (S::Offered, E::Accept) => S::Accepted,
            (S::Offered, E::Decline) => S::Cancelled,
            (S::Offered, E::Cancel) => S::Cancelled,
            (S::Accepted, E::Start) => S::InProgress,
            (S::Accepted, E::Cancel) => S::Cancelled,
            (S::InProgress, E::Pause) => S::Paused,
            (S::InProgress, E::ChunksComplete) => S::Completed,
            (S::InProgress, E::Fail) => S::Failed,
            (S::InProgress, E::Cancel) => S::Cancelled,
            (S::Paused, E::Resume) => S::InProgress,
            (S::Paused, E::Cancel) => S::Cancelled,
            (S::Paused, E::Fail) => S::Failed,
            _ => return Err(InvalidTransition { state: self, event }),
        };
        Ok(next)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// §29 "Pure Decision Functions," this domain's own instance of the
/// spec's exact shape (`fn decide(state, command) -> Result<Vec<
/// DomainEvent>, DomainError>`), adapted in one stated way: the
/// spec's own signature returns events alone, meaning a caller would
/// re-derive the resulting state by folding them — this domain's real
/// caller (`siar_file_transfer_service::FileTransferService::apply`,
/// which already existed before this function did, built inline
/// rather than through a named pure function) needs the resulting
/// [`TransferState`] directly, to keep its own in-memory state map
/// current without reimplementing that fold. So this returns the
/// state alongside the events instead.
///
/// Pure, synchronous, no IO, no async, no event log — [`TransferId`]
/// is the only "domain" data this needs, since every real
/// [`FileEvent`] variant `decide` can produce carries nothing else.
/// Every legal edge in [`TransferState::transition`]'s own table is
/// covered here too, by construction: this function calls that one
/// first, and only builds an event once the transition itself has
/// already succeeded.
///
/// Two things this function deliberately does NOT decide, named here
/// rather than silently missing:
/// - [`TransferEvent::Fail`] carries no `reason` field, but
///   [`FileEvent::TransferFailed`] needs one this function has no way
///   to supply — refused with [`DecideError::FailNeedsReason`] rather
///   than fabricating a placeholder string. The real caller's own
///   `fail_transfer` method (not this function) is where a real reason
///   and this decision meet.
/// - `TransferCreated` isn't a `(state, command)` pair at all — there
///   is no "current state" to decide FROM for a transfer's own first
///   event — so it was never in scope for this shape to begin with;
///   the real caller's own `create_transfer` handles it directly.
pub fn decide(
    state: TransferState,
    command: TransferEvent,
    transfer_id: TransferId,
) -> Result<(TransferState, Vec<FileEvent>), DecideError> {
    use TransferEvent as E;
    if matches!(command, E::Fail) {
        return Err(DecideError::FailNeedsReason);
    }
    let next = state.transition(command)?;
    let event = match command {
        E::Accept => FileEvent::TransferAccepted { transfer_id },
        E::Decline => FileEvent::TransferCancelled { transfer_id },
        E::Start => FileEvent::TransferStarted { transfer_id },
        E::Pause => FileEvent::TransferPaused { transfer_id },
        E::Resume => FileEvent::TransferResumed { transfer_id },
        E::ChunksComplete => FileEvent::TransferCompleted { transfer_id },
        E::Cancel => FileEvent::TransferCancelled { transfer_id },
        E::Fail => unreachable!("refused above, before state.transition was ever called"),
    };
    Ok((next, vec![event]))
}

/// [`decide`]'s own error type — a strict superset of
/// [`InvalidTransition`] (every illegal transition is still illegal
/// here) plus the one case pure decision can't handle at all. See
/// [`decide`]'s own doc comment for what each variant means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecideError {
    #[error(transparent)]
    InvalidTransition(#[from] InvalidTransition),
    #[error(
        "TransferEvent::Fail carries no reason for a pure decide() to use — the real caller's own fail_transfer is where a reason and this decision meet"
    )]
    FailNeedsReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_returns_the_matching_event_and_the_same_next_state_transition_would() {
        let transfer_id = TransferId::new();
        let (next, events) =
            decide(TransferState::Offered, TransferEvent::Accept, transfer_id).unwrap();
        assert_eq!(
            next,
            TransferState::Offered
                .transition(TransferEvent::Accept)
                .unwrap()
        );
        assert_eq!(events, vec![FileEvent::TransferAccepted { transfer_id }]);
    }

    #[test]
    fn decide_produces_exactly_one_event_for_every_non_fail_command() {
        let transfer_id = TransferId::new();
        for (state, command) in [
            (TransferState::Offered, TransferEvent::Accept),
            (TransferState::Offered, TransferEvent::Decline),
            (TransferState::Accepted, TransferEvent::Start),
            (TransferState::InProgress, TransferEvent::Pause),
            (TransferState::Paused, TransferEvent::Resume),
            (TransferState::InProgress, TransferEvent::ChunksComplete),
            (TransferState::Accepted, TransferEvent::Cancel),
        ] {
            let (_, events) = decide(state, command, transfer_id).unwrap();
            assert_eq!(events.len(), 1, "{state:?} + {command:?}");
        }
    }

    #[test]
    fn decide_refuses_fail_rather_than_fabricating_a_reason() {
        let transfer_id = TransferId::new();
        let result = decide(TransferState::InProgress, TransferEvent::Fail, transfer_id);
        assert_eq!(result, Err(DecideError::FailNeedsReason));
    }

    #[test]
    fn decide_rejects_an_illegal_transition_exactly_like_transition_does() {
        let transfer_id = TransferId::new();
        let result = decide(TransferState::Offered, TransferEvent::Start, transfer_id);
        assert!(matches!(result, Err(DecideError::InvalidTransition(_))));
    }

    #[test]
    fn decline_and_cancel_both_decide_the_same_cancelled_event() {
        let transfer_id = TransferId::new();
        let (_, declined) =
            decide(TransferState::Offered, TransferEvent::Decline, transfer_id).unwrap();
        let (_, cancelled) =
            decide(TransferState::Accepted, TransferEvent::Cancel, transfer_id).unwrap();
        assert_eq!(declined, vec![FileEvent::TransferCancelled { transfer_id }]);
        assert_eq!(
            cancelled,
            vec![FileEvent::TransferCancelled { transfer_id }]
        );
    }

    #[test]
    fn a_normal_transfer_walks_offer_through_completion() {
        let s = TransferState::Offered;
        let s = s.transition(TransferEvent::Accept).unwrap();
        assert_eq!(s, TransferState::Accepted);
        let s = s.transition(TransferEvent::Start).unwrap();
        assert_eq!(s, TransferState::InProgress);
        let s = s.transition(TransferEvent::ChunksComplete).unwrap();
        assert_eq!(s, TransferState::Completed);
        assert!(s.is_terminal());
    }

    #[test]
    fn pause_and_resume_returns_to_in_progress() {
        let s = TransferState::InProgress;
        let s = s.transition(TransferEvent::Pause).unwrap();
        assert_eq!(s, TransferState::Paused);
        let s = s.transition(TransferEvent::Resume).unwrap();
        assert_eq!(s, TransferState::InProgress);
    }

    #[test]
    fn completed_is_terminal_and_rejects_further_events() {
        let s = TransferState::Completed;
        assert!(s.is_terminal());
        assert!(s.transition(TransferEvent::Cancel).is_err());
        assert!(s.transition(TransferEvent::Pause).is_err());
    }

    #[test]
    fn starting_a_transfer_that_was_never_accepted_is_rejected() {
        let s = TransferState::Offered;
        let result = s.transition(TransferEvent::Start);
        assert_eq!(
            result,
            Err(InvalidTransition {
                state: TransferState::Offered,
                event: TransferEvent::Start
            })
        );
    }
}
