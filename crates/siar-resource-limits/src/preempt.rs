//! §153 reservation preemption, §154 preemptible permits, §155 safe pause.
//!
//! Only a hold that is both *marked* `Preemptible` and *able to pause at
//! a safe boundary* (a file transfer between chunks) can ever be a
//! victim. A hold that cannot be paused safely (an identity update
//! mid-commit) is rejected at registration if it claims to be
//! preemptible — so the unsafe combination is unrepresentable, not just
//! discouraged. Preemption is a handshake, not a kill: victims are asked
//! to pause, finish their chunk, and only then give their bytes back.

use std::collections::HashMap;

use crate::admission::WorkPriority;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermitClass {
    Hard,
    Preemptible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseSupport {
    /// Can stop cleanly at a boundary (chunk edge).
    AtBoundary,
    /// Mid-commit work: never pause.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HolderId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderState {
    Running,
    /// Asked to pause; has not yet reached its boundary.
    PauseRequested,
    /// At a boundary; resources may be reclaimed.
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PreemptError {
    #[error("a hold that cannot pause safely may not be marked preemptible")]
    UnsafePreemptible,
    #[error("unknown holder")]
    UnknownHolder,
    #[error("invalid state transition for this holder")]
    InvalidTransition,
    #[error("only {freeable} bytes could be reclaimed, {needed} needed")]
    NotEnough { needed: u64, freeable: u64 },
}

#[derive(Debug, Clone, Copy)]
struct Holder {
    priority: WorkPriority,
    class: PermitClass,
    bytes: u64,
    state: HolderState,
}

#[derive(Debug, Default)]
pub struct PreemptRegistry {
    next: u64,
    holders: HashMap<HolderId, Holder>,
}

impl PreemptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        priority: WorkPriority,
        class: PermitClass,
        pause: PauseSupport,
        bytes: u64,
    ) -> Result<HolderId, PreemptError> {
        if class == PermitClass::Preemptible && pause == PauseSupport::Never {
            return Err(PreemptError::UnsafePreemptible);
        }
        let id = HolderId(self.next);
        self.next += 1;
        self.holders.insert(
            id,
            Holder {
                priority,
                class,
                bytes,
                state: HolderState::Running,
            },
        );
        Ok(id)
    }

    pub fn unregister(&mut self, id: HolderId) -> bool {
        self.holders.remove(&id).is_some()
    }

    pub fn state(&self, id: HolderId) -> Option<HolderState> {
        self.holders.get(&id).map(|h| h.state)
    }

    /// Picks victims to free `needed` bytes for a request of
    /// `requester` priority and marks them `PauseRequested`. Candidates:
    /// preemptible, running, and *strictly* lower priority than the
    /// requester (equal priority never preempts — no ping-pong). Lowest
    /// priority first, then biggest first (fewest victims). Bytes already
    /// held by holders mid-pause count toward `needed`. All-or-nothing: if
    /// the request can't be met, nobody is asked to pause.
    pub fn request_preemption(
        &mut self,
        requester: WorkPriority,
        needed: u64,
    ) -> Result<Vec<HolderId>, PreemptError> {
        let in_flight: u64 = self
            .holders
            .values()
            .filter(|h| matches!(h.state, HolderState::PauseRequested | HolderState::Paused))
            .map(|h| h.bytes)
            .fold(0, u64::saturating_add);
        let mut cands: Vec<(HolderId, Holder)> = self
            .holders
            .iter()
            .filter(|(_, h)| {
                h.class == PermitClass::Preemptible
                    && h.state == HolderState::Running
                    && h.priority > requester
            })
            .map(|(id, h)| (*id, *h))
            .collect();
        // Larger priority value = less important = preempted first.
        cands.sort_by(|(ia, a), (ib, b)| {
            b.priority
                .cmp(&a.priority)
                .then(b.bytes.cmp(&a.bytes))
                .then(ia.cmp(ib))
        });
        let mut got = in_flight;
        let mut victims = Vec::new();
        for (id, h) in cands {
            if got >= needed {
                break;
            }
            got = got.saturating_add(h.bytes);
            victims.push(id);
        }
        if got < needed {
            return Err(PreemptError::NotEnough {
                needed,
                freeable: got,
            });
        }
        for id in &victims {
            if let Some(h) = self.holders.get_mut(id) {
                h.state = HolderState::PauseRequested;
            }
        }
        Ok(victims)
    }

    /// The holder reached a safe boundary. Returns the bytes it must now
    /// release (the caller drops the real permit).
    pub fn reached_boundary(&mut self, id: HolderId) -> Result<u64, PreemptError> {
        let h = self
            .holders
            .get_mut(&id)
            .ok_or(PreemptError::UnknownHolder)?;
        if h.state != HolderState::PauseRequested {
            return Err(PreemptError::InvalidTransition);
        }
        h.state = HolderState::Paused;
        Ok(h.bytes)
    }

    /// Paused holder may continue (it must re-acquire its resources
    /// first; that is the caller's job).
    pub fn resume(&mut self, id: HolderId) -> Result<(), PreemptError> {
        let h = self
            .holders
            .get_mut(&id)
            .ok_or(PreemptError::UnknownHolder)?;
        if h.state != HolderState::Paused {
            return Err(PreemptError::InvalidTransition);
        }
        h.state = HolderState::Running;
        Ok(())
    }

    /// Pause request withdrawn before the boundary was reached.
    pub fn cancel_request(&mut self, id: HolderId) -> Result<(), PreemptError> {
        let h = self
            .holders
            .get_mut(&id)
            .ok_or(PreemptError::UnknownHolder)?;
        if h.state != HolderState::PauseRequested {
            return Err(PreemptError::InvalidTransition);
        }
        h.state = HolderState::Running;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    fn reg() -> PreemptRegistry {
        PreemptRegistry::new()
    }

    #[test]
    fn unsafe_combination_is_unrepresentable() {
        let mut r = reg();
        assert_eq!(
            r.register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::Never,
                10
            ),
            Err(PreemptError::UnsafePreemptible)
        );
        assert!(r
            .register(Normal, PermitClass::Hard, PauseSupport::Never, 10)
            .is_ok());
    }

    #[test]
    fn identity_update_style_holds_are_never_victims() {
        let mut r = reg();
        let _ident = r
            .register(Background, PermitClass::Hard, PauseSupport::Never, 1_000)
            .unwrap();
        let thumb = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                100,
            )
            .unwrap();
        assert_eq!(r.request_preemption(Critical, 100).unwrap(), vec![thumb]);
        assert!(matches!(
            r.request_preemption(Critical, 101),
            Err(PreemptError::NotEnough { .. })
        ));
    }

    #[test]
    fn only_strictly_lower_priority_is_preempted_lowest_then_biggest_first() {
        let mut r = reg();
        let eq = r
            .register(
                Normal,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                500,
            )
            .unwrap();
        let bulk_small = r
            .register(Bulk, PermitClass::Preemptible, PauseSupport::AtBoundary, 50)
            .unwrap();
        let bg_big = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                300,
            )
            .unwrap();
        let bg_small = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                10,
            )
            .unwrap();
        let v = r.request_preemption(Normal, 305).unwrap();
        assert_eq!(
            v,
            vec![bg_big, bg_small],
            "background before bulk, big before small; equal priority untouched"
        );
        assert!(!v.contains(&eq) && !v.contains(&bulk_small));
    }

    #[test]
    fn shortfall_asks_nobody_to_pause() {
        let mut r = reg();
        let a = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                10,
            )
            .unwrap();
        assert!(r.request_preemption(Critical, 11).is_err());
        assert_eq!(r.state(a), Some(HolderState::Running));
    }

    #[test]
    fn pause_handshake_and_resume() {
        let mut r = reg();
        let a = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                40,
            )
            .unwrap();
        assert_eq!(
            r.reached_boundary(a),
            Err(PreemptError::InvalidTransition),
            "not asked yet"
        );
        r.request_preemption(Critical, 40).unwrap();
        assert_eq!(r.state(a), Some(HolderState::PauseRequested));
        assert_eq!(r.reached_boundary(a), Ok(40));
        assert_eq!(r.resume(a), Ok(()));
        assert_eq!(r.state(a), Some(HolderState::Running));
        assert_eq!(r.resume(a), Err(PreemptError::InvalidTransition));
    }

    #[test]
    fn in_flight_pauses_count_so_a_second_requester_does_not_over_preempt() {
        let mut r = reg();
        let a = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                100,
            )
            .unwrap();
        let b = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                100,
            )
            .unwrap();
        assert_eq!(r.request_preemption(Critical, 100).unwrap().len(), 1);
        // already freeing 100: another request for 100 needs no new victims
        assert_eq!(
            r.request_preemption(Critical, 100).unwrap(),
            Vec::<HolderId>::new()
        );
        let _ = (a, b);
    }

    #[test]
    fn cancel_request_and_unknown_ids() {
        let mut r = reg();
        let a = r
            .register(
                Background,
                PermitClass::Preemptible,
                PauseSupport::AtBoundary,
                1,
            )
            .unwrap();
        r.request_preemption(Critical, 1).unwrap();
        r.cancel_request(a).unwrap();
        assert_eq!(r.state(a), Some(HolderState::Running));
        assert!(r.unregister(a));
        assert_eq!(r.resume(a), Err(PreemptError::UnknownHolder));
    }
}
