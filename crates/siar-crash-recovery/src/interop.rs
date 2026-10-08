//! §179 "Recovery and Interoperability".
//!
//! §179 asks that interoperability tests (Part 23) cover three
//! recovery scenarios *across implementations*. This module is the
//! harness for that: [`InteropEndpoint`] is the surface an
//! implementation — this crate, or a wrapper that drives another
//! language's node over a socket — has to expose, and
//! [`run_case`] checks one scenario against it. [`ReferenceEndpoint`]
//! is this crate's own implementation; [`ForgetfulEndpoint`] is a
//! deliberately broken one proving the harness actually catches the
//! failures it exists to catch.

use crate::chunk_recovery::TransferId;
use crate::effect_identity::{IdempotentReceiver, ReceiveKey, ReceiveOutcome};
use crate::reevaluation::VerifiedChunks;
use siar_domain::MessageId;
use std::collections::HashMap;

/// §179's three scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InteropCase {
    DuplicateAfterReconnect,
    RetransmitAfterLostAck,
    ResumeTransferAfterRestart,
}

impl InteropCase {
    pub const ALL: [InteropCase; 3] = [
        InteropCase::DuplicateAfterReconnect,
        InteropCase::RetransmitAfterLostAck,
        InteropCase::ResumeTransferAfterRestart,
    ];
}

/// What a node must expose for the recovery interop cases.
pub trait InteropEndpoint {
    /// Deliver a message with a stable id.
    fn receive_message(&mut self, id: MessageId) -> ReceiveOutcome;
    /// Deliver one chunk of a transfer.
    fn receive_chunk(&mut self, transfer: TransferId, index: u32) -> ReceiveOutcome;
    /// Restart the node (crash + recovery, or reconnect).
    fn restart(&mut self);
    /// Chunks the node considers verified for `transfer`, ascending.
    fn verified_chunks(&self, transfer: TransferId) -> Vec<u32>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InteropFailure {
    #[error("{case:?}: {what}")]
    Violated {
        case: InteropCase,
        what: &'static str,
    },
}

fn fail(case: InteropCase, what: &'static str) -> Result<(), InteropFailure> {
    Err(InteropFailure::Violated { case, what })
}

pub fn run_case<E: InteropEndpoint>(case: InteropCase, e: &mut E) -> Result<(), InteropFailure> {
    use ReceiveOutcome::{Duplicate, Process};
    match case {
        InteropCase::DuplicateAfterReconnect => {
            let m = MessageId::new();
            if e.receive_message(m) != Process {
                return fail(case, "first delivery must be processed");
            }
            e.restart();
            if e.receive_message(m) != Duplicate {
                return fail(case, "duplicate after reconnect was processed again");
            }
            Ok(())
        }
        InteropCase::RetransmitAfterLostAck => {
            let m = MessageId::new();
            if e.receive_message(m) != Process {
                return fail(case, "first delivery must be processed");
            }
            // the ACK is lost, so the sender retransmits — repeatedly
            for _ in 0..3 {
                if e.receive_message(m) != Duplicate {
                    return fail(case, "retransmit after lost ACK was processed again");
                }
            }
            Ok(())
        }
        InteropCase::ResumeTransferAfterRestart => {
            let t = TransferId::new();
            for i in 0..4 {
                if e.receive_chunk(t, i) != Process {
                    return fail(case, "new chunk must be processed");
                }
            }
            e.restart();
            if e.verified_chunks(t) != vec![0, 1, 2, 3] {
                return fail(case, "transfer progress lost or invented across restart");
            }
            for i in 4..6 {
                if e.receive_chunk(t, i) != Process {
                    return fail(case, "resumed chunk must be processed");
                }
            }
            if e.receive_chunk(t, 2) != Duplicate {
                return fail(
                    case,
                    "already-verified chunk re-sent after restart was reprocessed",
                );
            }
            if e.verified_chunks(t) != vec![0, 1, 2, 3, 4, 5] {
                return fail(case, "final verified set wrong");
            }
            Ok(())
        }
    }
}

pub fn run_all<E: InteropEndpoint>(mut make: impl FnMut() -> E) -> Result<(), InteropFailure> {
    for c in InteropCase::ALL {
        run_case(c, &mut make())?;
    }
    Ok(())
}

/// This crate's own node: durable dedup state and chunk progress that
/// survive `restart()`.
#[derive(Debug, Default)]
pub struct ReferenceEndpoint {
    receiver: IdempotentReceiver,
    chunks: HashMap<TransferId, VerifiedChunks>,
}

impl ReferenceEndpoint {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InteropEndpoint for ReferenceEndpoint {
    fn receive_message(&mut self, id: MessageId) -> ReceiveOutcome {
        self.receiver.receive(ReceiveKey::Message(id))
    }

