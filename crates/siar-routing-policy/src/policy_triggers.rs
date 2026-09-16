//! §180 "Dynamic Policy Update", §181 "Call-Induced Policy Change",
//! §182 "Emergency-Induced Policy Change" — grouped as "policy
//! triggers": application-lifecycle events (a call starting, an
//! emergency beginning) that should change routing's behavior while
//! they're active and revert once they end.
//!
//! §180 itself needs no code: "policy can change at runtime... routing
//! re-evaluates relevant active operations." This crate has no
//! registry of "active operations" to re-evaluate (same "no history"
//! boundary this crate has kept consistently — see
//! [`crate::explain::RouteMetricEvent`]'s own doc comment) — that
//! registry, and the decision to call this crate again when something
//! changes, is a caller's job. What makes that job possible is
//! [`crate::decision::decide_route`]/[`crate::plan::plan_route`] being
//! pure functions with no hidden state of their own: calling either
//! twice with the same inputs gives the same answer (§123, already
//! proved), and calling either again with *different* inputs — a new
//! [`crate::privacy::PrivacyPolicy`], a new
//! [`crate::platform::DeviceState`] — is already exactly how a caller
//! would "re-evaluate." Nothing here needed to change for that to be
//! true.
//!
//! §181/§182 are the two *specific* triggers the spec names, and both
//! get real functions: [`hysteresis_for_call_state`] and
//! [`emergency_effective_requirements`].

use crate::policy::HysteresisPolicy;
use crate::requirements::DeliveryRequirements;
use crate::scoring::RouteScoreDelta;

/// §181's own three during-call effects, checked against what already
/// has a home elsewhere before adding anything new: "bulk file
/// traffic throttled" is
/// [`crate::resource_pressure::TrafficShapingPolicy`]'s own
/// `bulk_max_bitrate_during_call` (§141, already real — its very name
/// already says "during a call"); "realtime priority increased" is
/// nothing new either — a call's own
/// [`crate::requirements::DeliveryRequirements::priority`] is already
/// whatever the caller sets it to when building the call's own
/// request. Only "path switching hysteresis increased" had no
/// existing lever: [`hysteresis_for_call_state`].
///
/// "After call: normal policy restored" needs no explicit reset
/// logic — this is a pure function of `call_active`, not a stored
/// state, so calling it again with `call_active: false` *is*
/// "restored," the same way §180 above already established for
/// policy changes generally.
pub fn hysteresis_for_call_state(base: HysteresisPolicy, call_active: bool) -> HysteresisPolicy {
    if !call_active {
        return base;
    }
    HysteresisPolicy {
        // Doubled, not an arbitrary bump — a call's whole point in
        // §181's own list is *reducing* switching, and a threshold a
        // caller already tuned as "the right sensitivity" deserves a
        // multiple of itself, not a made-up absolute increment that
        // might already exceed it.
        switch_threshold: RouteScoreDelta(base.switch_threshold.0 * 2.0),
        minimum_hold_millis: base.minimum_hold_millis * 2,
        degraded_override: base.degraded_override,
    }
}

/// §182's own three named effects, checked the same way: "increase
/// critical queue weight" is a dispatch-scheduler concern
/// ([`crate::fairness::RoundRobinFairQueue`]'s own per-key fairness,
/// or `siar-protocol-ext`'s scheduler internals) this crate doesn't
/// own the internals of; "enable proximity" reads as activating
/// device *hardware* (BLE/proximity sensing) rather than a routing
/// policy field, the same kind of platform action §130's
/// rebind/renegotiate is out of scope for. Only "allow DTN" maps onto
/// a real field this crate owns —
/// [`DeliveryRequirements::allow_dtn`] — and only when
/// `user_opted_in` is also true: the spec's own "while still honoring
/// user opt-in" is not a suggestion, it's the same explicit-consent
/// shape §138 "Emergency Override" already established for a
/// different field (`avoid_relay`). An emergency with no opt-in
/// changes nothing here, same as §138.
pub fn emergency_effective_requirements(
    base: DeliveryRequirements,
    emergency_active: bool,
    user_opted_in: bool,
) -> DeliveryRequirements {
    if emergency_active && user_opted_in {
        DeliveryRequirements {
            allow_dtn: true,
            ..base
        }
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_181_an_active_call_doubles_both_hysteresis_fields() {
        let base = HysteresisPolicy {
            switch_threshold: RouteScoreDelta(0.3),
            minimum_hold_millis: 10_000,
            degraded_override: true,
        };
        let during_call = hysteresis_for_call_state(base, true);
        assert_eq!(during_call.switch_threshold, RouteScoreDelta(0.6));
        assert_eq!(during_call.minimum_hold_millis, 20_000);
        assert!(during_call.degraded_override);
    }

    #[test]
    fn spec_181_after_the_call_the_same_function_restores_the_base_policy() {
        let base = HysteresisPolicy {
            switch_threshold: RouteScoreDelta(0.3),
            minimum_hold_millis: 10_000,
            degraded_override: true,
        };
        let after_call = hysteresis_for_call_state(base, false);
        assert_eq!(after_call.switch_threshold, base.switch_threshold);
        assert_eq!(after_call.minimum_hold_millis, base.minimum_hold_millis);
    }

    #[test]
    fn spec_182_emergency_only_relaxes_dtn_with_explicit_opt_in() {
        let mut base = DeliveryRequirements::interactive_message();
        base.allow_dtn = false;

        let with_opt_in = emergency_effective_requirements(base.clone(), true, true);
        assert!(with_opt_in.allow_dtn);

        let without_opt_in = emergency_effective_requirements(base.clone(), true, false);
        assert!(
            !without_opt_in.allow_dtn,
            "emergency alone, without explicit opt-in, must never override — no silent policy override"
        );

        let no_emergency = emergency_effective_requirements(base, false, true);
        assert!(!no_emergency.allow_dtn);
    }
}
