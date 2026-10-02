//! §66 DTN backpressure (relay intake) and §67 encounter budget.

use crate::admission::WorkPriority;
use crate::storage::PressureState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayIntake {
    pub priority: WorkPriority,
    pub bundle_bytes: u64,
    pub relay_quota_remaining: u64,
    pub peer_quota_remaining: u64,
    pub storage: PressureState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeReject {
    PeerQuota,
    RelayQuotaFull,
    StoragePressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeDecision {
    Accept,
    /// Accept, but the store must first evict stored bundles of priority
    /// strictly *lower* than this one to make room (§66 "evicted first").
    AcceptEvictingBelow(WorkPriority),
    Reject(IntakeReject),
}

/// §66 order: peer quota → storage pressure → relay quota → priority.
/// The peer's own quota is checked first: a peer over its share is
/// rejected no matter how important it claims to be, so it can't use
/// priority to push other peers' bundles out.
pub fn relay_intake(i: &RelayIntake) -> IntakeDecision {
    if i.bundle_bytes > i.peer_quota_remaining {
        return IntakeDecision::Reject(IntakeReject::PeerQuota);
    }
    // Under storage pressure only increasingly important work is taken.
    let min_priority = match i.storage {
        PressureState::Normal => WorkPriority::Background,
        PressureState::Elevated => WorkPriority::Bulk,
        PressureState::Critical => WorkPriority::Interactive,
        PressureState::Exhausted => WorkPriority::Control,
    };
    if i.priority > min_priority {
        return IntakeDecision::Reject(IntakeReject::StoragePressure);
    }
    if i.bundle_bytes <= i.relay_quota_remaining {
        return IntakeDecision::Accept;
    }
    // Relay quota full: only displace strictly less important bundles.
    if i.priority == WorkPriority::Background {
        IntakeDecision::Reject(IntakeReject::RelayQuotaFull)
    } else {
        IntakeDecision::AcceptEvictingBelow(i.priority)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EncounterExhausted {
    #[error("encounter byte budget exhausted")]
    Bytes,
    #[error("encounter bundle-count budget exhausted")]
    Bundles,
}

/// §67: a short contact exchanges at most N bytes / M bundles, so one
/// chatty peer can't consume the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncounterBudget {
    bytes_left: u64,
    bundles_left: u32,
}

impl EncounterBudget {
    pub fn new(max_bytes: u64, max_bundles: u32) -> Self {
        Self {
            bytes_left: max_bytes,
            bundles_left: max_bundles,
        }
    }

    /// All-or-nothing: a failed spend changes nothing.
    pub fn try_spend(&mut self, bytes: u64) -> Result<(), EncounterExhausted> {
        if self.bundles_left == 0 {
            return Err(EncounterExhausted::Bundles);
        }
        if bytes > self.bytes_left {
            return Err(EncounterExhausted::Bytes);
        }
        self.bytes_left -= bytes;
        self.bundles_left -= 1;
        Ok(())
    }

    pub fn bytes_left(&self) -> u64 {
        self.bytes_left
    }

    pub fn bundles_left(&self) -> u32 {
        self.bundles_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    fn base(p: WorkPriority) -> RelayIntake {
        RelayIntake {
            priority: p,
            bundle_bytes: 100,
            relay_quota_remaining: 1_000,
            peer_quota_remaining: 1_000,
            storage: PressureState::Normal,
        }
    }

    #[test]
    fn healthy_relay_accepts() {
        assert_eq!(relay_intake(&base(Normal)), IntakeDecision::Accept);
    }

    #[test]
    fn peer_quota_beats_priority() {
        let mut i = base(Critical);
        i.peer_quota_remaining = 50;
        assert_eq!(
            relay_intake(&i),
            IntakeDecision::Reject(IntakeReject::PeerQuota)
        );
    }

    #[test]
    fn storage_pressure_sheds_low_priority_first() {
        let mut i = base(Bulk);
        i.storage = PressureState::Critical;
        assert_eq!(
            relay_intake(&i),
            IntakeDecision::Reject(IntakeReject::StoragePressure)
        );
        i.priority = Interactive;
        assert_eq!(relay_intake(&i), IntakeDecision::Accept);
        i.storage = PressureState::Exhausted;
        assert_eq!(
            relay_intake(&i),
            IntakeDecision::Reject(IntakeReject::StoragePressure)
        );
        i.priority = Critical;
        assert_eq!(relay_intake(&i), IntakeDecision::Accept);
    }

    #[test]
    fn full_relay_evicts_only_for_something_displaceable() {
        let mut i = base(Normal);
        i.relay_quota_remaining = 10;
        assert_eq!(
            relay_intake(&i),
            IntakeDecision::AcceptEvictingBelow(Normal)
        );
        i.priority = Background;
        assert_eq!(
            relay_intake(&i),
            IntakeDecision::Reject(IntakeReject::RelayQuotaFull)
        );
    }

    #[test]
    fn encounter_budget_is_all_or_nothing_and_bounded_both_ways() {
        let mut b = EncounterBudget::new(100, 2);
        assert_eq!(b.try_spend(101), Err(EncounterExhausted::Bytes));
        assert_eq!((b.bytes_left(), b.bundles_left()), (100, 2));
        b.try_spend(60).unwrap();
        b.try_spend(1).unwrap();
        assert_eq!(b.try_spend(1), Err(EncounterExhausted::Bundles));
        assert_eq!(b.bytes_left(), 39);
    }
}