    fn receive_chunk(&mut self, transfer: TransferId, index: u32) -> ReceiveOutcome {
        let outcome = self.receiver.receive(ReceiveKey::Chunk { transfer, index });
        if outcome == ReceiveOutcome::Process {
            self.chunks
                .entry(transfer)
                .or_default()
                .mark_verified(index);
        }
        outcome
    }

    fn restart(&mut self) {
        // Everything above is durable state in a real node; a restart
        // keeps it. (Process-local caches would be dropped here.)
    }

    fn verified_chunks(&self, transfer: TransferId) -> Vec<u32> {
        self.chunks
            .get(&transfer)
            .map(VerifiedChunks::verified_indexes)
            .unwrap_or_default()
    }
}

/// Deliberately broken: forgets everything on restart.
#[derive(Debug, Default)]
pub struct ForgetfulEndpoint(ReferenceEndpoint);

impl InteropEndpoint for ForgetfulEndpoint {
    fn receive_message(&mut self, id: MessageId) -> ReceiveOutcome {
        self.0.receive_message(id)
    }
    fn receive_chunk(&mut self, t: TransferId, i: u32) -> ReceiveOutcome {
        self.0.receive_chunk(t, i)
    }
    fn restart(&mut self) {
        self.0 = ReferenceEndpoint::new();
    }
    fn verified_chunks(&self, t: TransferId) -> Vec<u32> {
        self.0.verified_chunks(t)
    }
}

/// Deliberately broken the other way: processes every retransmit.
#[derive(Debug, Default)]
pub struct NoDedupEndpoint(ReferenceEndpoint);

impl InteropEndpoint for NoDedupEndpoint {
    fn receive_message(&mut self, _id: MessageId) -> ReceiveOutcome {
        ReceiveOutcome::Process
    }
    fn receive_chunk(&mut self, t: TransferId, i: u32) -> ReceiveOutcome {
        self.0.receive_chunk(t, i)
    }
    fn restart(&mut self) {}
    fn verified_chunks(&self, t: TransferId) -> Vec<u32> {
        self.0.verified_chunks(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_implementation_passes_all_three_cases() {
        assert_eq!(run_all(ReferenceEndpoint::new), Ok(()));
    }

    #[test]
    fn a_node_that_forgets_dedup_state_on_restart_fails_the_reconnect_case() {
        let mut e = ForgetfulEndpoint::default();
        assert_eq!(
            run_case(InteropCase::DuplicateAfterReconnect, &mut e),
            Err(InteropFailure::Violated {
                case: InteropCase::DuplicateAfterReconnect,
                what: "duplicate after reconnect was processed again"
            })
        );
    }

    #[test]
    fn a_node_that_forgets_progress_fails_the_resume_case() {
        let mut e = ForgetfulEndpoint::default();
        assert!(matches!(
            run_case(InteropCase::ResumeTransferAfterRestart, &mut e),
            Err(InteropFailure::Violated {
                case: InteropCase::ResumeTransferAfterRestart,
                ..
            })
        ));
    }

    #[test]
    fn a_node_without_dedup_fails_the_lost_ack_case() {
        let mut e = NoDedupEndpoint::default();
        assert!(matches!(
            run_case(InteropCase::RetransmitAfterLostAck, &mut e),
            Err(InteropFailure::Violated {
                case: InteropCase::RetransmitAfterLostAck,
                ..
            })
        ));
    }

    #[test]
    fn the_harness_covers_exactly_the_three_spec_scenarios() {
        assert_eq!(InteropCase::ALL.len(), 3);
    }

    #[test]
    fn verified_chunks_are_reported_ascending_and_without_inventing_any() {
        let mut e = ReferenceEndpoint::new();
        let t = TransferId::new();
        assert!(e.verified_chunks(t).is_empty());
        for i in [5, 1, 3] {
            e.receive_chunk(t, i);
        }
        assert_eq!(e.verified_chunks(t), vec![1, 3, 5]);
    }
}
