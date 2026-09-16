//! §24 "Route Scoring", §25 "Hard Constraints vs Soft Preferences", §26
//! "Path Scoring Interface".

use serde::{Deserialize, Serialize};

use crate::candidate::PathCandidate;
use crate::metrics::{CongestionState, EnergyCost, NetworkCost, StabilityScore};
use crate::platform::DeviceState;
use crate::policy::PolicyWeights;
use crate::requirements::DeliveryRequirements;
use crate::setup::{effective_setup_cost, SetupCost};
use crate::types::{DeliveryClass, MeteredState, PathId, RoamingState, RouteHealth, TransportKind};

/// Higher is better. `f64`, not an integer — §155 "Integer Score
/// Option" names an integer score as an *alternative* worth
/// considering, not the required choice, and a weighted sum of several
/// `[0.0, 1.0]`-ish terms (this module's own `score` function) is far
/// more natural to compose as floats than to keep rescaling into
/// integers at every step.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct RouteScore(pub f64);

impl RouteScore {
    /// §155 "Integer Score Option": "for deterministic portability,
    /// use fixed-point/integer score... 0..10_000... avoid floating
    /// point if reproducibility matters." Named as an *option* in the
    /// spec's own section title, not a replacement — this crate's
    /// existing `f64` scoring already reproduces exactly given
    /// identical inputs (§123, proved by property test), so this
    /// isn't fixing a real non-determinism; it's a second
    /// representation for a caller that specifically needs a stable
    /// integer for serialization, cross-language comparison, or
    /// display (§157's own worked example, "Score: 8240", is exactly
    /// this range).
    ///
    /// Normalizes against `weights`'s own maximum possible sum (per
    /// [`crate::policy::PolicyWeights::sum`]) rather than an assumed
    /// constant — that struct's own doc comment is explicit that
    /// weights aren't required to sum to 1.0, so a fixed denominator
    /// would silently misscale any profile whose weights don't happen
    /// to match it. A `weights` with all-zero terms (nothing to
    /// compare against) returns `0` rather than dividing by zero.
    pub fn as_fixed_point(&self, weights: &crate::policy::PolicyWeights) -> u16 {
        let max = weights.sum();
        if max <= 0.0 {
            return 0;
        }
        let normalized = (self.0 / max).clamp(0.0, 1.0);
        (normalized * 10_000.0).round() as u16
    }
}

/// §35's `switch_threshold` field type. `Serialize`/`Deserialize`
/// added for §179 "Route Policy Persistence," since this is a field
/// of [`crate::policy::HysteresisPolicy`], itself a persistable
/// setting — see [`crate::privacy::PrivacyPolicy`]'s own doc comment
/// for the fuller reasoning.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RouteScoreDelta(pub f64);

/// §26's third parameter. Just enough state for stickiness/hysteresis
/// (§34-35) and existing-connection preference (§45) to work — this
/// crate has no live connection pool of its own to introspect (see its
/// top doc comment on scope), so `current_path` is supplied by
/// whatever caller does have one. `device` (§85, this round) is the
/// same idea applied to battery/thermal/foreground state — optional,
/// and `None` changes nothing (see [`crate::discovery::discovery_permitted`]
/// for the one place it's actually read).
#[derive(Debug, Clone, Copy, Default)]
pub struct RoutingContext {
    pub current_path: Option<PathId>,
    pub device: Option<DeviceState>,
}

/// §26.
pub trait PathScorer {
    fn score(
        &self,
        candidate: &PathCandidate,
        req: &DeliveryRequirements,
        context: &RoutingContext,
    ) -> RouteScore;
}

