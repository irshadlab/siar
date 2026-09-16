//! §27 "Policy Profiles" through §35 "Path Hysteresis".

use serde::{Deserialize, Serialize};

use crate::scoring::RouteScoreDelta;

/// The tunable inputs to [`crate::scoring::DefaultScorer`] — §24's
/// formula terms this crate actually models (see that module's own doc
/// comment for the one it still doesn't: failure penalty — congestion
/// was closed this round, §79).
/// Not required to sum to 1.0 — [`crate::scoring::RouteScore`] is a
/// relative ranking value, not a probability, so un-normalized weights
/// are fine as long as they're consistent within one comparison.
///
/// `setup_cost` and `existing_connection` are new this round, closing
/// two gaps this doc comment used to leave unnamed: §44 "Route scoring
/// should include setup latency/cost" had no term at all before now
/// (see [`crate::setup`]); `existing_connection` makes §45's
/// preference a real scored term rather than leaving it to §34
/// stickiness alone to express — stickiness only helps a path that's
/// *already primary*, but §45 applies just as much when scoring a
/// brand-new plan with no current path pinned yet (e.g. the very first
/// route to a destination, where several candidates happen to already
/// have pooled connections from other traffic).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolicyWeights {
    pub reachability: f64,
    pub latency: f64,
    pub bandwidth: f64,
    pub stability: f64,
    pub energy: f64,
    pub cost: f64,
    pub recent_success: f64,
    pub setup_cost: f64,
    pub existing_connection: f64,
    /// §79 "Congestion Signals" — closes the gap this struct's own doc
    /// comment has named since Phase 1 ("the two it doesn't: congestion,
    /// failure penalty"). Only "congestion" is closed this round;
    /// failure penalty still needs failure history this crate doesn't
    /// keep.
    pub congestion: f64,
    /// §89 "Passive vs Active Candidates": "Scoring can penalize
    /// setup" — a distinct axis from `setup_cost` (§44's cost *of*
    /// establishing a connection once you've decided to) since this
    /// is about whether the candidate is already reachable at all
    /// (see [`crate::acquisition::CandidateState`]).
    pub candidate_state: f64,
}

