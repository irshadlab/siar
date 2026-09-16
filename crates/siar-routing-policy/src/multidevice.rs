//! §171 "Multi-Device Route Aggregation", §172 "Device Preference",
//! §173 "Group Routing", §175 "Route Constraints by Content
//! Sensitivity".
//!
//! ## §171
//!
//! "Account target may use parallel routes to multiple devices, with
//! per-device plan. Do not flatten all devices into one route score."
//! [`crate::resolve::resolve_destination_devices`] already resolves
//! an [`crate::types::Destination::Account`] to every one of its
//! active devices (unchanged, real since round 1) — but every
//! decision function built on top of it since then
//! ([`crate::plan::plan_route`], [`crate::decision::decide_route`])
//! takes one flat candidate list and produces exactly one winning
//! plan. Handed all of an account's devices' candidates pooled
//! together, that's precisely the "one route score" the spec says not
//! to produce: whichever single device's candidate scores highest
//! wins, and every other device — phone, laptop, tablet — is silently
//! dropped, even though the point of a multi-device account is
//! usually to reach more than one of them. [`plan_per_device`] is the
//! fix: it never pools candidates across devices at all — each
//! device's own slice is scored and planned in total isolation from
//! every other device's, via its own independent
//! [`crate::plan::plan_route`] call, so a failure or a bad score on
//! one device's path can never influence another device's plan.
//!
//! ## §172
//!
//! "Application may mark: primary device, preferred file device,
//! call-capable device. Routing uses as policy input, not immutable
//! identity." [`DeviceRole`]/[`devices_matching_role_for_class`] is
//! deliberately a *preference with fallback*, not a filter that can
//! leave zero devices targeted: an operation whose class has a
//! matching role-tagged device narrows to just those; one that
//! doesn't (no device tagged, or an ordinary [`crate::types::DeliveryClass::Reliable`]
//! message with no role logic at all) falls back to every device —
//! "policy input" the way the spec's own wording draws the line, not
//! "immutable identity" that could exclude every device on a
//! misconfiguration.
//!
//! ## §173
//!
//! "Group messaging does not mean one path to group. It may fan out
//! to devices or use group dissemination protocol. Routing provides
//! per-destination transport choices." The "fan out to devices" half
//! is exactly [`plan_per_device`] again, applied to a group's
//! resolved member list instead of one account's device list — no
//! new function needed, since neither this module nor
//! [`plan_per_device`] cares *why* a caller is calling it once per
//! recipient. "Use group dissemination protocol" (an actual
//! multicast/gossip transport, as opposed to N individual unicast
//! plans) is out of scope — that's a messaging or Part-06 concern,
//! not a routing-decision one. Worth naming plainly rather than
//! glossed over: [`crate::resolve::resolve_destination_devices`]
//! itself doesn't yet resolve [`crate::types::Destination::Group`] at
//! all (it returns [`crate::error::RoutingError::NoActiveDevicesForAccount`]
//! for that variant today) — a real, still-open gap that predates
//! this round and isn't closed by it; [`plan_per_device`] is ready to
//! consume a group's member list the moment something resolves one.
//!
//! ## §175
//!
//! "High-sensitivity content may forbid untrusted store-and-forward...
//! Represent `forwarding_allowed`, `relay_allowed` explicitly." Both
//! already exist, named slightly differently:
//! [`crate::requirements::DeliveryRequirements::allow_dtn`] *is*
//! `forwarding_allowed` (DTN is exactly "store-and-forward" — the
//! spec's own §56 already frames it that way) and `allow_relay` is
//! `relay_allowed`, unchanged. Zero new code — an application
//! representing content sensitivity as a policy input sets these two
//! existing fields to `false`, the same way every other
//! per-operation restriction in this crate already works.

use std::collections::HashMap;

use crate::candidate::PathCandidate;
use crate::error::RoutingError;
use crate::plan::{plan_route, RoutePlan};
use crate::platform::DeviceState;
use crate::policy::RoutingPolicy;
use crate::requirements::DeliveryRequirements;
use crate::scoring::PathScorer;
use crate::types::DeliveryClass;
use siar_domain::DeviceId;

/// §171: one independent [`crate::plan::plan_route`] call per device,
/// never a shared scoring pass. `current_by_device` and `device` are
/// the same optional context [`plan_route`] itself already accepts,
/// just threaded through per device rather than assumed identical for
/// all of them (different devices can have genuinely different
/// current sessions and even different platform state, if a caller
/// has that granularity).
pub fn plan_per_device(
    candidates_by_device: &HashMap<DeviceId, Vec<PathCandidate>>,
    req: &DeliveryRequirements,
    policy: &RoutingPolicy,
    scorer: &dyn PathScorer,
    current_by_device: &HashMap<DeviceId, PathCandidate>,
    device: Option<&DeviceState>,
    now_millis: u64,
) -> HashMap<DeviceId, Result<RoutePlan, RoutingError>> {
    candidates_by_device
        .iter()
        .map(|(device_id, candidates)| {
            let current = current_by_device.get(device_id);
            let result = plan_route(candidates, req, policy, scorer, current, device, now_millis);
            (*device_id, result)
        })
        .collect()
}

