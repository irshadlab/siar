//! §61 "Side Effect Journal", §62 "InFlight State Semantics", §63
//! "Unknown Outcome State", §64 "Network Effect Recovery".
//!
//! The journal records explicit effect state instead of relying on
//! "a task exists". The crash rule (§62): `InFlight` becomes
//! `UnknownOutcome`, never `Failed`. The only way out of
//! `UnknownOutcome` is evidence ([`resolve_unknown`]) or a retry that
//! is safe because the effect carries a stable id (§59/§64).

use crate::effect_identity::EffectId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// §61's five states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectState {
    Pending,
    InFlight,
    Succeeded,
    Failed,
    Deferred,
}

impl EffectState {
    pub const fn can_transition(from: EffectState, to: EffectState) -> bool {
        use EffectState::*;
        matches!(
            (from, to),
            (Pending, InFlight)
                | (InFlight, Succeeded)
                | (InFlight, Failed)
                | (InFlight, Deferred)
                | (Deferred, Pending)
                | (Failed, Pending) // explicit retry
        )
    }
}

/// §63, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectRecoveryState {
    Pending,
    UnknownOutcome,
    ConfirmedSuccess,
    ConfirmedFailure,
}

/// §62: what each journal state means once the process has crashed.
pub const fn classify_after_crash(s: EffectState) -> EffectRecoveryState {
    match s {
        EffectState::Pending | EffectState::Deferred => EffectRecoveryState::Pending,
        EffectState::InFlight => EffectRecoveryState::UnknownOutcome,
        EffectState::Succeeded => EffectRecoveryState::ConfirmedSuccess,
        EffectState::Failed => EffectRecoveryState::ConfirmedFailure,
    }
}

/// What a remote-state check (or an ACK found later) tells us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEvidence {
    Applied,
    NotApplied,
    Inconclusive,
}

/// The only evidence-driven exit from `UnknownOutcome`. Inconclusive
/// evidence leaves it unknown — "do not assume failure".
pub const fn resolve_unknown(e: RemoteEvidence) -> EffectRecoveryState {
    match e {
        RemoteEvidence::Applied => EffectRecoveryState::ConfirmedSuccess,
        RemoteEvidence::NotApplied => EffectRecoveryState::ConfirmedFailure,
        RemoteEvidence::Inconclusive => EffectRecoveryState::UnknownOutcome,
    }
}

/// §62's three options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownOutcomeAction {
    /// §64's preferred path: same id, receiver dedups.
    RetrySameId,
    VerifyRemoteState,
    Wait,
}

/// Retry if the protocol made the effect idempotent; otherwise verify
/// if a check exists; otherwise wait. Never "give up as failed".
pub const fn decide_unknown(idempotent: bool, can_verify_remote: bool) -> UnknownOutcomeAction {
    if idempotent {
        UnknownOutcomeAction::RetrySameId
    } else if can_verify_remote {
        UnknownOutcomeAction::VerifyRemoteState
    } else {
        UnknownOutcomeAction::Wait
    }
}

/// Whether re-performing the effect could duplicate it.
pub const fn retry_is_safe(s: EffectRecoveryState, idempotent: bool) -> bool {
    match s {
        EffectRecoveryState::Pending | EffectRecoveryState::ConfirmedFailure => true,
        EffectRecoveryState::UnknownOutcome => idempotent,
        EffectRecoveryState::ConfirmedSuccess => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("unknown effect")]
    Unknown,
    #[error("invalid effect transition {from:?} -> {to:?}")]
    InvalidTransition { from: EffectState, to: EffectState },
    #[error("effect already journaled")]
    Duplicate,
}

#[derive(Debug, Clone, Default)]
pub struct EffectJournal {
    entries: HashMap<EffectId, EffectState>,
}