impl PolicyWeights {
    /// §155 "Integer Score Option" needs a denominator to normalize
    /// against — this struct's own doc comment already establishes
    /// that weights aren't required to sum to 1.0, so
    /// [`crate::scoring::RouteScore::as_fixed_point`] has to compute
    /// the actual maximum a given weight configuration could produce
    /// rather than assuming one.
    pub fn sum(&self) -> f64 {
        self.reachability
            + self.latency
            + self.bandwidth
            + self.stability
            + self.energy
            + self.cost
            + self.recent_success
            + self.setup_cost
            + self.existing_connection
            + self.congestion
            + self.candidate_state
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HysteresisPolicy {
    pub switch_threshold: RouteScoreDelta,
    pub minimum_hold_millis: u64,
    pub degraded_override: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct RoutingPolicy {
    pub weights: PolicyWeights,
    pub hysteresis: HysteresisPolicy,
}

/// §27's seven named defaults. `BulkTransfer` is listed in §27's own
/// enumeration but, unlike the other six, has no dedicated numbered
/// section spelling out its rules — its preset below is inferred from
/// §7's own `file transfer → Bulk/Reliable` example and §31's
/// "Low-Cost Policy" (large-file guidance), not transcribed from spec
/// text the way the other six are, and is flagged here as such rather
/// than presented with the same confidence. `Serialize`/`Deserialize`
/// added for §179 "Route Policy Persistence" — the named profile a
/// user picked is exactly the kind of setting that section means to
/// persist, as distinct from `PolicyWeights`'s own raw floats, which
/// §154's own "do not expose floating-point tuning directly to
/// ordinary users" is why this enum exists as the persisted form in
/// the first place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingPolicyProfile {
    Balanced,
    LowLatency,
    LowPower,
    LowCost,
    HighReliability,
    Emergency,
    BulkTransfer,
}

impl RoutingPolicyProfile {
    pub fn policy(self) -> RoutingPolicy {
        match self {
            // §28: "prefer direct, prefer unmetered, avoid high battery
            // cost, use relay if needed" — a moderate, roughly even
            // weighting with a mild energy lean.
            Self::Balanced => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.0,
                    latency: 0.6,
                    bandwidth: 0.5,
                    stability: 0.7,
                    energy: 0.6,
                    cost: 0.4,
                    recent_success: 0.5,
                    setup_cost: 0.5,
                    existing_connection: 0.5,
                    congestion: 0.5,
                    candidate_state: 0.5,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.3),
                    minimum_hold_millis: 10_000,
                    degraded_override: true,
                },
            },
            // §29: "minimize RTT, penalize jitter/loss, avoid path
            // switching, avoid DTN, avoid BLE" — latency dominates;
            // energy/cost barely matter; hysteresis threshold raised
            // ("avoid path switching") since a call mid-flight
            // shouldn't churn paths for a marginal gain.
            Self::LowLatency => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.0,
                    latency: 1.5,
                    bandwidth: 0.8,
                    stability: 1.0,
                    energy: 0.1,
                    cost: 0.1,
                    recent_success: 0.4,
                    // Low, not zero — a call already accepted a
                    // one-time setup cost to start; mid-call it
                    // shouldn't dominate over jitter/loss the way it
                    // would for a one-off tiny message.
                    setup_cost: 0.3,
                    existing_connection: 0.4,
                    // Congestion is one of the most directly relevant
                    // signals to a live call of anything in this
                    // formula — the highest `congestion` weight of any
                    // profile.
                    congestion: 0.7,
                    // A call shouldn't wait on active discovery/setup
                    // — prefer already-reachable candidates strongly.
                    candidate_state: 0.6,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.6),
                    minimum_hold_millis: 30_000,
                    degraded_override: true,
                },
            },
            // §30: "prefer existing connection, avoid active discovery,
            // avoid Wi-Fi Direct setup" — energy dominates; latency and
            // bandwidth matter far less. `setup_cost`/`existing_connection`
            // are this profile's whole point per §30's own words, so
            // both get this policy's highest weights of any term.
            Self::LowPower => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.0,
                    latency: 0.2,
                    bandwidth: 0.2,
                    stability: 0.5,
                    energy: 1.5,
                    cost: 0.4,
                    recent_success: 0.8, // "prefer existing connection"
                    setup_cost: 1.3,
                    existing_connection: 1.2,
                    congestion: 0.3,
                    // "Avoid active discovery" is this profile's own
                    // §30 text — the highest `candidate_state` weight
                    // of any profile, matching its already-highest
                    // `existing_connection`.
                    candidate_state: 0.8,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.5),
                    minimum_hold_millis: 60_000,
                    degraded_override: true,
                },
            },
            // §31: "prefer LAN/Wi-Fi, delay bulk traffic" — monetary
            // cost dominates.
            Self::LowCost => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.0,
                    latency: 0.2,
                    bandwidth: 0.4,
                    stability: 0.5,
                    energy: 0.3,
                    cost: 1.5,
                    recent_success: 0.4,
                    // §51's "LAN direct... no Internet dependency" is
                    // this profile's territory too — a moderate lean
                    // toward already-cheap-to-reach paths.
                    setup_cost: 0.6,
                    existing_connection: 0.5,
                    congestion: 0.3,
                    candidate_state: 0.4,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.3),
                    minimum_hold_millis: 10_000,
                    degraded_override: true,
                },
            },
            // §32: "prefer proven paths, allow retry" — stability and
            // recent success dominate over raw latency/energy;
            // existing_connection joins them for the same reason (a
            // connection already proven to work IS a proven path).
            Self::HighReliability => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.2,
                    latency: 0.4,
                    bandwidth: 0.4,
                    stability: 1.3,
                    energy: 0.3,
                    cost: 0.3,
                    recent_success: 1.2,
                    setup_cost: 0.4,
                    existing_connection: 0.9,
                    // Congestion is a leading indicator of the exact
                    // kind of unreliability this profile most wants to
                    // avoid — its own highest weight of any profile.
                    congestion: 0.8,
                    candidate_state: 0.5,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.4),
                    minimum_hold_millis: 15_000,
                    degraded_override: true,
                },
            },
            // §33: "allow mesh, allow DTN, increase discovery, allow
            // redundancy... ignore some cost preferences... but should
            // still remain battery-aware" — reachability dominates
            // everything else; cost weight near zero (ignored, not
            // literally zero — "battery-aware" keeps a nonzero energy
            // term).
            Self::Emergency => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 2.0,
                    latency: 0.3,
                    bandwidth: 0.2,
                    stability: 0.6,
                    energy: 0.3,
                    cost: 0.05,
                    recent_success: 0.5,
                    // "increase discovery" means this profile should
                    // be willing to pay Wi-Fi Direct/Bluetooth pairing
                    // cost when reachability needs it — the lowest
                    // setup_cost weight of any profile, deliberately,
                    // matching §52's own "emergency policy" as one of
                    // its named exceptions to the normal threshold.
                    setup_cost: 0.1,
                    existing_connection: 0.3,
                    // Reachability matters far more than avoiding
                    // congestion when the traffic is an SOS — lowest
                    // weight of any profile, deliberately, matching
                    // this profile's already-lowest `setup_cost`.
                    congestion: 0.1,
                    // §90's own "SOS: aggressive discovery allowed" —
                    // willing to use a `RequiresDiscovery`/
                    // `RequiresSetup` candidate readily; lowest weight
                    // of any profile, deliberately.
                    candidate_state: 0.1,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.2), // switch readily — reachability matters more than stability here
                    minimum_hold_millis: 2_000,
                    degraded_override: true,
                },
            },
            // Inferred, not transcribed — see this enum's own doc
            // comment. Bandwidth and cost dominate; latency barely
            // matters for a background bulk transfer.
            Self::BulkTransfer => RoutingPolicy {
                weights: PolicyWeights {
                    reachability: 1.0,
                    latency: 0.1,
                    bandwidth: 1.3,
                    stability: 0.8,
                    energy: 0.5,
                    cost: 0.9,
                    recent_success: 0.5,
                    // A one-time transfer can amortize setup cost
                    // over a large payload — low weight, same
                    // reasoning as Emergency but for throughput
                    // instead of reachability.
                    setup_cost: 0.2,
                    existing_connection: 0.3,
                    // Sustained throughput is exactly what congestion
                    // erodes — a real, if not top, priority for a bulk
                    // transfer.
                    congestion: 0.6,
                    candidate_state: 0.3,
                },
                hysteresis: HysteresisPolicy {
                    switch_threshold: RouteScoreDelta(0.3),
                    minimum_hold_millis: 20_000,
                    degraded_override: true,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_latency_weighs_latency_more_than_low_power_does() {
        let low_latency = RoutingPolicyProfile::LowLatency.policy();
        let low_power = RoutingPolicyProfile::LowPower.policy();
        assert!(low_latency.weights.latency > low_power.weights.latency);
    }

    #[test]
    fn low_power_weighs_energy_more_than_low_latency_does() {
        let low_latency = RoutingPolicyProfile::LowLatency.policy();
        let low_power = RoutingPolicyProfile::LowPower.policy();
        assert!(low_power.weights.energy > low_latency.weights.energy);
    }

    #[test]
    fn emergency_weighs_reachability_above_every_other_profile() {
        let emergency = RoutingPolicyProfile::Emergency.policy();
        for other in [
            RoutingPolicyProfile::Balanced,
            RoutingPolicyProfile::LowLatency,
            RoutingPolicyProfile::LowPower,
            RoutingPolicyProfile::LowCost,
            RoutingPolicyProfile::HighReliability,
            RoutingPolicyProfile::BulkTransfer,
        ] {
            assert!(emergency.weights.reachability >= other.policy().weights.reachability);
        }
    }
}
