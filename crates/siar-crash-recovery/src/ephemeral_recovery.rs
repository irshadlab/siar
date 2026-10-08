//! §103 "Recovery of Media Calls", §104 "Call History", §105
//! "Presence Recovery".
//!
//! Live and ephemeral state is *not* recovered, it is ended or
//! discarded and recomputed. A crashed call is never resumed from
//! stale RTP state; typing and presence are never persisted at all.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId(uuid::Uuid);

impl CallId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for CallId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallState {
    Ringing,
    Connecting,
    Active,
    Ended,
    Interrupted,
}

impl CallState {
    pub const fn is_terminal(self) -> bool {
        matches!(self, CallState::Ended | CallState::Interrupted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallRecovery {
    /// Non-terminal at crash time → interrupted.
    MarkInterrupted,
    /// Already finished before the crash.
    Unchanged,
}

/// §103: there is no `Resume` outcome — by construction.
pub const fn recover_call(s: CallState) -> CallRecovery {
    if s.is_terminal() {
        CallRecovery::Unchanged
    } else {
        CallRecovery::MarkInterrupted
    }
}

/// §103: "do not attempt to resume RTP state blindly."
pub const fn may_resume_media_session(_s: CallState) -> bool {
    false
}

/// §104's semantic event; distinct from any live-media recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallHistoryEvent {
    CallInterrupted { call: CallId, at_millis: u64 },
}

pub const fn history_event_for_recovery(
    call: CallId,
    state: CallState,
    now_millis: u64,
) -> Option<CallHistoryEvent> {
    match recover_call(state) {
        CallRecovery::MarkInterrupted => Some(CallHistoryEvent::CallInterrupted {
            call,
            at_millis: now_millis,
        }),
        CallRecovery::Unchanged => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EphemeralKind {
    Presence,
    Typing,
}

/// §105: ephemeral state is never written to durable storage.
pub const fn is_persistable(_k: EphemeralKind) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresenceRecovery {
    /// Stale entries thrown away.
    pub discarded: usize,
    /// Always true: the current presence is recomputed fresh.
    pub recompute_current: bool,
}

/// Whatever presence/typing entries a (buggy or legacy) store still
/// holds after a restart are all stale.
pub const fn recover_presence(stale_entries: usize) -> PresenceRecovery {
    PresenceRecovery {
        discarded: stale_entries,
        recompute_current: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_live_call_state_is_interrupted_never_resumed() {
        for s in [CallState::Ringing, CallState::Connecting, CallState::Active] {
            assert_eq!(recover_call(s), CallRecovery::MarkInterrupted, "{s:?}");
            assert!(!may_resume_media_session(s));
        }
    }

    #[test]
    fn finished_calls_are_left_alone() {
        assert_eq!(recover_call(CallState::Ended), CallRecovery::Unchanged);
        assert_eq!(
            recover_call(CallState::Interrupted),
            CallRecovery::Unchanged
        );
    }

    #[test]
    fn interrupted_event_is_emitted_exactly_for_live_calls() {
        let id = CallId::new();
        assert_eq!(
            history_event_for_recovery(id, CallState::Active, 99),
            Some(CallHistoryEvent::CallInterrupted {
                call: id,
                at_millis: 99
            })
        );
        assert_eq!(history_event_for_recovery(id, CallState::Ended, 99), None);
        assert_eq!(
            history_event_for_recovery(id, CallState::Interrupted, 99),
            None
        );
    }

    #[test]
    fn recovery_is_idempotent_across_repeated_restarts() {
        // first restart interrupts; second restart sees a terminal state
        let after_first = match recover_call(CallState::Active) {
            CallRecovery::MarkInterrupted => CallState::Interrupted,
            CallRecovery::Unchanged => CallState::Active,
        };
        assert_eq!(recover_call(after_first), CallRecovery::Unchanged);
    }

    #[test]
    fn presence_and_typing_are_never_persisted() {
        assert!(!is_persistable(EphemeralKind::Presence));
        assert!(!is_persistable(EphemeralKind::Typing));
    }

    #[test]
    fn stale_presence_is_discarded_and_recomputed() {
        let r = recover_presence(12);
        assert_eq!(r.discarded, 12);
        assert!(r.recompute_current);
    }
}
