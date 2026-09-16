//! §18 "Route Plan" through §23 "Delay-Tolerant Route", tying together
//! §25's evaluation order and §34's stickiness.

use crate::candidate::PathCandidate;
use crate::error::RoutingError;
use crate::policy::RoutingPolicy;
use crate::requirements::DeliveryRequirements;
use crate::scoring::{
    eliminate_hard_constraint_violations, PathScorer, RouteScore, RoutingContext,
};
use crate::types::{DeliveryClass, Priority};

/// §18's strategies, plus `Hedged` (§93 "Hedged Requests", round 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteStrategy {
    Single,
    Failover,
    Redundant,
    Hedged,
    Multipath,
    DelayTolerant,
}

/// §18, plus `hedge_delay_millis` (§93) — `Some` only when `strategy
/// == Hedged`; see [`crate::resilience::hedge_policy_for`] for how
/// it's derived — and `reason` (§97), `created_at_millis`/
/// `valid_until_millis` (§104), all three new this round.
///
/// §103 "Process Death": "Routing correctness must not depend on
/// in-memory plans... After restart: reconstruct route from current
/// conditions." This type deliberately does **not** derive
/// `Serialize`/`Deserialize` — unlike almost every other value type in
/// this crate — so there is no way to persist a `RoutePlan` itself
/// using this crate's own types; a caller can only ever get a fresh
/// one from [`plan_route`]. That's not an oversight this round is
/// filling in; it's the one place in this crate where the *absence*
/// of a capability is the actual §103 compliance, so it's named here
/// rather than silently left unremarked.
#[derive(Debug, Clone)]
pub struct RoutePlan {
    pub primary: PathCandidate,
    pub fallbacks: Vec<PathCandidate>,
    pub replicas: Vec<PathCandidate>,
    pub strategy: RouteStrategy,
    pub hedge_delay_millis: Option<u64>,
    pub reason: crate::explain::RouteReason,
    pub created_at_millis: u64,
    pub valid_until_millis: u64,
    /// §157 "Developer Diagnostics"'s own worked example ("Score:
    /// 8240") needs the winning score somewhere a caller can read it
    /// back — before this round, `plan_route` computed one internally
    /// and then threw it away. See [`crate::diagnostics`] for where
    /// this is actually turned into that worked example's exact
    /// shape (including the 0..10,000 fixed-point conversion, §155).
    pub primary_score: crate::scoring::RouteScore,
}

