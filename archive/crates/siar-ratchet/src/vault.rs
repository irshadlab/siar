//! §24, §103: stale-state detection.
//!
//! Restoring an old backup brings back an old ratchet state. Using it would
//! re-derive message keys that were already used (and nonces with them). The
//! defence is a **high-water mark kept outside the backup set** (the platform
//! key store, a monotonic counter, a server-visible epoch): a state whose
//! epoch is below the mark is refused and the session is re-established.

use std::collections::HashMap;

use crate::state::{RatchetError, RatchetState};

#[derive(Debug, Default)]
pub struct HighWaterMarks {
    marks: HashMap<[u8; 16], u64>,
}

impl HighWaterMarks {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Call when loading a session's state.
    pub fn check(&self, session: [u8; 16], state: &RatchetState) -> Result<(), RatchetError> {
        match self.marks.get(&session) {
            Some(&hw) if state.epoch() < hw => Err(RatchetError::StaleState),
            _ => Ok(()),
        }
    }

    /// Call when a new state is durably stored. Never moves backwards.
    pub fn record(&mut self, session: [u8; 16], state: &RatchetState) {
        let e = self.marks.entry(session).or_insert(0);
        *e = (*e).max(state.epoch());
    }
}
