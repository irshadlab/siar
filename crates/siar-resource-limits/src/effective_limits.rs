//! §121 limit negotiation, §122 runtime tightening.
//!
//! ```text
//! effective = min(local hard limit, local policy,
//!                 negotiated remote limit, current pressure limit)
//! ```
//! Pressure is applied as a *local* throttle on top of whatever was
//! negotiated: negotiated parallel chunks = 8, Critical pressure →
//! effective 2, and nothing is renegotiated — unless the peer's own
//! behaviour depends on the number (then [`RuntimeLimit::renegotiation_needed`]
//! says so). When pressure eases the effective value climbs back toward
//! the negotiated one without any message exchange.

use crate::storage::PressureState;

/// Which input is the binding constraint. Ties are attributed to the
/// highest-authority source, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitSource {
    Hard,
    Policy,
    Negotiated,
    Pressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveLimit {
    pub value: u64,
    pub binding: LimitSource,
}

pub fn effective_limit(
    hard: u64,
    policy: u64,
    negotiated: Option<u64>,
    pressure: Option<u64>,
) -> EffectiveLimit {
    let mut best = EffectiveLimit {
        value: hard,
        binding: LimitSource::Hard,
    };
    for (v, src) in [
        (Some(policy), LimitSource::Policy),
        (negotiated, LimitSource::Negotiated),
        (pressure, LimitSource::Pressure),
    ] {
        if let Some(v) = v {
            if v < best.value {
                best = EffectiveLimit {
                    value: v,
                    binding: src,
                };
            }
        }
    }
    best
}

/// Share of a limit that remains usable at a pressure level. A nonzero
/// limit never drops to zero here — total shutdown of a resource is the
/// job of the load shedder / modes, not of proportional throttling.
pub fn pressure_limit_for(base: u64, state: PressureState) -> u64 {
    let pct: u128 = match state {
        PressureState::Normal => 100,
        PressureState::Elevated => 50,
        PressureState::Critical => 25,
        PressureState::Exhausted => 10,
    };
    let v = (u128::from(base) * pct / 100) as u64;
    if base > 0 {
        v.max(1)
    } else {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimit {
    hard: u64,
    policy: u64,
    negotiated: Option<u64>,
    pressure: PressureState,
    peer_behaviour_depends: bool,
}

impl RuntimeLimit {
    /// `peer_behaviour_depends`: does the remote side's behaviour rely on
    /// the negotiated number (e.g. it pipelines exactly that many chunks
    /// and expects acks accordingly)?
    pub fn new(hard: u64, policy: u64, peer_behaviour_depends: bool) -> Self {
        Self {
            hard,
            policy,
            negotiated: None,
            pressure: PressureState::Normal,
            peer_behaviour_depends,
        }
    }

    pub fn set_negotiated(&mut self, v: u64) {
        self.negotiated = Some(v);
    }

    pub fn set_pressure(&mut self, p: PressureState) {
        self.pressure = p;
    }

    pub fn effective(&self) -> EffectiveLimit {
        let base = effective_limit(self.hard, self.policy, self.negotiated, None);
        effective_limit(
            self.hard,
            self.policy,
            self.negotiated,
            Some(pressure_limit_for(base.value, self.pressure)),
        )
    }

    /// Only true when we are running below what we negotiated *and* the
    /// peer cares. Pure local throttling needs no renegotiation (§122).
    pub fn renegotiation_needed(&self) -> bool {
        self.peer_behaviour_depends && self.negotiated.is_some_and(|n| self.effective().value < n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_of_all_four_with_authority_ordered_ties() {
        let e = effective_limit(100, 50, Some(8), Some(2));
        assert_eq!(
            e,
            EffectiveLimit {
                value: 2,
                binding: LimitSource::Pressure
            }
        );
        assert_eq!(
            effective_limit(100, 50, Some(8), None).binding,
            LimitSource::Negotiated
        );
        assert_eq!(
            effective_limit(100, 50, None, None).binding,
            LimitSource::Policy
        );
        assert_eq!(
            effective_limit(10, 50, Some(80), Some(99)).binding,
            LimitSource::Hard
        );
        // ties go to the higher-authority owner
        assert_eq!(
            effective_limit(8, 8, Some(8), Some(8)).binding,
            LimitSource::Hard
        );
        assert_eq!(
            effective_limit(100, 8, Some(8), Some(8)).binding,
            LimitSource::Policy
        );
    }

    #[test]
    fn spec_example_negotiated_8_pressure_gives_2_without_renegotiating() {
        let mut l = RuntimeLimit::new(64, 32, false);
        l.set_negotiated(8);
        assert_eq!(l.effective().value, 8);
        l.set_pressure(PressureState::Critical);
        assert_eq!(
            l.effective(),
            EffectiveLimit {
                value: 2,
                binding: LimitSource::Pressure
            }
        );
        assert!(!l.renegotiation_needed(), "local throttle only");
        l.set_pressure(PressureState::Normal);
        assert_eq!(l.effective().value, 8, "recovers with no message exchange");
    }

    #[test]
    fn renegotiation_only_when_the_peer_depends_on_the_number() {
        let mut l = RuntimeLimit::new(64, 32, true);
        l.set_negotiated(8);
        assert!(!l.renegotiation_needed(), "at the negotiated value");
        l.set_pressure(PressureState::Elevated);
        assert!(l.renegotiation_needed());
        l.set_pressure(PressureState::Normal);
        assert!(!l.renegotiation_needed());
    }

    #[test]
    fn a_remote_cannot_negotiate_above_our_hard_or_policy_limits() {
        let mut l = RuntimeLimit::new(16, 4, false);
        l.set_negotiated(1_000_000);
        assert_eq!(
            l.effective(),
            EffectiveLimit {
                value: 4,
                binding: LimitSource::Policy
            }
        );
    }

    #[test]
    fn proportional_throttling_never_reaches_zero_for_a_nonzero_limit() {
        for st in [
            PressureState::Normal,
            PressureState::Elevated,
            PressureState::Critical,
            PressureState::Exhausted,
        ] {
            assert_eq!(pressure_limit_for(0, st), 0);
            assert!(pressure_limit_for(1, st) >= 1);
        }
        assert_eq!(pressure_limit_for(100, PressureState::Exhausted), 10);
        assert_eq!(
            pressure_limit_for(u64::MAX, PressureState::Normal),
            u64::MAX
        );
    }

    #[test]
    fn throttling_is_monotonic_in_pressure() {
        let levels = [
            PressureState::Normal,
            PressureState::Elevated,
            PressureState::Critical,
            PressureState::Exhausted,
        ];
        for base in [1u64, 3, 8, 100, 12_345] {
            let v: Vec<u64> = levels
                .iter()
                .map(|s| pressure_limit_for(base, *s))
                .collect();
            assert!(v.windows(2).all(|w| w[0] >= w[1]), "{base}: {v:?}");
        }
    }
}