/// §82 "Metered Networks": "Do not use one global allow/deny." —
/// `req.allow_metered` is already that per-traffic-type policy (each
/// [`DeliveryRequirements`] constructor sets it appropriately: see
/// [`DeliveryRequirements::file_chunk`]'s own doc comment for §82's
/// own "block bulk" example, corrected this round to actually do
/// that). What this function adds is the third state: a candidate
/// whose metered-ness the platform hasn't reported yet
/// ([`MeteredState::Unknown`]) is treated the same as
/// [`MeteredState::Metered`] — the conservative direction for a
/// signal that exists specifically to protect a user's data plan.
/// [`MeteredState::Unmetered`] is the only state that bypasses
/// `allow_metered` entirely, since there's nothing to protect against.
fn metered_state_permits(state: MeteredState, allow_metered: bool) -> bool {
    match state {
        MeteredState::Unmetered => true,
        MeteredState::Metered | MeteredState::Unknown => allow_metered,
    }
}

/// §83 "Roaming": "A user may allow cellular but forbid roaming bulk
/// transfer" — read literally: the restriction is specifically
/// roaming *and* bulk together, not roaming in general (an
/// interactive message while roaming is fine; a large file chunk
/// while roaming is what §83 actually names as the thing to block).
/// [`RoamingState::Unknown`] is treated like [`RoamingState::Roaming`]
/// for the same conservative-default reasoning as
/// [`metered_state_permits`]'s own `Unknown` case.
fn permits_roaming_bulk(state: RoamingState, req: &DeliveryRequirements) -> bool {
    if req.class != DeliveryClass::Bulk {
        return true;
    }
    match state {
        RoamingState::NotRoaming => true,
        RoamingState::Roaming | RoamingState::Unknown => req.allow_roaming_bulk,
    }
}

/// §25 step 1, applied to one candidate. Real checks against fields
/// this crate actually has — not every hard constraint the spec
/// gestures at is checkable without live transport/policy state this
/// crate doesn't hold (e.g. §105 "Path Authorization"'s
/// device-trusted/operation-authorized checks belong in
/// [`crate::resolve`], upstream of this function, not duplicated here).
pub fn passes_hard_constraints(candidate: &PathCandidate, req: &DeliveryRequirements) -> bool {
    if candidate.health == RouteHealth::Unreachable {
        return false;
    }
    if req.class == DeliveryClass::Realtime && !candidate.capabilities.realtime_media {
        return false;
    }
    if !req.allow_relay && candidate.transport == TransportKind::IrohRelay {
        return false;
    }
    if !req.allow_bluetooth
        && matches!(
            candidate.transport,
            TransportKind::BluetoothClassic | TransportKind::BluetoothLe
        )
    {
        return false;
    }
    if !req.allow_dtn && candidate.transport == TransportKind::Dtn {
        return false;
    }
    if !metered_state_permits(candidate.capabilities.metered, req.allow_metered) {
        return false;
    }
    if !permits_roaming_bulk(candidate.capabilities.roaming, req) {
        return false;
    }
    // A missing bandwidth estimate does NOT eliminate the candidate —
    // §13's own rule ("missing metrics must be represented explicitly")
    // means "unknown" is a distinct state from "known to be
    // insufficient," and only the latter is a hard-constraint failure.
    if let (Some(min_bw), Some(estimated)) =
        (req.min_bandwidth, candidate.metrics.estimated_bandwidth)
    {
        if estimated < min_bw {
            return false;
        }
    }
    true
}

/// §25 step 1, applied to a whole candidate set.
pub fn eliminate_hard_constraint_violations<'a>(
    candidates: &'a [PathCandidate],
    req: &DeliveryRequirements,
) -> Vec<&'a PathCandidate> {
    candidates
        .iter()
        .filter(|c| passes_hard_constraints(c, req))
        .collect()
}

fn unit_interval(low_is_bad_high_is_good: f64) -> f64 {
    low_is_bad_high_is_good.clamp(0.0, 1.0)
}

fn stability_unit(s: StabilityScore) -> f64 {
    match s {
        StabilityScore::VeryUnstable => 0.0,
        StabilityScore::Unstable => 0.25,
        StabilityScore::Moderate => 0.5,
        StabilityScore::Stable => 0.75,
        StabilityScore::VeryStable => 1.0,
    }
}