impl EffectJournal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_durable<I: IntoIterator<Item = (EffectId, EffectState)>>(it: I) -> Self {
        Self {
            entries: it.into_iter().collect(),
        }
    }

    pub fn begin(&mut self, id: EffectId) -> Result<(), JournalError> {
        match self.entries.entry(id) {
            std::collections::hash_map::Entry::Occupied(_) => Err(JournalError::Duplicate),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(EffectState::Pending);
                Ok(())
            }
        }
    }

    pub fn transition(&mut self, id: EffectId, to: EffectState) -> Result<(), JournalError> {
        let cur = self.entries.get_mut(&id).ok_or(JournalError::Unknown)?;
        if !EffectState::can_transition(*cur, to) {
            return Err(JournalError::InvalidTransition { from: *cur, to });
        }
        *cur = to;
        Ok(())
    }

    pub fn state(&self, id: &EffectId) -> Option<EffectState> {
        self.entries.get(id).copied()
    }

    /// What recovery sees for every journaled effect. Does not mutate:
    /// `InFlight` entries are *reported* as unknown; they move on only
    /// via evidence or a deliberate retry.
    pub fn recover_after_crash(&self) -> Vec<(EffectId, EffectRecoveryState)> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .map(|(id, s)| (*id, classify_after_crash(*s)))
            .collect();
        v.sort_by_key(|(id, _)| id.sort_key());
        v
    }

    pub fn unknown_outcomes(&self) -> Vec<EffectId> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, s)| **s == EffectState::InFlight)
            .map(|(id, _)| *id)
            .collect();
        v.sort_by_key(EffectId::sort_key);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_identity::EffectId;
    use siar_domain::MessageId;

    fn eid() -> EffectId {
        EffectId::Message(MessageId::new())
    }

    #[test]
    fn in_flight_becomes_unknown_never_failed() {
        assert_eq!(
            classify_after_crash(EffectState::InFlight),
            EffectRecoveryState::UnknownOutcome
        );
        assert_eq!(
            classify_after_crash(EffectState::Succeeded),
            EffectRecoveryState::ConfirmedSuccess
        );
        assert_eq!(
            classify_after_crash(EffectState::Failed),
            EffectRecoveryState::ConfirmedFailure
        );
        assert_eq!(
            classify_after_crash(EffectState::Deferred),
            EffectRecoveryState::Pending
        );
    }

    #[test]
    fn inconclusive_evidence_keeps_the_outcome_unknown() {
        assert_eq!(
            resolve_unknown(RemoteEvidence::Inconclusive),
            EffectRecoveryState::UnknownOutcome
        );
        assert_eq!(
            resolve_unknown(RemoteEvidence::Applied),
            EffectRecoveryState::ConfirmedSuccess
        );
        assert_eq!(
            resolve_unknown(RemoteEvidence::NotApplied),
            EffectRecoveryState::ConfirmedFailure
        );
    }

    #[test]
    fn unknown_outcome_action_prefers_idempotent_retry() {
        use UnknownOutcomeAction::*;
        assert_eq!(decide_unknown(true, true), RetrySameId);
        assert_eq!(decide_unknown(true, false), RetrySameId);
        assert_eq!(decide_unknown(false, true), VerifyRemoteState);
        assert_eq!(decide_unknown(false, false), Wait);
    }

    #[test]
    fn retry_safety_matrix() {
        use EffectRecoveryState::*;
        assert!(retry_is_safe(Pending, false));
        assert!(retry_is_safe(ConfirmedFailure, false));
        assert!(!retry_is_safe(ConfirmedSuccess, true));
        assert!(retry_is_safe(UnknownOutcome, true));
        assert!(!retry_is_safe(UnknownOutcome, false));
    }

    #[test]
    fn journal_walks_the_legal_path_and_rejects_shortcuts() {
        let mut j = EffectJournal::new();
        let id = eid();
        j.begin(id).unwrap();
        assert_eq!(j.begin(id), Err(JournalError::Duplicate));
        assert!(matches!(
            j.transition(id, EffectState::Succeeded),
            Err(JournalError::InvalidTransition { .. })
        ));
        j.transition(id, EffectState::InFlight).unwrap();
        j.transition(id, EffectState::Succeeded).unwrap();
        assert_eq!(j.state(&id), Some(EffectState::Succeeded));
        assert_eq!(
            j.transition(eid(), EffectState::InFlight),
            Err(JournalError::Unknown)
        );
    }

    #[test]
    fn deferred_and_failed_can_go_back_to_pending() {
        let mut j = EffectJournal::new();
        let (a, b) = (eid(), eid());
        for id in [a, b] {
            j.begin(id).unwrap();
            j.transition(id, EffectState::InFlight).unwrap();
        }
        j.transition(a, EffectState::Deferred).unwrap();
        j.transition(a, EffectState::Pending).unwrap();
        j.transition(b, EffectState::Failed).unwrap();
        j.transition(b, EffectState::Pending).unwrap();
    }

    #[test]
    fn recovery_reports_in_flight_as_unknown_without_mutating() {
        let (a, b, c) = (eid(), eid(), eid());
        let j = EffectJournal::from_durable([
            (a, EffectState::InFlight),
            (b, EffectState::Succeeded),
            (c, EffectState::Pending),
        ]);
        let r: HashMap<_, _> = j.recover_after_crash().into_iter().collect();
        assert_eq!(r[&a], EffectRecoveryState::UnknownOutcome);
        assert_eq!(r[&b], EffectRecoveryState::ConfirmedSuccess);
        assert_eq!(r[&c], EffectRecoveryState::Pending);
        assert_eq!(j.unknown_outcomes(), vec![a]);
        assert_eq!(j.state(&a), Some(EffectState::InFlight));
    }
}
