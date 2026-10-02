//! §45-48, §95-96: dynamic capability updates.
//!
//! The local side numbers every change with a strictly increasing
//! [`CapabilityEpoch`]; the peer side applies a delta only if it is the
//! *next* epoch. Anything older is stale (ignored — replay-safe), anything
//! that skips an epoch means a delta was missed and the peer must request
//! a full snapshot (§48, §96). Application is atomic: a malformed update
//! changes nothing.

use serde::{Deserialize, Serialize};

use crate::descriptor::CapabilityDescriptor;
use crate::id::CapabilityId;
use crate::set::{CapabilitySet, CapabilitySetError};

/// §47: "Keep updates bounded."
pub const MAX_DELTAS_PER_UPDATE: usize = 64;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct CapabilityEpoch(pub u64);

impl CapabilityEpoch {
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CapabilityDelta {
    /// Add, or replace an existing descriptor of the same id.
    Upsert(CapabilityDescriptor),
    Remove(CapabilityId),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityUpdate {
    pub epoch: CapabilityEpoch,
    pub changed: Vec<CapabilityDelta>,
    /// Stand-in for §47's `SessionBinding` (Part 02 wiring not built):
    /// an opaque tag the receiver compares against its own session's.
    pub session_binding: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EpochError {
    #[error("update carries {0} deltas, more than {MAX_DELTAS_PER_UPDATE}")]
    TooManyDeltas(usize),
    #[error("update is bound to a different session")]
    WrongSession,
    #[error("epoch counter exhausted")]
    EpochExhausted,
    #[error(transparent)]
    Set(#[from] CapabilitySetError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    /// Epoch ≤ current: duplicate or replay. State untouched.
    Stale,
    /// Epoch skipped ahead: request a full snapshot (§48).
    MissedUpdate {
        have: CapabilityEpoch,
        got: CapabilityEpoch,
    },
}

/// The local node's advertised view; produces updates.
#[derive(Debug, Clone, Default)]
pub struct LocalCapabilityState {
    epoch: CapabilityEpoch,
    set: CapabilitySet,
}

impl LocalCapabilityState {
    pub fn new(set: CapabilitySet) -> Self {
        Self {
            epoch: CapabilityEpoch(0),
            set,
        }
    }

    pub fn epoch(&self) -> CapabilityEpoch {
        self.epoch
    }

    pub fn snapshot(&self) -> (CapabilityEpoch, &CapabilitySet) {
        (self.epoch, &self.set)
    }

    /// Applies `changes` locally and returns the update to send. Nothing
    /// is committed if any step fails.
    pub fn update(
        &mut self,
        changes: Vec<CapabilityDelta>,
        session_binding: [u8; 32],
    ) -> Result<CapabilityUpdate, EpochError> {
        if changes.len() > MAX_DELTAS_PER_UPDATE {
            return Err(EpochError::TooManyDeltas(changes.len()));
        }
        let epoch = self.epoch.next().ok_or(EpochError::EpochExhausted)?;
        let mut next = self.set.clone();
        apply_deltas(&mut next, &changes)?;
        self.set = next;
        self.epoch = epoch;
        Ok(CapabilityUpdate {
            epoch,
            changed: changes,
            session_binding,
        })
    }
}

fn apply_deltas(set: &mut CapabilitySet, deltas: &[CapabilityDelta]) -> Result<(), EpochError> {
    for d in deltas {
        match d {
            CapabilityDelta::Upsert(desc) => set.insert(desc.clone())?,
            CapabilityDelta::Remove(id) => {
                set.remove(id);
            }
        }
    }
    Ok(())
}

/// What we currently believe about a peer.
#[derive(Debug, Clone, Default)]
pub struct PeerCapabilityView {
    epoch: CapabilityEpoch,
    set: CapabilitySet,
}

impl PeerCapabilityView {
    pub fn from_snapshot(epoch: CapabilityEpoch, set: CapabilitySet) -> Self {
        Self { epoch, set }
    }

    pub fn epoch(&self) -> CapabilityEpoch {
        self.epoch
    }

    pub fn set(&self) -> &CapabilitySet {
        &self.set
    }

    /// Replace with a full snapshot (§48 recovery). A snapshot older than
    /// what we hold is refused, so a replayed old snapshot can't roll the
    /// view back. Returns whether it was installed.
    pub fn install_snapshot(&mut self, epoch: CapabilityEpoch, set: CapabilitySet) -> bool {
        if epoch < self.epoch {
            return false;
        }
        self.epoch = epoch;
        self.set = set;
        true
    }

    pub fn apply(
        &mut self,
        update: &CapabilityUpdate,
        expected_binding: &[u8; 32],
    ) -> Result<ApplyOutcome, EpochError> {
        if update.changed.len() > MAX_DELTAS_PER_UPDATE {
            return Err(EpochError::TooManyDeltas(update.changed.len()));
        }
        if !constant_time_eq(&update.session_binding, expected_binding) {
            return Err(EpochError::WrongSession);
        }
        if update.epoch <= self.epoch {
            return Ok(ApplyOutcome::Stale);
        }
        if self.epoch.next() != Some(update.epoch) {
            return Ok(ApplyOutcome::MissedUpdate {
                have: self.epoch,
                got: update.epoch,
            });
        }
        let mut next = self.set.clone();
        apply_deltas(&mut next, &update.changed)?;
        self.set = next;
        self.epoch = update.epoch;
        Ok(ApplyOutcome::Applied)
    }
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityNamespace;
    use crate::version::CapabilityVersion;

    const BIND: [u8; 32] = [7; 32];

    fn desc(code: u32) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new(CapabilityNamespace::Messaging, code),
            CapabilityVersion::new(1, 0),
            CapabilityRequirement::Optional,
            CapabilityParameters::None,
        )
    }

    fn pair() -> (LocalCapabilityState, PeerCapabilityView) {
        let mut set = CapabilitySet::new();
        set.insert(desc(1)).unwrap();
        (
            LocalCapabilityState::new(set.clone()),
            PeerCapabilityView::from_snapshot(CapabilityEpoch(0), set),
        )
    }

    #[test]
    fn deltas_apply_in_order_and_converge() {
        let (mut local, mut peer) = pair();
        let u1 = local
            .update(vec![CapabilityDelta::Upsert(desc(2))], BIND)
            .unwrap();
        let u2 = local
            .update(vec![CapabilityDelta::Remove(desc(1).id)], BIND)
            .unwrap();
        assert_eq!(peer.apply(&u1, &BIND).unwrap(), ApplyOutcome::Applied);
        assert_eq!(peer.apply(&u2, &BIND).unwrap(), ApplyOutcome::Applied);
        assert_eq!(peer.set(), local.snapshot().1);
        assert_eq!(peer.epoch(), local.epoch());
    }

    #[test]
    fn replayed_update_is_stale_and_changes_nothing() {
        let (mut local, mut peer) = pair();
        let u1 = local
            .update(vec![CapabilityDelta::Upsert(desc(2))], BIND)
            .unwrap();
        peer.apply(&u1, &BIND).unwrap();
        let before = peer.set().clone();
        assert_eq!(peer.apply(&u1, &BIND).unwrap(), ApplyOutcome::Stale);
        assert_eq!(peer.set(), &before);
    }

    #[test]
    fn skipped_epoch_requests_snapshot_then_recovers() {
        let (mut local, mut peer) = pair();
        let _lost = local
            .update(vec![CapabilityDelta::Upsert(desc(2))], BIND)
            .unwrap();
        let u2 = local
            .update(vec![CapabilityDelta::Upsert(desc(3))], BIND)
            .unwrap();
        assert_eq!(
            peer.apply(&u2, &BIND).unwrap(),
            ApplyOutcome::MissedUpdate {
                have: CapabilityEpoch(0),
                got: CapabilityEpoch(2)
            }
        );
        let (e, s) = local.snapshot();
        assert!(peer.install_snapshot(e, s.clone()));
        assert_eq!(peer.set(), local.snapshot().1);
    }

    #[test]
    fn wrong_session_binding_is_rejected() {
        let (mut local, mut peer) = pair();
        let u = local.update(vec![], BIND).unwrap();
        assert_eq!(peer.apply(&u, &[0; 32]), Err(EpochError::WrongSession));
        assert_eq!(peer.epoch(), CapabilityEpoch(0));
    }

    #[test]
    fn oversized_update_is_rejected_both_sides() {
        let (mut local, mut peer) = pair();
        let many: Vec<_> = (0..MAX_DELTAS_PER_UPDATE as u32 + 1)
            .map(|i| CapabilityDelta::Upsert(desc(i + 10)))
            .collect();
        assert!(matches!(
            local.update(many.clone(), BIND),
            Err(EpochError::TooManyDeltas(_))
        ));
        let forged = CapabilityUpdate {
            epoch: CapabilityEpoch(1),
            changed: many,
            session_binding: BIND,
        };
        assert!(matches!(
            peer.apply(&forged, &BIND),
            Err(EpochError::TooManyDeltas(_))
        ));
    }

    #[test]
    fn failed_apply_is_atomic() {
        use crate::set::MAX_CAPABILITIES_PER_SET;
        // One slot free, then an update that needs two: the first delta
        // would succeed, the second must fail, and nothing may stick.
        let mut full = CapabilitySet::new();
        for i in 0..(MAX_CAPABILITIES_PER_SET as u32 - 1) {
            full.insert(desc(i + 1_000)).unwrap();
        }
        let mut peer = PeerCapabilityView::from_snapshot(CapabilityEpoch(0), full.clone());
        let upd = CapabilityUpdate {
            epoch: CapabilityEpoch(1),
            changed: vec![
                CapabilityDelta::Upsert(desc(1)),
                CapabilityDelta::Upsert(desc(2)),
            ],
            session_binding: BIND,
        };
        assert!(matches!(peer.apply(&upd, &BIND), Err(EpochError::Set(_))));
        assert_eq!(peer.set(), &full, "partial application leaked");
        assert_eq!(peer.epoch(), CapabilityEpoch(0));
    }

    #[test]
    fn stale_snapshot_cannot_roll_back() {
        let (_, mut peer) = pair();
        peer.install_snapshot(CapabilityEpoch(5), CapabilitySet::new());
        assert!(!peer.install_snapshot(CapabilityEpoch(3), CapabilitySet::new()));
        assert_eq!(peer.epoch(), CapabilityEpoch(5));
    }
}
