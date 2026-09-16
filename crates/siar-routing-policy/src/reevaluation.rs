//! §189 "Call Routing Frequency", §190 "File Routing Frequency", §191
//! "Message Routing Frequency".
//!
//! All three are, at bottom, the same question asked for three
//! different traffic shapes: "when should a caller actually re-run
//! the planner, as opposed to just keeping the plan it already has?"
//! §191's own answer needed nothing new: "reuse healthy session route
//! until invalidated. No expensive full scoring every time" is
//! exactly [`crate::cache::RouteCache`]/[`crate::explain::RouteHint`]
//! (§100-101, real since round 9) — a caller checks
//! [`crate::explain::revalidate_hint`] first and only calls
//! [`crate::plan::plan_route`] when that comes back invalid. §189/§190
//! are the two cases with genuinely different triggers, so they get
//! their own functions: [`quality_change_exceeds_threshold`] and
//! [`should_reevaluate_file_route`].

use crate::scoring::{RouteScore, RouteScoreDelta};
use crate::types::RouteHealth;

/// §189: "don't rerun full planner every frame... trigger
/// reevaluation only when thresholds cross." Reuses
/// [`RouteScoreDelta`] — [`crate::policy::HysteresisPolicy::switch_threshold`]'s
/// own field type — rather than inventing a second threshold concept;
/// a caller already has one of these configured for exactly this kind
/// of "how much change is worth reacting to" judgment. Deliberately
/// compares two whole [`RouteScore`]s, not the raw metrics underneath
/// them: a caller running this once per frame would otherwise be
/// re-deriving `DefaultScorer`'s own suitability terms itself just to
/// decide whether to call the real scorer, which defeats the point of
/// "no expensive full scoring every time."
pub fn quality_change_exceeds_threshold(
    previous: RouteScore,
    current: RouteScore,
    threshold: RouteScoreDelta,
) -> bool {
    (previous.0 - current.0).abs() > threshold.0
}

/// §190's own four named triggers, joined by "or" — any one of them
/// alone is reason enough to re-plan a file transfer; none of them
/// individually implies the others, so this isn't collapsible into a
/// single boolean the way §189's threshold check is. Explicitly not
/// evaluated per-chunk ("not per packet," per the spec's own
/// wording) — a caller checks this at whatever cadence its own
/// transfer loop naturally reaches these events, not inside the
/// per-chunk write loop itself.
#[allow(clippy::too_many_arguments)]
pub fn should_reevaluate_file_route(
    path_failed: bool,
    quality_change_significant: bool,
    superior_path_available: bool,
    policy_changed: bool,
) -> bool {
    path_failed || quality_change_significant || superior_path_available || policy_changed
}

/// A small helper for §190's own "path failure" trigger specifically
/// — reading `RouteHealth::Unreachable` as "failed" the same way
/// [`crate::scoring::passes_hard_constraints`] already treats that
/// variant as disqualifying, so a caller doesn't need to duplicate
/// that judgment call itself.
pub fn path_has_failed(health: RouteHealth) -> bool {
    health == RouteHealth::Unreachable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_189_a_change_within_the_threshold_does_not_trigger_reevaluation() {
        assert!(!quality_change_exceeds_threshold(
            RouteScore(5.0),
            RouteScore(5.2),
            RouteScoreDelta(0.3)
        ));
    }

    #[test]
    fn spec_189_a_change_beyond_the_threshold_triggers_reevaluation() {
        assert!(quality_change_exceeds_threshold(
            RouteScore(5.0),
            RouteScore(6.0),
            RouteScoreDelta(0.3)
        ));
    }

    #[test]
    fn spec_189_the_check_is_symmetric_improvement_also_counts() {
        // A route getting *better* by a lot is just as much a reason
        // to reconsider (a superior path may now be worth switching
        // to) as one getting worse.
        assert!(quality_change_exceeds_threshold(
            RouteScore(3.0),
            RouteScore(9.0),
            RouteScoreDelta(0.3)
        ));
    }

    #[test]
    fn spec_190_any_single_trigger_is_enough_to_reevaluate() {
        assert!(should_reevaluate_file_route(true, false, false, false));
        assert!(should_reevaluate_file_route(false, true, false, false));
        assert!(should_reevaluate_file_route(false, false, true, false));
        assert!(should_reevaluate_file_route(false, false, false, true));
    }

    #[test]
    fn spec_190_no_triggers_means_no_reevaluation() {
        assert!(!should_reevaluate_file_route(false, false, false, false));
    }

    #[test]
    fn path_has_failed_matches_only_unreachable() {
        assert!(path_has_failed(RouteHealth::Unreachable));
        assert!(!path_has_failed(RouteHealth::Degraded));
        assert!(!path_has_failed(RouteHealth::Healthy));
    }
}