fn energy_unit(e: EnergyCost) -> f64 {
    // Inverted — Free is the *best* outcome for a "suitability" score.
    match e {
        EnergyCost::Free => 1.0,
        EnergyCost::Low => 0.75,
        EnergyCost::Moderate => 0.5,
        EnergyCost::High => 0.25,
        EnergyCost::VeryHigh => 0.0,
    }
}

fn cost_unit(c: NetworkCost) -> f64 {
    match c {
        NetworkCost::Free => 1.0,
        NetworkCost::Low => 0.75,
        NetworkCost::Moderate => 0.5,
        NetworkCost::High => 0.25,
        NetworkCost::VeryHigh => 0.0,
    }
}

/// §44: higher is better, so a *cheap* setup gets the high end.
fn setup_cost_unit(c: SetupCost) -> f64 {
    match c {
        SetupCost::Cheap => 1.0,
        SetupCost::Moderate => 0.5,
        SetupCost::Expensive => 0.0,
    }
}

fn reachability_unit(h: RouteHealth) -> f64 {
    match h {
        RouteHealth::Healthy => 1.0,
        RouteHealth::Degraded => 0.6,
        RouteHealth::Suspect => 0.3,
        RouteHealth::Unknown => 0.4,
        RouteHealth::Unreachable => 0.0, // already eliminated by passes_hard_constraints; kept exhaustive rather than unreachable!()
    }
}

/// The default, policy-weighted implementation of §24's conceptual
/// formula. Deterministic (§123 "Deterministic Scoring" — no RNG, no
/// wall-clock reads inside this function itself) and real, but a
/// genuinely partial reading of §24's formula: `failure_penalty`
/// beyond what [`RouteHealth`] already folds into `reachability` is
/// treated as zero rather than modeled — it would need failure
/// *history* this crate keeps no record of, named explicitly here
/// rather than silently dropped. `congestion` (§79) is no longer in
/// that list as of this round — see `congestion_suitability` in
/// [`Self::score`]'s own body.
pub struct DefaultScorer {
    pub weights: PolicyWeights,
}