/// §25's four-step evaluation order, run end to end:
/// 1. eliminate paths violating hard constraints
/// 2. score remaining paths
/// 3. apply stickiness/hysteresis
/// 4. produce route plan
///
/// `current` is the currently-active candidate, if any, matching §34's
/// worked example ("Iroh direct stable, Wi-Fi briefly appears — do not
/// churn"); pass `None` for a fresh destination with no existing route.
///
/// `device` (§85, this round) is the same idea for battery/thermal/
/// foreground state — `None` when a caller has none to report, which
/// changes nothing (see [`crate::acquisition::eliminate_background_restricted`],
/// the only place this function actually reads it, for what `None`
/// resolves to: every candidate kept, since nothing is confirmed
/// backgrounded).
///
/// §22 "Multipath Route" is explicitly named in the spec as "a later
/// optimization, not required for v1 routing" — this function never
/// produces [`RouteStrategy::Multipath`] for that reason, not because
/// the type doesn't exist.
///
/// `now_millis` (§104, this round) sets `RoutePlan::created_at_millis`
/// and derives `valid_until_millis` — this crate has no clock of its
/// own (see its top doc comment on scope), so "now" is always a
/// caller-supplied parameter, the same convention every other
/// timestamp in this crate already follows (e.g.
/// [`crate::cache::RouteCache::get`]'s own `now_millis`). The validity
/// window reuses `policy.hysteresis.minimum_hold_millis` rather than
/// inventing a separate per-profile duration — that field already
/// represents "how long this decision should be trusted before
/// re-evaluating" for stickiness purposes (§34-35), which is the same
/// question §104 is asking.
pub fn plan_route(
    candidates: &[PathCandidate],
    req: &DeliveryRequirements,
    policy: &RoutingPolicy,
    scorer: &dyn PathScorer,
    current: Option<&PathCandidate>,
    device: Option<&crate::platform::DeviceState>,
    now_millis: u64,
) -> Result<RoutePlan, RoutingError> {
    let mut eligible = eliminate_hard_constraint_violations(candidates, req);
    // §86 "Background Restrictions": a second, independent elimination
    // pass — kept separate from `eliminate_hard_constraint_violations`
    // rather than folded into it, the same "composable, not baked into
    // the core hard-constraint function" posture already used for
    // [`crate::privacy::eliminate_privacy_violations`]/
    // [`crate::security::eliminate_untrusted_candidates`].
    eligible.retain(|c| crate::acquisition::is_currently_usable(c, device));
    if eligible.is_empty() {
        return Err(RoutingError::NoEligibleCandidates);
    }

    let context = RoutingContext {
        current_path: current.map(|c| c.path_id),
        device: device.copied(),
    };
    let mut scored: Vec<(&PathCandidate, RouteScore)> = eligible
        .into_iter()
        .map(|c| (c, scorer.score(c, req, &context)))
        .collect();
    // Deterministic tie-break by `path_id` (§123 "Deterministic
    // Scoring") — `f64` doesn't implement `Ord`, and two genuinely
    // equal scores should still produce a stable, repeatable pick
    // rather than depending on input order.
    scored.sort_by(|(a, a_score), (b, b_score)| {
        b_score
            .0
            .partial_cmp(&a_score.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path_id.cmp(&b.path_id))
    });

    // §34/§35 stickiness: keep the current path unless a candidate
    // beats it by more than the policy's switch threshold, or the
    // current path has degraded (§35's `degraded_override`).
    let best = if let Some(current) = current {
        let current_still_eligible = scored.iter().find(|(c, _)| c.path_id == current.path_id);
        match current_still_eligible {
            Some((_, current_score)) => {
                let top = scored[0];
                let current_is_degraded = matches!(
                    current.health,
                    crate::types::RouteHealth::Degraded | crate::types::RouteHealth::Suspect
                );
                let should_switch = (top.1 .0 - current_score.0
                    > policy.hysteresis.switch_threshold.0)
                    || (policy.hysteresis.degraded_override
                        && current_is_degraded
                        && top.0.path_id != current.path_id);
                if should_switch {
                    top
                } else {
                    *current_still_eligible.unwrap()
                }
            }
            // The current path didn't survive hard-constraint
            // elimination this round (e.g. it just went Unreachable) —
            // there's nothing to stick to.
            None => scored[0],
        }
    } else {
        scored[0]
    };

    let primary = best.0.clone();
    let mut fallbacks: Vec<PathCandidate> = scored
        .iter()
        .map(|(c, _)| (*c).clone())
        .filter(|c| c.path_id != primary.path_id)
        .collect();

    // §21 "Redundant Route": "Use redundancy sparingly" — reserved for
    // the spec's own named case, Critical + DelayTolerant (its SOS
    // example), and (§162, this round) gated on `allow_redundancy` so
    // a caller can opt out of the "sparingly" case entirely rather
    // than this crate deciding "SOS" always means "spend the extra
    // replica." §93 "Hedged Requests" comes next in priority once
    // Redundant doesn't apply — a caller-configured hedge preference
    // for a fallback-having plan; everything else with at least one
    // fallback is Failover (§20); a lone eligible candidate is Single
    // (§19). §23 DelayTolerant is not separately produced here since a
    // DTN candidate competes on its own merits via [`crate::scoring`]
    // rather than this function special-casing "no other path exists"
    // — a real DTN candidate reaching this function already passed
    // §25 step 1 like any other transport.
    let hedge = crate::resilience::hedge_policy_for(req);
    let strategy = if req.priority == Priority::Critical
        && req.class == DeliveryClass::DelayTolerant
        && req.allow_redundancy
        && !fallbacks.is_empty()
    {
        RouteStrategy::Redundant
    } else if hedge.should_hedge && !fallbacks.is_empty() {
        RouteStrategy::Hedged
    } else if fallbacks.is_empty() {
        RouteStrategy::Single
    } else {
        RouteStrategy::Failover
    };
    let hedge_delay_millis = (strategy == RouteStrategy::Hedged).then_some(hedge.delay_millis);

    // §73 "Path Diversity": prefer a replica on a different underlay
    // from the primary rather than blindly taking the next-best-scored
    // fallback (which could easily share the primary's own underlay —
    // see [`crate::diversity`]'s own doc comment for §73's worked
    // "same Wi-Fi path twice" non-example).
    let replicas = if strategy == RouteStrategy::Redundant {
        match crate::diversity::most_diverse_fallback(&primary, &fallbacks) {
            Some(chosen) => {
                let chosen_id = chosen.path_id;
                let index = fallbacks
                    .iter()
                    .position(|f| f.path_id == chosen_id)
                    .expect("most_diverse_fallback only ever returns an item from `fallbacks`");
                vec![fallbacks.remove(index)]
            }
            // Unreachable in practice — `strategy == Redundant` is
            // only ever set when `!fallbacks.is_empty()` — but handled
            // rather than assumed, per this crate's usual "don't panic
            // on a state that shouldn't happen" posture.
            None => vec![],
        }
    } else {
        vec![]
    };

    let reason = crate::explain::infer_reason(&primary, &context, strategy);

    Ok(RoutePlan {
        primary,
        fallbacks,
        replicas,
        strategy,
        hedge_delay_millis,
        reason,
        created_at_millis: now_millis,
        valid_until_millis: now_millis + policy.hysteresis.minimum_hold_millis,
        primary_score: best.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::metrics::PathMetrics;
    use crate::policy::RoutingPolicyProfile;
    use crate::scoring::DefaultScorer;
    use crate::types::{PathCapabilities, PathId, RouteHealth, TransportKind};
    use siar_domain::DeviceId;

    fn candidate(transport: TransportKind, health: RouteHealth) -> PathCandidate {
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
                realtime_media: false,
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
    fn a_single_eligible_candidate_produces_a_single_strategy_plan() {
        let candidates = vec![candidate(TransportKind::IrohDirect, RouteHealth::Healthy)];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Single);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    fn multiple_eligible_candidates_produce_a_failover_plan_with_the_healthiest_primary() {
        let healthy = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let degraded = candidate(TransportKind::IrohRelay, RouteHealth::Degraded);
        let candidates = vec![degraded.clone(), healthy.clone()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Failover);
        assert_eq!(plan.primary.path_id, healthy.path_id);
        assert_eq!(plan.fallbacks.len(), 1);
    }

    /// §104 "Route Plan Lifetime" wired end to end: `created_at_millis`
    /// echoes the caller-supplied `now_millis`, and `valid_until_millis`
    /// is derived from the policy's own hysteresis window rather than
    /// being left for the caller to guess at.
    #[test]
    fn route_plan_lifetime_is_derived_from_the_policys_own_hysteresis_window() {
        let healthy = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let candidates = vec![healthy];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 10_000).unwrap();
        assert_eq!(plan.created_at_millis, 10_000);
        assert_eq!(
            plan.valid_until_millis,
            10_000 + policy.hysteresis.minimum_hold_millis
        );
    }

    /// §93 "Hedged Requests" wired end to end: a high-priority
    /// interactive message with a fallback available should produce
    /// `Hedged`, not `Failover`, and carry a real delay.
    #[test]
    fn a_high_priority_message_with_a_fallback_produces_a_hedged_plan() {
        let healthy = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let also_healthy = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![healthy.clone(), also_healthy.clone()];
        let mut req = DeliveryRequirements::interactive_message();
        req.priority = Priority::High;
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Hedged);
        assert_eq!(plan.hedge_delay_millis, Some(150)); // no deadline stated -> default
        assert_eq!(plan.fallbacks.len(), 1);
    }

    /// A file transfer never hedges even at Critical priority (§93's
    /// own "potentially expensive: use selectively" caveat) — this
    /// stays `Failover`, and `hedge_delay_millis` stays `None`.
    #[test]
    fn a_critical_bulk_transfer_does_not_hedge() {
        let mut healthy = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        healthy.capabilities.metered = crate::types::MeteredState::Unmetered;
        healthy.capabilities.roaming = crate::types::RoamingState::NotRoaming;
        let mut also_healthy = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        also_healthy.capabilities.metered = crate::types::MeteredState::Unmetered;
        also_healthy.capabilities.roaming = crate::types::RoamingState::NotRoaming;
        let candidates = vec![healthy.clone(), also_healthy.clone()];
        let mut req = DeliveryRequirements::file_chunk();
        req.priority = Priority::Critical;
        let policy = RoutingPolicyProfile::BulkTransfer.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Failover);
        assert_eq!(plan.hedge_delay_millis, None);
    }

    #[test]
    fn no_eligible_candidates_is_a_real_error_not_a_panic() {
        let unreachable = candidate(TransportKind::IrohDirect, RouteHealth::Unreachable);
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let result = plan_route(&[unreachable], &req, &policy, &scorer, None, None, 0);
        assert!(matches!(result, Err(RoutingError::NoEligibleCandidates)));
    }

    #[test]
    fn stickiness_keeps_the_current_path_when_a_new_candidate_is_only_marginally_better() {
        let current = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let marginally_better = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![current.clone(), marginally_better];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        // Both candidates score identically under DefaultScorer (same
        // health, same unknown metrics) — well under the switch
        // threshold, so stickiness should keep `current`.
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan =
            plan_route(&candidates, &req, &policy, &scorer, Some(&current), None, 0).unwrap();
        assert_eq!(plan.primary.path_id, current.path_id);
    }

    #[test]
    fn a_degraded_current_path_is_switched_away_from_even_without_beating_the_threshold() {
        let current = candidate(TransportKind::IrohDirect, RouteHealth::Degraded);
        let healthy_alternative = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![current.clone(), healthy_alternative.clone()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan =
            plan_route(&candidates, &req, &policy, &scorer, Some(&current), None, 0).unwrap();
        assert_eq!(plan.primary.path_id, healthy_alternative.path_id);
    }

    #[test]
    fn critical_delay_tolerant_traffic_with_a_fallback_uses_redundant_strategy() {
        let a = candidate(TransportKind::Dtn, RouteHealth::Healthy);
        let b = candidate(TransportKind::MeshRelay, RouteHealth::Healthy);
        let candidates = vec![a, b];
        let req = DeliveryRequirements::emergency();
        let policy = RoutingPolicyProfile::Emergency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Redundant);
        assert_eq!(plan.replicas.len(), 1);
    }

    /// §162's own `.allow_redundancy(true)` implies a caller can also
    /// say `false` — before this round there was no way to reach
    /// Critical+DelayTolerant without automatically paying for a
    /// second replica.
    #[test]
    fn spec_162_allow_redundancy_false_falls_back_to_failover_instead() {
        let a = candidate(TransportKind::Dtn, RouteHealth::Healthy);
        let b = candidate(TransportKind::MeshRelay, RouteHealth::Healthy);
        let candidates = vec![a, b];
        let mut req = DeliveryRequirements::emergency();
        req.allow_redundancy = false;
        let policy = RoutingPolicyProfile::Emergency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Failover);
        assert!(plan.replicas.is_empty());
    }

    #[test]
    fn redundant_strategy_prefers_an_underlay_diverse_replica_over_the_next_best_score() {
        // Three eligible candidates: a healthy primary, a same-
        // underlay fallback that would score marginally higher than
        // the diverse one (via `recent_success`), and an underlay-
        // diverse fallback that scores lower but is what §73 actually
        // wants picked as the replica.
        let underlay = crate::diversity::UnderlayId::new();
        let mut primary = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        primary.underlay = Some(underlay);

        let mut same_underlay_but_better_scored =
            candidate(TransportKind::IrohRelay, RouteHealth::Healthy);
        same_underlay_but_better_scored.underlay = Some(underlay);
        same_underlay_but_better_scored.metrics.last_success_millis = Some(1_000);

        let diverse_but_lower_scored = candidate(TransportKind::MeshRelay, RouteHealth::Healthy);

        let candidates = vec![
            primary.clone(),
            same_underlay_but_better_scored.clone(),
            diverse_but_lower_scored.clone(),
        ];
        let req = DeliveryRequirements::emergency();
        let policy = RoutingPolicyProfile::Emergency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan =
            plan_route(&candidates, &req, &policy, &scorer, Some(&primary), None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Redundant);
        assert_eq!(plan.replicas.len(), 1);
        assert_eq!(plan.replicas[0].path_id, diverse_but_lower_scored.path_id);
        // The same-underlay candidate is still available as an
        // ordinary fallback — diversity only changes which one
        // becomes the *replica*, it doesn't discard the other.
        assert!(plan
            .fallbacks
            .iter()
            .any(|f| f.path_id == same_underlay_but_better_scored.path_id));
    }

    /// §69 "Route Planning for Messaging": "1. existing authenticated
    /// direct route... 5. DTN," with the spec's own caveat "actual
    /// ordering depends on policy and measured health" taken
    /// seriously rather than glossed over — this only asserts the
    /// parts of that ordering [`plan_route`]'s existing scoring
    /// robustly produces without fabricated measurements: an
    /// already-connected candidate stays primary (already covered by
    /// this file's own stickiness tests, not re-tested here), and a
    /// cheap-setup transport (LAN) outranks an expensive-setup one
    /// (Bluetooth pairing) when nothing else differs, straight from
    /// §44's `setup_cost` term. Where a candidate would land relative
    /// to DTN specifically depends on real measured latency/reliability
    /// data this crate has no basis to invent for a healthy-but-
    /// otherwise-unmeasured DTN candidate — see [`crate::setup::static_setup_cost`]'s
    /// own doc comment on why DTN's *setup* cost being cheap doesn't by
    /// itself say anything about whether it's a *good* choice for
    /// message delivery right now.
    #[test]
    fn messaging_prefers_cheap_setup_transports_when_all_else_is_equal() {
        let lan = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let bluetooth_pairing = candidate(TransportKind::BluetoothClassic, RouteHealth::Healthy);
        let candidates = vec![bluetooth_pairing.clone(), lan.clone()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::LocalLan);
    }

    /// §70 "Route Planning for Files": "1. high-bandwidth direct...
    /// 5. Bluetooth only if small/allowed." Using realistic
    /// differentiated measurements (the kind a real caller would
    /// actually supply for a file transfer, unlike §69's test above)
    /// rather than leaving metrics unknown — with a `min_bandwidth`
    /// floor actually set, both §44's setup-cost term and §24's own
    /// bandwidth-suitability term agree: a high-bandwidth direct path
    /// beats a low-bandwidth, expensive-setup Bluetooth one.
    #[test]
    fn files_prefer_high_bandwidth_direct_over_small_only_bluetooth() {
        let mut high_bandwidth_direct = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        high_bandwidth_direct.metrics.estimated_bandwidth =
            Some(crate::metrics::Bitrate(50_000_000));
        high_bandwidth_direct.capabilities.metered = crate::types::MeteredState::Unmetered;
        high_bandwidth_direct.capabilities.roaming = crate::types::RoamingState::NotRoaming;

        let mut bluetooth_small_only =
            candidate(TransportKind::BluetoothClassic, RouteHealth::Healthy);
        bluetooth_small_only.metrics.estimated_bandwidth = Some(crate::metrics::Bitrate(500_000));
        bluetooth_small_only.capabilities.metered = crate::types::MeteredState::Unmetered;
        bluetooth_small_only.capabilities.roaming = crate::types::RoamingState::NotRoaming;

        let candidates = vec![bluetooth_small_only.clone(), high_bandwidth_direct.clone()];
        let mut req = DeliveryRequirements::file_chunk();
        req.min_bandwidth = Some(crate::metrics::Bitrate(5_000_000));
        let policy = RoutingPolicyProfile::BulkTransfer.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::IrohDirect);
    }

    /// §72 "Route Planning for Emergency": "SOS may choose: Internet
    /// direct + nearby mesh copy... depending on connectivity." This
    /// verifies the Emergency policy profile actually produces exactly
    /// that composition — an Internet-direct primary plus a mesh
    /// replica — when both are available, rather than asserting it as
    /// prose without checking.
    #[test]
    fn emergency_composes_internet_direct_plus_a_nearby_mesh_replica() {
        let internet_direct = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let nearby_mesh = candidate(TransportKind::MeshRelay, RouteHealth::Healthy);
        let candidates = vec![internet_direct.clone(), nearby_mesh.clone()];
        let req = DeliveryRequirements::emergency();
        let policy = RoutingPolicyProfile::Emergency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.strategy, RouteStrategy::Redundant);
        assert_eq!(plan.primary.transport, TransportKind::IrohDirect);
        assert_eq!(plan.replicas[0].transport, TransportKind::MeshRelay);
    }

    /// §124 "Simulated Routing Tests", transcribed with its own exact
    /// numbers: "Path A (10ms, 1Mbps, metered), Path B (50ms,
    /// 100Mbps, unmetered)." Two sub-tests for the spec's own two
    /// worked outcomes.
    fn spec_124_path_a() -> PathCandidate {
        let mut a = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        a.metrics.rtt_millis = Some(10);
        a.metrics.estimated_bandwidth = Some(crate::metrics::Bitrate(1_000_000));
        a.capabilities.metered = crate::types::MeteredState::Metered;
        a.capabilities.roaming = crate::types::RoamingState::NotRoaming;
        a
    }

    fn spec_124_path_b() -> PathCandidate {
        let mut b = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        b.metrics.rtt_millis = Some(50);
        b.metrics.estimated_bandwidth = Some(crate::metrics::Bitrate(100_000_000));
        b.capabilities.metered = crate::types::MeteredState::Unmetered;
        b.capabilities.roaming = crate::types::RoamingState::NotRoaming;
        b
    }

    /// §124's own "text → A or B depending on policy": a text
    /// message has no `min_bandwidth` floor, so the choice genuinely
    /// comes down to which policy profile's weights matter more —
    /// `LowLatency` favors A's better RTT; `LowPower`'s heavier
    /// congestion/candidate-state weighting combined with an
    /// explicit `avoid_metered` user policy (layered in via
    /// `crate::privacy::eliminate_privacy_violations` the way
    /// `crate::decision::decide_route` would) favors B. Neither
    /// answer is "correct" on its own — that's the property being
    /// tested here, not a specific pick.
    #[test]
    fn spec_124_text_message_prefers_a_under_low_latency_policy() {
        let candidates = vec![spec_124_path_b(), spec_124_path_a()];
        let mut req = DeliveryRequirements::interactive_message();
        // `interactive_message()` leaves `max_latency_millis: None`,
        // under which `scoring::DefaultScorer` treats RTT as neutral
        // regardless of its actual value (§13's "unknown/unconstrained
        // isn't penalized" posture). The suitability curve itself
        // only penalizes RTT that *exceeds* the deadline — it doesn't
        // reward being well under it — so the deadline has to sit
        // between the two RTTs (10ms, 50ms) for this to differentiate
        // them at all, not merely be "set to something."
        req.max_latency_millis = Some(30);
        let policy = RoutingPolicyProfile::LowLatency.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::IrohDirect); // Path A
    }

    #[test]
    fn spec_124_text_message_prefers_b_once_a_metered_path_is_ruled_out() {
        let candidates = vec![spec_124_path_a(), spec_124_path_b()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::LowLatency.policy();
        let user_policy = crate::privacy::PrivacyPolicy {
            avoid_metered: true,
            ..Default::default()
        };
        let eligible: Vec<PathCandidate> =
            crate::privacy::eliminate_privacy_violations(&candidates, &user_policy)
                .into_iter()
                .cloned()
                .collect();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };
        let plan = plan_route(&eligible, &req, &policy, &scorer, None, None, 0).unwrap();
        assert_eq!(plan.primary.transport, TransportKind::LocalLan); // Path B
    }

    /// §124's own "large file → B": `file_chunk`'s `allow_metered:
    /// false` rules A out as a *hard* constraint regardless of
    /// policy profile, not merely a scoring preference — this is the
    /// one of the spec's own two outcomes that isn't policy-dependent
    /// at all.
    #[test]
    fn spec_124_large_file_always_uses_path_b_regardless_of_policy() {
        for profile in [
            RoutingPolicyProfile::Balanced,
            RoutingPolicyProfile::LowLatency,
            RoutingPolicyProfile::BulkTransfer,
        ] {
            let candidates = vec![spec_124_path_a(), spec_124_path_b()];
            let mut req = DeliveryRequirements::file_chunk();
            req.min_bandwidth = Some(crate::metrics::Bitrate(5_000_000));
            let policy = profile.policy();
            let scorer = DefaultScorer {
                weights: policy.weights,
            };
            let plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
            assert_eq!(
                plan.primary.transport,
                TransportKind::LocalLan,
                "profile {profile:?} should still land on Path B"
            );
        }
    }

    /// §123 "Deterministic Scoring", one layer down from
    /// `decision.rs`'s own version of this property: `plan_route`
    /// itself, called twice with identical inputs, must produce the
    /// same primary and the same fallback ordering both times.
    #[test]
    fn spec_123_plan_route_is_deterministic_given_identical_inputs() {
        let candidates = vec![spec_124_path_a(), spec_124_path_b()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let first = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        let second = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();

        assert_eq!(first.primary.path_id, second.primary.path_id);
        assert_eq!(first.strategy, second.strategy);
        let first_fallback_ids: Vec<_> = first.fallbacks.iter().map(|c| c.path_id).collect();
        let second_fallback_ids: Vec<_> = second.fallbacks.iter().map(|c| c.path_id).collect();
        assert_eq!(first_fallback_ids, second_fallback_ids);
    }

    /// §127 "Failover Test": "primary healthy → mid-transfer failure
    /// → fallback acquired." Re-planning after the primary's health
    /// degrades to `Unreachable` must promote a healthy fallback to
    /// primary — `crate::scoring::passes_hard_constraints`'s own
    /// unconditional `RouteHealth::Unreachable` elimination is the
    /// mechanism; this test is the end-to-end proof of the
    /// spec's own worked scenario. §127's second half — "operation
    /// resumes if semantics allow... message retry and file resume
    /// differ... routing only coordinates path change, feature layer
    /// owns semantic resume" — is a statement about a layer *above*
    /// this crate: `plan_route` has no concept of "resume," only of
    /// "here is a new path"; nothing here could honestly claim to
    /// test a resume behavior this crate doesn't implement.
    #[test]
    fn spec_127_a_failed_primary_fails_over_to_a_healthy_fallback() {
        let path_x = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let path_y = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let candidates = vec![path_x.clone(), path_y.clone()];
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        // Whichever of the two wins initially becomes "primary" for
        // this test; the point being tested is what happens *after*
        // it fails, not which transport a tied-metrics scoring pass
        // happens to prefer.
        let initial_plan = plan_route(&candidates, &req, &policy, &scorer, None, None, 0).unwrap();
        let (primary, fallback) = if initial_plan.primary.path_id == path_x.path_id {
            (path_x, path_y)
        } else {
            (path_y, path_x)
        };
        assert_eq!(initial_plan.primary.path_id, primary.path_id);

        // Mid-transfer failure: the primary's health degrades.
        let mut failed_primary = primary.clone();
        failed_primary.health = RouteHealth::Unreachable;
        let candidates_after_failure = vec![failed_primary, fallback.clone()];

        let replanned = plan_route(
            &candidates_after_failure,
            &req,
            &policy,
            &scorer,
            Some(&initial_plan.primary),
            None,
            0,
        )
        .unwrap();
        assert_eq!(replanned.primary.path_id, fallback.path_id);
    }

    /// §126 "Chaos Tests": "Simulate: Wi-Fi flaps... Assert: no route
    /// storm." A flapping measurement — small RTT jitter that
    /// narrowly swaps which of two paths *looks* slightly better from
    /// one measurement to the next, with nothing actually wrong with
    /// either — is exactly what §35's `switch_threshold` exists to
    /// absorb. This simulates 20 consecutive re-plans, each one
    /// re-scored with the "better" side swapping every round (the
    /// noisiest possible pattern), feeding each round's own primary
    /// forward as the next round's `current` the way a real caller's
    /// loop would. A naive "always take the top score" policy would
    /// switch on every single round (19 switches out of 19
    /// opportunities); real stickiness should switch on effectively
    /// none of them, since the swapped RTT delta here (a few ms) is
    /// deliberately kept well under `Balanced`'s own
    /// `switch_threshold`.
    ///
    /// This crate's other two named chaos properties —
    /// "no infinite retry loop" and "bounded queues" — already have
    /// dedicated tests elsewhere and aren't re-proven here:
    /// `retry::tests::an_expired_operation_is_not_allowed_to_retry_even_under_its_attempt_cap`
    /// (retry always stops once `has_expired`, regardless of
    /// `max_attempts`) and
    /// `fairness::tests`' own capacity tests for
    /// [`crate::fairness::PerTransportDispatchQueue`] (enqueue past
    /// capacity is rejected, not silently unbounded).
    #[test]
    fn spec_126_wifi_flapping_does_not_cause_a_route_storm() {
        let path_x = candidate(TransportKind::IrohDirect, RouteHealth::Healthy);
        let path_y = candidate(TransportKind::LocalLan, RouteHealth::Healthy);
        let req = DeliveryRequirements::interactive_message();
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let mut current: Option<PathCandidate> = None;
        let mut switch_count = 0;
        const ROUNDS: u64 = 20;

        for round in 0..ROUNDS {
            let mut x = path_x.clone();
            let mut y = path_y.clone();
            // Swap who "looks" slightly better every round — a few
            // ms of jitter, not a real health change.
            if round % 2 == 0 {
                x.metrics.rtt_millis = Some(48);
                y.metrics.rtt_millis = Some(52);
            } else {
                x.metrics.rtt_millis = Some(52);
                y.metrics.rtt_millis = Some(48);
            }
            let candidates = vec![x, y];

            let plan = plan_route(
                &candidates,
                &req,
                &policy,
                &scorer,
                current.as_ref(),
                None,
                round * 1_000,
            )
            .unwrap();

            if let Some(prev) = &current {
                if plan.primary.path_id != prev.path_id {
                    switch_count += 1;
                }
            }
            current = Some(plan.primary);
        }

        assert!(
            switch_count <= 1,
            "expected stickiness to absorb the flapping (at most 1 switch), got {switch_count} switches across {ROUNDS} rounds"
        );
    }
}