/// §172, transcribed with its own three named roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceRole {
    Primary,
    PreferredFile,
    CallCapable,
}

/// See this module's own doc comment for why this always returns a
/// non-empty subset of `all_devices` (given `all_devices` itself is
/// non-empty) rather than a filter that could zero one out.
/// `DeviceRole::Primary` narrows nothing here — it's a general
/// preference the spec names but ties to no specific
/// [`DeliveryClass`], unlike the other two.
pub fn devices_matching_role_for_class(
    class: DeliveryClass,
    roles: &HashMap<DeviceId, Vec<DeviceRole>>,
    all_devices: &[DeviceId],
) -> Vec<DeviceId> {
    let wanted_role = match class {
        DeliveryClass::Realtime => Some(DeviceRole::CallCapable),
        DeliveryClass::Bulk => Some(DeviceRole::PreferredFile),
        DeliveryClass::Interactive | DeliveryClass::Reliable | DeliveryClass::DelayTolerant => None,
    };

    let Some(wanted_role) = wanted_role else {
        return all_devices.to_vec();
    };

    let matching: Vec<DeviceId> = all_devices
        .iter()
        .filter(|d| roles.get(d).is_some_and(|r| r.contains(&wanted_role)))
        .copied()
        .collect();

    if matching.is_empty() {
        all_devices.to_vec()
    } else {
        matching
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::metrics::PathMetrics;
    use crate::policy::RoutingPolicyProfile;
    use crate::scoring::DefaultScorer;
    use crate::types::{
        MeteredState, PathCapabilities, PathId, RoamingState, RouteHealth, TransportKind,
    };

    fn candidate(peer: DeviceId, rtt: u32) -> PathCandidate {
        let mut c = PathCandidate {
            path_id: PathId::new(),
            transport: TransportKind::IrohDirect,
            peer,
            endpoint: TransportEndpoint(Vec::new()),
            metrics: PathMetrics::unknown(),
            capabilities: PathCapabilities {
                reliable_stream: true,
                datagram: false,
                large_files: false,
                realtime_media: false,
                peer_discovery: false,
                store_and_forward: false,
                metered: MeteredState::Unmetered,
                roaming: RoamingState::NotRoaming,
                requires_foreground: false,
            },
            health: RouteHealth::Healthy,
            underlay: None,
            state: crate::acquisition::CandidateState::Active,
        };
        c.metrics.rtt_millis = Some(rtt);
        c
    }

    #[test]
    fn spec_171_a_bad_device_never_influences_another_devices_plan() {
        let phone = DeviceId::new();
        let laptop = DeviceId::new();
        let mut by_device = HashMap::new();
        // The laptop's only candidate is excellent; the phone's only
        // candidate is metered and forbidden — under a *pooled* score
        // the phone would simply lose and disappear. Per-device, it
        // must still get its own (failing) answer independently.
        by_device.insert(laptop, vec![candidate(laptop, 5)]);
        let mut forbidden = candidate(phone, 5);
        forbidden.capabilities.metered = MeteredState::Metered;
        by_device.insert(phone, vec![forbidden]);

        let mut req = DeliveryRequirements::interactive_message();
        req.allow_metered = false;
        let policy = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: policy.weights,
        };

        let results = plan_per_device(&by_device, &req, &policy, &scorer, &HashMap::new(), None, 0);

        assert!(results.get(&laptop).unwrap().is_ok());
        assert!(results.get(&phone).unwrap().is_err());
    }

    #[test]
    fn spec_172_a_call_prefers_the_call_capable_device_when_one_is_tagged() {
        let phone = DeviceId::new();
        let tablet = DeviceId::new();
        let mut roles = HashMap::new();
        roles.insert(phone, vec![DeviceRole::CallCapable]);
        let all = vec![phone, tablet];

        let targets = devices_matching_role_for_class(DeliveryClass::Realtime, &roles, &all);
        assert_eq!(targets, vec![phone]);
    }

    #[test]
    fn spec_172_falls_back_to_every_device_when_none_is_tagged() {
        let phone = DeviceId::new();
        let tablet = DeviceId::new();
        let all = vec![phone, tablet];

        let targets =
            devices_matching_role_for_class(DeliveryClass::Realtime, &HashMap::new(), &all);
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn an_ordinary_message_class_targets_every_device_regardless_of_roles() {
        let phone = DeviceId::new();
        let tablet = DeviceId::new();
        let mut roles = HashMap::new();
        roles.insert(phone, vec![DeviceRole::CallCapable]);
        let all = vec![phone, tablet];

        let targets = devices_matching_role_for_class(DeliveryClass::Reliable, &roles, &all);
        assert_eq!(targets.len(), 2);
    }
}