impl PathScorer for DefaultScorer {
    fn score(
        &self,
        candidate: &PathCandidate,
        req: &DeliveryRequirements,
        context: &RoutingContext,
    ) -> RouteScore {
        let w = &self.weights;
        let m = &candidate.metrics;

        let reachability = reachability_unit(candidate.health);

        let latency_suitability = match (req.max_latency_millis, m.rtt_millis) {
            (Some(max), Some(rtt)) => {
                unit_interval(1.0 - (rtt as f64 / max as f64 - 1.0).max(0.0) / 2.0)
            }
            _ => 0.5, // unknown or unconstrained — neutral, not penalized (§13)
        };

        let bandwidth_suitability = match (req.min_bandwidth, m.estimated_bandwidth) {
            (Some(min), Some(est)) => unit_interval(est.0 as f64 / min.0.max(1) as f64).min(1.0),
            _ => 0.5,
        };

        let stability = stability_unit(m.stability);
        let energy_suitability = energy_unit(m.energy_cost);
        let cost_suitability = cost_unit(m.monetary_cost);
        // §40 "Path Memory": a hint, not proof — a present
        // `last_success_millis` counts for something, but this
        // function has no clock of its own to judge *how* recent, so
        // it's binary (present/absent) rather than decayed by age.
        let recent_success = if m.last_success_millis.is_some() {
            1.0
        } else {
            0.5
        };

        // §44: fold pool state into a setup-cost suitability term —        // an Active pooled connection scores like a cheap transport
        // regardless of what that transport statically costs.
        let setup_cost_suitability =
            setup_cost_unit(effective_setup_cost(candidate.transport, m.pool_state));

        // §45 "Existing Connection Preference": this candidate *is*
        // the context's currently-pinned path, on top of whatever
        // §34/§35 stickiness in [`crate::plan`] already does with the
        // same `current_path` field — stickiness protects an already-
        // chosen primary from being unseated by a marginal gain
        // elsewhere; this term additionally lets scoring itself notice
        // "this one's already connected" the very first time several
        // fresh candidates are compared, before any primary has been
        // pinned at all.
        let existing_connection = if context.current_path == Some(candidate.path_id) {
            1.0
        } else {
            0.5
        };

        // §79 "Congestion Signals": higher is better, so "Normal"
        // scores highest. When `congestion_state` itself is unset,
        // fall back to reading `retransmission_rate` directly — a
        // rising retransmission rate is one of the concrete signals
        // §79 names *for* deriving a congestion state, so a caller
        // that reports the rate but hasn't (yet) classified it into a
        // named state shouldn't be treated identically to one with no
        // signal at all. Both absent falls back to neutral 0.5, same
        // as every other unknown-metric case in this function.
        let congestion_suitability = match (m.congestion_state, m.retransmission_rate) {
            (Some(CongestionState::Normal), _) => 1.0,
            (Some(CongestionState::Congested), _) => 0.4,
            (Some(CongestionState::Severe), _) => 0.0,
            (None, Some(rate)) => unit_interval(1.0 - rate.get()),
            (None, None) => 0.5,
        };

        let total = w.reachability * reachability
            + w.latency * latency_suitability
            + w.bandwidth * bandwidth_suitability
            + w.stability * stability
            + w.energy * energy_suitability
            + w.cost * cost_suitability
            + w.recent_success * recent_success
            + w.setup_cost * setup_cost_suitability
            + w.existing_connection * existing_connection
            + w.congestion * congestion_suitability
            + w.candidate_state * crate::acquisition::candidate_state_unit(candidate.state);

        RouteScore(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::metrics::PathMetrics;
    use crate::policy::RoutingPolicyProfile;
    use crate::types::PathCapabilities;
    use siar_domain::DeviceId;

    fn candidate(transport: TransportKind, health: RouteHealth, realtime: bool) -> PathCandidate {
        PathCandidate {
            path_id: PathId::new(),
            transport,
            peer: DeviceId::new(),
            endpoint: TransportEndpoint(vec![]),
            metrics: PathMetrics::unknown(),
            capabilities: PathCapabilities {
                reliable_stream: true,
                datagram: true,
                large_files: true,
                realtime_media: realtime,
                peer_discovery: true,
                store_and_forward: false,
                metered: crate::types::MeteredState::Unknown,
                roaming: crate::types::RoamingState::Unknown,
                requires_foreground: false,
            },
            health,
            underlay: None,
            state: crate::acquisition::CandidateState::Active,
        }
    }

    #[test]
    fn realtime_media_requires_a_realtime_capable_path() {
        let req = DeliveryRequirements::realtime_media();
        let capable = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, true);
        let incapable = candidate(TransportKind::BluetoothLe, RouteHealth::Healthy, false);
        assert!(passes_hard_constraints(&capable, &req));
        assert!(!passes_hard_constraints(&incapable, &req));
    }

    #[test]
    fn disallowed_relay_is_eliminated_even_when_otherwise_healthy() {
        let mut req = DeliveryRequirements::interactive_message();
        req.allow_relay = false;
        let relay = candidate(TransportKind::IrohRelay, RouteHealth::Healthy, false);
        assert!(!passes_hard_constraints(&relay, &req));
    }

    #[test]
    fn unreachable_health_is_always_a_hard_elimination() {
        let req = DeliveryRequirements::interactive_message();
        let dead = candidate(TransportKind::IrohDirect, RouteHealth::Unreachable, false);
        assert!(!passes_hard_constraints(&dead, &req));
    }

    #[test]
    fn a_missing_bandwidth_estimate_does_not_eliminate_a_candidate() {
        let mut req = DeliveryRequirements::interactive_message();
        req.min_bandwidth = Some(crate::metrics::Bitrate(1_000_000));
        let unknown_bw = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        assert!(passes_hard_constraints(&unknown_bw, &req)); // metrics.estimated_bandwidth is None from ::unknown()
    }

    /// §185 "Property Tests": "hard minimum bandwidth respected." The
    /// test just above this one proves the opposite direction
    /// (*unknown* bandwidth isn't penalized); this proves a *known*,
    /// insufficient bandwidth genuinely is a hard elimination, not
    /// merely a lower score.
    #[test]
    fn spec_185_a_known_bandwidth_below_the_required_minimum_is_hard_eliminated() {
        let mut req = DeliveryRequirements::interactive_message();
        req.min_bandwidth = Some(crate::metrics::Bitrate(5_000_000));
        let mut too_slow = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        too_slow.metrics.estimated_bandwidth = Some(crate::metrics::Bitrate(1_000_000));
        assert!(!passes_hard_constraints(&too_slow, &req));
    }

    #[test]
    fn an_explicitly_metered_candidate_is_eliminated_when_the_operation_disallows_it() {
        let mut req = DeliveryRequirements::interactive_message();
        req.allow_metered = false;
        let mut metered = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        metered.capabilities.metered = MeteredState::Metered;
        assert!(!passes_hard_constraints(&metered, &req));
    }

    #[test]
    fn an_unconfirmed_metered_state_is_treated_as_metered_by_default() {
        // §82: "unknown" is a real third state, not a free pass — the
        // conservative default protects a user's data plan when the
        // platform simply hasn't reported yet.
        let mut req = DeliveryRequirements::interactive_message();
        req.allow_metered = false;
        let mut unknown_metered = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        unknown_metered.capabilities.metered = MeteredState::Unknown;
        assert!(!passes_hard_constraints(&unknown_metered, &req));
    }

    #[test]
    fn a_confirmed_unmetered_candidate_always_passes_regardless_of_the_flag() {
        let mut req = DeliveryRequirements::interactive_message();
        req.allow_metered = false;
        let mut unmetered = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        unmetered.capabilities.metered = MeteredState::Unmetered;
        assert!(passes_hard_constraints(&unmetered, &req));
    }

    #[test]
    fn roaming_bulk_transfer_is_blocked_by_default() {
        // §83's own worked example, verbatim: "allow cellular but
        // forbid roaming bulk transfer."
        let mut req = DeliveryRequirements::file_chunk(); // Bulk, allow_roaming_bulk: false
        req.allow_metered = true; // isolate the roaming check from the metered one
        let mut roaming = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        roaming.capabilities.metered = MeteredState::Unmetered;
        roaming.capabilities.roaming = RoamingState::Roaming;
        assert!(!passes_hard_constraints(&roaming, &req));
    }

    #[test]
    fn roaming_is_fine_for_non_bulk_traffic() {
        // §83's restriction names "roaming bulk transfer" specifically
        // — an ordinary interactive message while roaming is not what
        // it's about.
        let mut req = DeliveryRequirements::interactive_message(); // Interactive, not Bulk
        req.allow_metered = true;
        let mut roaming = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        roaming.capabilities.metered = MeteredState::Unmetered;
        roaming.capabilities.roaming = RoamingState::Roaming;
        assert!(passes_hard_constraints(&roaming, &req));
    }

    #[test]
    fn a_requirement_that_explicitly_allows_roaming_bulk_permits_it() {
        let mut req = DeliveryRequirements::file_chunk();
        req.allow_metered = true;
        req.allow_roaming_bulk = true; // explicit override
        let mut roaming = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        roaming.capabilities.metered = MeteredState::Unmetered;
        roaming.capabilities.roaming = RoamingState::Roaming;
        assert!(passes_hard_constraints(&roaming, &req));
    }

    #[test]
    fn a_healthier_candidate_scores_higher_all_else_equal() {
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let context = RoutingContext::default();

        let healthy = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        let degraded = candidate(TransportKind::IrohDirect, RouteHealth::Degraded, false);

        let healthy_score = scorer.score(&healthy, &req, &context);
        let degraded_score = scorer.score(&degraded, &req, &context);
        assert!(healthy_score.0 > degraded_score.0);
    }

    #[test]
    fn a_severely_congested_candidate_scores_lower_than_a_normal_one() {
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let context = RoutingContext::default();

        let mut normal = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        normal.metrics.congestion_state = Some(CongestionState::Normal);
        let mut severe = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        severe.metrics.congestion_state = Some(CongestionState::Severe);

        let normal_score = scorer.score(&normal, &req, &context);
        let severe_score = scorer.score(&severe, &req, &context);
        assert!(normal_score.0 > severe_score.0);
    }

    #[test]
    fn a_rising_retransmission_rate_lowers_the_score_even_without_a_named_congestion_state() {
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let context = RoutingContext::default();

        let mut low_retransmission =
            candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        low_retransmission.metrics.retransmission_rate = Some(crate::metrics::Ratio::new(0.01));
        let mut high_retransmission =
            candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        high_retransmission.metrics.retransmission_rate = Some(crate::metrics::Ratio::new(0.5));

        let low_score = scorer.score(&low_retransmission, &req, &context);
        let high_score = scorer.score(&high_retransmission, &req, &context);
        assert!(low_score.0 > high_score.0);
    }

    /// §123 "Deterministic Scoring": "given the same policy, metrics,
    /// and context, the routing decision should be reproducible."
    /// `DefaultScorer::score` is an ordinary pure function — no
    /// clock, no RNG, no shared mutable state — so this is really a
    /// regression guard against a future change accidentally
    /// introducing one of those, not a property in doubt today.
    #[test]
    fn spec_123_scoring_the_same_candidate_twice_produces_the_same_score() {
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let context = RoutingContext::default();
        let mut candidate = candidate(TransportKind::IrohDirect, RouteHealth::Healthy, false);
        candidate.metrics.rtt_millis = Some(37);
        candidate.metrics.congestion_state = Some(CongestionState::Congested);

        let first = scorer.score(&candidate, &req, &context);
        let second = scorer.score(&candidate, &req, &context);
        assert_eq!(first, second);
    }

    /// §155 "Integer Score Option": worked-example proof that a
    /// maximal score lands at the top of `0..10_000`, not merely
    /// "some plausible number."
    #[test]
    fn spec_155_a_maximal_score_lands_at_the_top_of_the_fixed_point_range() {
        let weights = crate::policy::PolicyWeights {
            reachability: 1.0,
            latency: 1.0,
            bandwidth: 1.0,
            stability: 1.0,
            energy: 1.0,
            cost: 1.0,
            recent_success: 1.0,
            setup_cost: 1.0,
            existing_connection: 1.0,
            congestion: 1.0,
            candidate_state: 1.0,
        };
        let max_possible = RouteScore(weights.sum());
        assert_eq!(max_possible.as_fixed_point(&weights), 10_000);
    }

    #[test]
    fn spec_155_a_zero_score_lands_at_the_bottom_of_the_fixed_point_range() {
        let weights = RoutingPolicyProfile::Balanced.policy().weights;
        assert_eq!(RouteScore(0.0).as_fixed_point(&weights), 0);
    }

    #[test]
    fn as_fixed_point_never_divides_by_zero_for_an_all_zero_weight_set() {
        let weights = crate::policy::PolicyWeights {
            reachability: 0.0,
            latency: 0.0,
            bandwidth: 0.0,
            stability: 0.0,
            energy: 0.0,
            cost: 0.0,
            recent_success: 0.0,
            setup_cost: 0.0,
            existing_connection: 0.0,
            congestion: 0.0,
            candidate_state: 0.0,
        };
        assert_eq!(RouteScore(5.0).as_fixed_point(&weights), 0);
    }
}
