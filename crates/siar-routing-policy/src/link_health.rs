//! Numeric rolling-window link health — ported from the retired
//! `siar-routing::link_health::LinkHealth` (see `MIGRATION.md`, step
//! 2). Real gap this crate's own [`crate::engine::health_after_outcome`]
//! never closed: that function only ever turns a [`crate::engine::
//! RouteOutcome`] event into a qualitative [`crate::types::RouteHealth`]
//! state (`Healthy`/`Degraded`/`Suspect`/`Unreachable`) — it has never
//! produced a number. [`crate::metrics::PathMetrics`] has had
//! `rtt_millis`/`packet_loss` fields since round 1, but nothing in
//! this crate has ever computed them from a bounded history of real
//! send outcomes; every existing candidate either carries `None` or a
//! caller-supplied one-off value.
//!
//! [`LinkHealth`] is that missing recorder — pure logic, no I/O, same
//! shape as everything else in this crate: a caller with a real
//! connection attempt (timed, success/failure known) calls
//! [`LinkHealth::record_outcome`]; [`LinkHealth::apply_to`] then folds
//! the resulting rolling reliability/RTT onto an existing
//! [`crate::metrics::PathMetrics`] in place, rather than this module
//! inventing a second metrics-shaped struct that would need to be kept
//! in sync with the real one by hand. [`health_from_reliability`]
//! closes the qualitative side of the same gap: turning this window's
//! own reliability fraction into a [`crate::types::RouteHealth`] a
//! caller can set directly on a [`crate::candidate::PathCandidate`]
//! without waiting for a `RouteOutcome` event to arrive first.
//!
//! Nothing in this workspace calls [`LinkHealth::record_outcome`] with
//! a real observation yet — that wiring needs an actual send attempt
//! to time, which is real transport-touching work belonging to
//! whichever crate owns the connection (`siar-connectivity`, per
//! `MIGRATION.md` step 5), not this infra-free crate. This type is the
//! computation a future caller needs, built now so that wiring doesn't
//! also have to invent "how do I turn a pile of send attempts into a
//! reliability number" from scratch later.

use crate::metrics::{PathMetrics, Ratio};
use crate::types::RouteHealth;
use std::collections::VecDeque;

/// One measured send attempt. A failed attempt has no meaningful RTT —
/// `rtt_millis` is only ever read for attempts where `success` is
/// `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOutcome {
    pub success: bool,
    pub rtt_millis: Option<u32>,
}

impl SendOutcome {
    pub fn success(rtt_millis: u32) -> Self {
        Self {
            success: true,
            rtt_millis: Some(rtt_millis),
        }
    }

    pub fn failure() -> Self {
        Self {
            success: false,
            rtt_millis: None,
        }
    }
}

/// A bounded rolling window of [`SendOutcome`]s for one path — never
/// grows unbounded no matter how many times a long-lived path is sent
/// over. Older observations are dropped first, so `reliability`/
/// `average_rtt_millis` always reflect *recent* behavior — a stale
/// signal is worse than no signal on a mesh topology that changes
/// quickly.
pub struct LinkHealth {
    window: VecDeque<SendOutcome>,
    capacity: usize,
}

impl LinkHealth {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity >= 1,
            "a zero-capacity window could never track anything"
        );
        Self {
            window: VecDeque::new(),
            capacity,
        }
    }

    pub fn record_outcome(&mut self, outcome: SendOutcome) {
        if self.window.len() >= self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(outcome);
    }

    /// Fraction of recorded attempts that succeeded, `0.0..=1.0`.
    /// `1.0` — not `0.0` or some other "unknown" stand-in — when
    /// nothing has been recorded yet: an unmeasured path defaults to
    /// "assume it works" rather than reading as artificially
    /// unreliable before any real data exists.
    pub fn reliability(&self) -> f32 {
        if self.window.is_empty() {
            return 1.0;
        }
        let successes = self.window.iter().filter(|o| o.success).count();
        successes as f32 / self.window.len() as f32
    }

    /// Mean RTT across successful attempts with a recorded RTT —
    /// `None` if there are none yet, matching
    /// [`PathMetrics::rtt_millis`]'s own `Option<u32>` shape rather
    /// than inventing a fake zero.
    pub fn average_rtt_millis(&self) -> Option<u32> {
        let samples: Vec<u64> = self
            .window
            .iter()
            .filter(|o| o.success)
            .filter_map(|o| o.rtt_millis)
            .map(u32::into)
            .collect();
        if samples.is_empty() {
            return None;
        }
        let sum: u64 = samples.iter().sum();
        Some((sum / samples.len() as u64) as u32)
    }

    /// How many observations this window currently holds — lets a
    /// caller distinguish "genuinely 100% reliable" from "no data yet,
    /// defaulting to 100%" if that distinction ever matters to it, and
    /// lets [`Self::apply_to`] decide whether to touch `packet_loss`
    /// at all.
    pub fn observation_count(&self) -> usize {
        self.window.len()
    }

    /// Folds this window's own reliability/RTT onto an existing
    /// [`PathMetrics`] in place — `rtt_millis` direct from
    /// [`Self::average_rtt_millis`], and `reliability` mapped onto
    /// [`PathMetrics::packet_loss`] as its complement (`1.0 -
    /// reliability`): `packet_loss` is this crate's own existing,
    /// documented home for "how often does a send on this path fail,"
    /// and reusing it here means a scorer reading `packet_loss`
    /// doesn't need to also know to check some second, LinkHealth-only
    /// field. Deliberately does *not* touch `packet_loss` at all while
    /// [`Self::observation_count`] is zero — leaving it exactly as the
    /// caller already had it (typically `None`, from
    /// [`PathMetrics::unknown`]) rather than overwriting real prior
    /// data with an unearned `0.0`.
    pub fn apply_to(&self, metrics: &mut PathMetrics) {
        metrics.rtt_millis = self.average_rtt_millis();
        if self.observation_count() > 0 {
            metrics.packet_loss = Some(Ratio::new((1.0 - self.reliability()) as f64));
        }
    }
}

