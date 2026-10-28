//! §56-§58: command ids and idempotent retry.
//!
//! A mutating command carries a client-chosen [`CommandId`]. If the
//! connection drops after the daemon applied it but before the client saw the
//! answer, the retry must **not** apply it twice: the ledger returns the
//! recorded result instead. Keys are scoped to the authenticated principal so
//! one client can never replay or probe another's ids.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandId(pub u128);

/// Who issued a command (the transport-authenticated principal, e.g. a uid).
pub type Principal = u64;

pub type CommandOutcome = Result<Vec<u8>, RuntimeError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begin {
    /// First time seen: the caller must execute and then [`CommandLedger::complete`].
    New,
    /// Same id is executing right now (a racing retry).
    InFlight,
    /// Already executed: return this, do not re-run.
    Done(CommandOutcome),
}

#[derive(Debug)]
enum Slot {
    InFlight,
    Done(CommandOutcome),
}

/// Bounded ledger. Completed entries are evicted oldest-first; in-flight
/// entries are never evicted (that would re-open a double-apply window).
#[derive(Debug)]
pub struct CommandLedger {
    cap: usize,
    slots: HashMap<(Principal, CommandId), Slot>,
    done_order: VecDeque<(Principal, CommandId)>,
}

impl CommandLedger {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            slots: HashMap::new(),
            done_order: VecDeque::new(),
        }
    }

    pub fn begin(&mut self, who: Principal, id: CommandId) -> Result<Begin, RuntimeError> {
        match self.slots.get(&(who, id)) {
            Some(Slot::InFlight) => return Ok(Begin::InFlight),
            Some(Slot::Done(r)) => return Ok(Begin::Done(r.clone())),
            None => {}
        }
        while self.slots.len() >= self.cap {
            match self.done_order.pop_front() {
                Some(old) => {
                    self.slots.remove(&old);
                }
                None => return Err(RuntimeError::Busy), // full of in-flight work
            }
        }
        self.slots.insert((who, id), Slot::InFlight);
        Ok(Begin::New)
    }

    pub fn complete(&mut self, who: Principal, id: CommandId, outcome: CommandOutcome) {
        if let Some(slot) = self.slots.get_mut(&(who, id)) {
            *slot = Slot::Done(outcome);
            self.done_order.push_back((who, id));
        }
    }

    /// The command was cancelled before taking effect: allow a clean retry.
    pub fn abandon(&mut self, who: Principal, id: CommandId) {
        if matches!(self.slots.get(&(who, id)), Some(Slot::InFlight)) {
            self.slots.remove(&(who, id));
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_returns_the_recorded_result_without_reapplying() {
        let mut l = CommandLedger::new(8);
        assert_eq!(l.begin(1, CommandId(7)), Ok(Begin::New));
        assert_eq!(l.begin(1, CommandId(7)), Ok(Begin::InFlight));
        l.complete(1, CommandId(7), Ok(vec![42]));
        assert_eq!(l.begin(1, CommandId(7)), Ok(Begin::Done(Ok(vec![42]))));
        assert_eq!(l.begin(1, CommandId(7)), Ok(Begin::Done(Ok(vec![42]))));
    }

    #[test]
    fn errors_are_recorded_too() {
        let mut l = CommandLedger::new(8);
        l.begin(1, CommandId(1)).unwrap();
        l.complete(1, CommandId(1), Err(RuntimeError::ReadOnly));
        assert_eq!(
            l.begin(1, CommandId(1)),
            Ok(Begin::Done(Err(RuntimeError::ReadOnly)))
        );
    }

    #[test]
    fn ids_are_scoped_per_principal() {
        let mut l = CommandLedger::new(8);
        l.begin(1, CommandId(1)).unwrap();
        l.complete(1, CommandId(1), Ok(vec![1]));
        assert_eq!(
            l.begin(2, CommandId(1)),
            Ok(Begin::New),
            "another principal's id is independent"
        );
    }

    #[test]
    fn eviction_drops_oldest_completed_never_in_flight() {
        let mut l = CommandLedger::new(2);
        l.begin(1, CommandId(1)).unwrap();
        l.complete(1, CommandId(1), Ok(vec![]));
        l.begin(1, CommandId(2)).unwrap(); // in flight
        assert_eq!(l.begin(1, CommandId(3)), Ok(Begin::New), "evicts id 1");
        assert_eq!(
            l.begin(1, CommandId(2)),
            Ok(Begin::InFlight),
            "in-flight survived"
        );
        assert_eq!(
            l.begin(1, CommandId(4)),
            Err(RuntimeError::Busy),
            "full of in-flight work"
        );
    }

    #[test]
    fn abandon_allows_retry() {
        let mut l = CommandLedger::new(4);
        l.begin(1, CommandId(1)).unwrap();
        l.abandon(1, CommandId(1));
        assert_eq!(l.begin(1, CommandId(1)), Ok(Begin::New));
    }
}
