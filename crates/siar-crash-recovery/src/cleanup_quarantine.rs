//! §109 "Cleanup Order" and §110 "Quarantine".
//!
//! §109's warning is "do not aggressively GC before establishing
//! authoritative references". [`CleanupSequencer`] makes the order a
//! checked sequence and exposes [`CleanupSequencer::gc_allowed`], true
//! only once every phase that *establishes* references is done.
//! §110: suspicious artifacts go to a size-bounded quarantine instead
//! of being deleted outright.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum CleanupPhase {
    DbIntegrity = 0,
    Identity = 1,
    EventAndOutbox = 2,
    TransferStaging = 3,
    Dtn = 4,
    CacheAndTempCleanup = 5,
}

impl CleanupPhase {
    pub const ORDER: [CleanupPhase; 6] = [
        CleanupPhase::DbIntegrity,
        CleanupPhase::Identity,
        CleanupPhase::EventAndOutbox,
        CleanupPhase::TransferStaging,
        CleanupPhase::Dtn,
        CleanupPhase::CacheAndTempCleanup,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cleanup phase {attempted:?} attempted before {expected:?}")]
pub struct OutOfOrder {
    pub attempted: CleanupPhase,
    pub expected: CleanupPhase,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CleanupSequencer {
    completed: usize,
}

impl CleanupSequencer {
    pub const fn new() -> Self {
        Self { completed: 0 }
    }

    pub fn next_phase(&self) -> Option<CleanupPhase> {
        CleanupPhase::ORDER.get(self.completed).copied()
    }

    /// Marks `phase` done. Must be exactly the next phase; re-marking
    /// an already-completed phase is an error too (it would mean a
    /// phase ran twice or out of band).
    pub fn complete(&mut self, phase: CleanupPhase) -> Result<(), OutOfOrder> {
        match self.next_phase() {
            Some(exp) if exp == phase => {
                self.completed += 1;
                Ok(())
            }
            Some(exp) => Err(OutOfOrder {
                attempted: phase,
                expected: exp,
            }),
            None => Err(OutOfOrder {
                attempted: phase,
                expected: CleanupPhase::CacheAndTempCleanup,
            }),
        }
    }

    /// Destructive GC/temp deletion may run only once DB, identity,
    /// events/outbox, staging and DTN have all established what is
    /// referenced.
    pub const fn gc_allowed(&self) -> bool {
        self.completed >= CleanupPhase::CacheAndTempCleanup as usize
    }

    pub const fn is_complete(&self) -> bool {
        self.completed == CleanupPhase::ORDER.len()
    }
}

// ---------------------------------------------------------------------
// §110
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantineEntry {
    pub id: u64,
    pub bytes: u64,
    pub quarantined_at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineOutcome {
    /// Stored; `evicted` lists the oldest entries removed to make room.
    Admitted { evicted: Vec<u64> },
    /// Larger than the whole quarantine. The artifact must stay where
    /// it is (and be reported) — it is not silently deleted.
    TooLarge,
}

#[derive(Debug, Clone)]
pub struct Quarantine {
    max_bytes: u64,
    used: u64,
    entries: VecDeque<QuarantineEntry>,
}

impl Quarantine {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            used: 0,
            entries: VecDeque::new(),
        }
    }

    pub const fn used_bytes(&self) -> u64 {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bounded: oldest entries are evicted first to stay within
    /// `max_bytes`.
    pub fn admit(&mut self, e: QuarantineEntry) -> QuarantineOutcome {
        if e.bytes > self.max_bytes {
            return QuarantineOutcome::TooLarge;
        }
        let mut evicted = Vec::new();
        while self.used.saturating_add(e.bytes) > self.max_bytes {
            match self.entries.pop_front() {
                Some(old) => {
                    self.used -= old.bytes;
                    evicted.push(old.id);
                }
                None => break,
            }
        }
        self.used += e.bytes;
        self.entries.push_back(e);
        QuarantineOutcome::Admitted { evicted }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_runs_in_the_spec_order() {
        let mut s = CleanupSequencer::new();
        for p in CleanupPhase::ORDER {
            assert_eq!(s.next_phase(), Some(p));
            s.complete(p).unwrap();
        }
        assert!(s.is_complete());
        assert_eq!(s.next_phase(), None);
    }

    #[test]
    fn skipping_ahead_is_refused() {
        let mut s = CleanupSequencer::new();
        assert_eq!(
            s.complete(CleanupPhase::CacheAndTempCleanup),
            Err(OutOfOrder {
                attempted: CleanupPhase::CacheAndTempCleanup,
                expected: CleanupPhase::DbIntegrity
            })
        );
        s.complete(CleanupPhase::DbIntegrity).unwrap();
        assert!(s.complete(CleanupPhase::DbIntegrity).is_err());
    }

    #[test]
    fn gc_is_forbidden_until_references_are_established() {
        let mut s = CleanupSequencer::new();
        for p in &CleanupPhase::ORDER[..5] {
            assert!(!s.gc_allowed(), "before {p:?}");
            s.complete(*p).unwrap();
        }
        assert!(s.gc_allowed(), "all reference-establishing phases done");
        s.complete(CleanupPhase::CacheAndTempCleanup).unwrap();
        assert!(s.gc_allowed());
    }

    fn q(id: u64, bytes: u64) -> QuarantineEntry {
        QuarantineEntry {
            id,
            bytes,
            quarantined_at_millis: id,
        }
    }

    #[test]
    fn quarantine_admits_within_budget() {
        let mut z = Quarantine::new(100);
        assert_eq!(
            z.admit(q(1, 40)),
            QuarantineOutcome::Admitted { evicted: vec![] }
        );
        assert_eq!(
            z.admit(q(2, 60)),
            QuarantineOutcome::Admitted { evicted: vec![] }
        );
        assert_eq!(z.used_bytes(), 100);
    }

    #[test]
    fn quarantine_is_bounded_by_evicting_oldest_first() {
        let mut z = Quarantine::new(100);
        z.admit(q(1, 40));
        z.admit(q(2, 40));
        assert_eq!(
            z.admit(q(3, 50)),
            QuarantineOutcome::Admitted { evicted: vec![1] }
        );
        assert_eq!(z.used_bytes(), 90);
        assert_eq!(
            z.admit(q(4, 100)),
            QuarantineOutcome::Admitted {
                evicted: vec![2, 3]
            }
        );
        assert_eq!((z.len(), z.used_bytes()), (1, 100));
    }

    #[test]
    fn oversize_artifact_is_refused_not_destroyed() {
        let mut z = Quarantine::new(100);
        z.admit(q(1, 10));
        assert_eq!(z.admit(q(2, 101)), QuarantineOutcome::TooLarge);
        assert_eq!(
            (z.len(), z.used_bytes()),
            (1, 10),
            "existing entries untouched"
        );
    }
}