/// Turns this window's own reliability fraction into a qualitative
/// [`RouteHealth`] — the same four-way split
/// [`crate::engine::health_after_outcome`] already uses for its
/// `Success`/`Timeout` transitions, so a caller building an initial
/// [`crate::candidate::PathCandidate::health`] from real measurements
/// (rather than waiting for the first `RouteOutcome` event) lands on
/// values downstream scoring already knows how to interpret.
/// Thresholds are this module's own reasoned starting point — not
/// transcribed from any spec text, which doesn't name concrete cutoffs
/// for this mapping — chosen so a path needs to be *mostly* failing
/// before it reads as `Unreachable`, matching next.md §92's own
/// "stale/uncertain shouldn't read as certainly broken" posture toward
/// noisy real-world measurements.
pub fn health_from_reliability(reliability: f32) -> RouteHealth {
    if reliability >= 0.8 {
        RouteHealth::Healthy
    } else if reliability >= 0.5 {
        RouteHealth::Degraded
    } else if reliability > 0.0 {
        RouteHealth::Suspect
    } else {
        RouteHealth::Unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmeasured_link_defaults_to_fully_reliable_not_zero() {
        let health = LinkHealth::new(4);
        assert_eq!(health.reliability(), 1.0);
        assert_eq!(health.average_rtt_millis(), None);
        assert_eq!(health.observation_count(), 0);
    }

    #[test]
    fn reliability_reflects_the_mix_of_recorded_outcomes() {
        let mut health = LinkHealth::new(4);
        health.record_outcome(SendOutcome::success(50));
        health.record_outcome(SendOutcome::success(60));
        health.record_outcome(SendOutcome::failure());
        health.record_outcome(SendOutcome::failure());
        assert_eq!(health.reliability(), 0.5);
    }

    #[test]
    fn average_rtt_only_counts_successful_attempts() {
        let mut health = LinkHealth::new(4);
        health.record_outcome(SendOutcome::success(100));
        health.record_outcome(SendOutcome::success(200));
        health.record_outcome(SendOutcome::failure());
        assert_eq!(health.average_rtt_millis(), Some(150));
    }

    #[test]
    fn window_drops_oldest_observation_once_at_capacity() {
        let mut health = LinkHealth::new(2);
        health.record_outcome(SendOutcome::failure());
        health.record_outcome(SendOutcome::failure());
        assert_eq!(health.reliability(), 0.0);

        health.record_outcome(SendOutcome::success(10));
        health.record_outcome(SendOutcome::success(10));
        assert_eq!(health.reliability(), 1.0);
        assert_eq!(health.observation_count(), 2);
    }

    #[test]
    #[should_panic(expected = "zero-capacity")]
    fn zero_capacity_window_is_rejected_up_front() {
        LinkHealth::new(0);
    }

    #[test]
    fn apply_to_leaves_packet_loss_untouched_with_no_observations() {
        let health = LinkHealth::new(4);
        let mut metrics = PathMetrics::unknown();
        health.apply_to(&mut metrics);
        assert_eq!(metrics.rtt_millis, None);
        assert_eq!(metrics.packet_loss, None);
    }

    #[test]
    fn apply_to_sets_rtt_and_packet_loss_as_complements_of_reliability() {
        let mut health = LinkHealth::new(4);
        health.record_outcome(SendOutcome::success(40));
        health.record_outcome(SendOutcome::success(60));
        health.record_outcome(SendOutcome::failure());
        health.record_outcome(SendOutcome::failure());
        let mut metrics = PathMetrics::unknown();
        health.apply_to(&mut metrics);
        assert_eq!(metrics.rtt_millis, Some(50));
        assert_eq!(metrics.packet_loss, Some(Ratio::new(0.5)));
    }

    #[test]
    fn apply_to_does_not_touch_unrelated_fields() {
        let health = LinkHealth::new(4);
        let mut metrics = PathMetrics::unknown();
        metrics.stability = crate::metrics::StabilityScore::VeryStable;
        health.apply_to(&mut metrics);
        assert_eq!(
            metrics.stability,
            crate::metrics::StabilityScore::VeryStable
        );
    }

    #[test]
    fn health_from_reliability_matches_documented_thresholds() {
        assert_eq!(health_from_reliability(1.0), RouteHealth::Healthy);
        assert_eq!(health_from_reliability(0.8), RouteHealth::Healthy);
        assert_eq!(health_from_reliability(0.79), RouteHealth::Degraded);
        assert_eq!(health_from_reliability(0.5), RouteHealth::Degraded);
        assert_eq!(health_from_reliability(0.49), RouteHealth::Suspect);
        assert_eq!(health_from_reliability(0.0), RouteHealth::Unreachable);
    }
}
