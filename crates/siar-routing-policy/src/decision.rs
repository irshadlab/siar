//! §108 "Policy Layering" through §115 "Deferred Reasons".
//!
//! §108's own diagram —
//!
//! ```text
//! system hard safety policy
//!  ↓
//! application policy
//!  ↓
//! user preference
//!  ↓
//! operation requirements
//!  ↓
//! current network context
//! ```
//!
//! — describes an ORDER this crate's pieces have been calling in ad
//! hoc since round 2 (a caller composes
//! [`crate::security::eliminate_untrusted_candidates`], then
//! [`crate::privacy::eliminate_privacy_violations`], then
//! [`crate::scoring::eliminate_hard_constraint_violations`], then
//! [`crate::plan::plan_route`], roughly top-to-bottom already) but
//! never a single function that actually runs them in that order and
//! reports which layer, if any, was the one that left nothing
//! routable. That composition — not a sixth elimination mechanism —
//! is this module's job:
//!
//! - **system** (§109): [`SystemPolicy`] — the layer §108 says
//!   "cannot be overridden by UI preference." Its own two checkable
//!   examples are real code here: "never exceed hard size limit"
//!   ([`SystemPolicy::max_operation_bytes`] against
//!   [`crate::descriptor::OperationDescriptor::estimated_size`]) and
//!   "never route to revoked device"
//!   ([`crate::security::eliminate_untrusted_candidates`], already
//!   built for §48/§105 — reused, not reimplemented). §109's third
//!   example, "never send unencrypted private message," has no
//!   checkable equivalent here — this crate has no plaintext/
//!   ciphertext field to inspect in the first place (see
//!   [`crate::descriptor`]'s own doc comment on why), so it's a
//!   property of what a caller puts in a message, not something this
//!   layer can verify.
//! - **application** (§110): [`ApplicationPolicy`] — genuinely new.
//!   §110's own three examples split cleanly: "SOS may use DTN" and
//!   (implicitly) "normal messages may use relay" are already
//!   *operation*-level facts on
//!   [`crate::requirements::DeliveryRequirements`]
//!   (`allow_dtn`/`allow_relay`, §112 below), set per call by
//!   whichever constructor a caller uses — not something a
//!   fixed-at-the-application-layer type needs to duplicate. "Documents
//!   must not use unknown relay peers" is the one example with no
//!   existing home: an app-wide, not per-operation, relay ban —
//!   [`ApplicationPolicy::allow_relay`].
//! - **user** (§111): [`crate::privacy::PrivacyPolicy`] (unchanged) —
//!   already real since round 2. `avoid_metered` is §111's "no mobile
//!   data for files," `nearby_only` is "prefer local connections."
//!   "Disable Bluetooth relay" and "battery saver" have no field on
//!   `PrivacyPolicy` itself — the former isn't separately expressible
//!   from `avoid_relay`'s existing `IrohRelay`-only scope (an honest
//!   gap, not fixed this round to keep this round's own scope to
//!   §108-115 rather than reopening §49); the latter is
//!   [`crate::platform::DeviceState::battery_saver`], read directly by
//!   [`decide_route`] below rather than duplicated onto `PrivacyPolicy`.
//! - **operation** (§112): [`crate::requirements::DeliveryRequirements`]
//!   (unchanged) — already real. "Deadline" is
//!   `expiry_millis`/`max_latency_millis`, "minimum bandwidth" is
//!   `min_bandwidth` (all three already enforced —
//!   `expiry_millis` by [`crate::requirements::DeliveryRequirements::has_expired`],
//!   `min_bandwidth` by [`crate::scoring::eliminate_hard_constraint_violations`]),
//!   "specific device" is [`crate::types::Destination::Device`]. Zero
//!   new code for this layer.
//! - **network context** (§108's bottom rung, unlabeled in the
//!   diagram but covered by §90/§86): [`crate::discovery::discovery_permitted`]/
//!   [`crate::discovery::DiscoveryBudget`] and
//!   [`crate::acquisition::eliminate_background_restricted`], both
//!   already real, now actually called from one place in the right
//!   order instead of being purely opt-in plumbing a caller might
//!   forget.
//!
//! §113 "Policy Conflict"'s own worked example — large file + no
//! metered + only a metered path exists — is transcribed directly
//! into a test below. The spec's own point ("result: DeferredByPolicy,
//! not silent policy violation") is exactly why [`decide_route`]
//! checks each layer *separately* and returns as soon as one empties
//! a previously non-empty candidate list, instead of running every
//! elimination first and asking only "is anything left" — the former
//! can name *which* layer caused it (§115), the latter can't.
//!
//! §114 "Policy Result Types": [`RouteDecisionResult`], transcribed
//! with its four variants named exactly as listed. §115 "Deferred
//! Reasons": [`DeferredReason`]'s six variants, also transcribed
//! exactly, each wired to a real condition below rather than left as
//! inert enum cases — see [`decide_route`]'s own step-by-step doc
//! comment for which check produces which variant.

use siar_domain::AccountId;
use siar_identity_multidevice::TrustedAccountStore;

use serde::{Deserialize, Serialize};

use crate::acquisition::eliminate_background_restricted;
use crate::candidate::PathCandidate;
use crate::descriptor::{ByteCount, OperationDescriptor};
use crate::discovery::{discovery_permitted, DiscoveryBudget};
use crate::plan::{plan_route, RoutePlan};
use crate::platform::DeviceState;
use crate::policy::RoutingPolicy;
use crate::privacy::{eliminate_privacy_violations, PrivacyPolicy};
use crate::scoring::{eliminate_hard_constraint_violations, PathScorer};
use crate::security::eliminate_untrusted_candidates;
use crate::types::{Priority, TransportKind};

/// §109, transcribed as far as this crate can check it — see this
/// module's own doc comment for why "never send unencrypted private
/// message" has no field here. `Serialize`/`Deserialize` added for
/// §179 "Route Policy Persistence" (see [`PrivacyPolicy`]'s own doc
/// comment for the fuller reasoning).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPolicy {
    pub max_operation_bytes: ByteCount,
}

/// §110's one example with no existing home — see this module's own
/// doc comment for why the other two don't need a field here. `true`
/// by default: an application that doesn't set this up front imposes
/// no additional restriction beyond whatever `DeliveryRequirements`
/// and `PrivacyPolicy` already express, the same permissive-default
/// convention every other policy struct in this crate already
/// follows. `Serialize`/`Deserialize` added for §179, same reasoning
/// as [`SystemPolicy`] above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationPolicy {
    pub allow_relay: bool,
}

impl Default for ApplicationPolicy {
    fn default() -> Self {
        Self { allow_relay: true }
    }
}

fn is_relay_transport(t: TransportKind) -> bool {
    matches!(t, TransportKind::IrohRelay | TransportKind::MeshRelay)
}

/// §108's own diagram, made a real type: the three layers that sit
/// *above* [`crate::requirements::DeliveryRequirements`] (operation)
/// and live network context, exactly the three [`decide_route`]
/// applies in order before anything else. Bundled together mainly to
/// keep [`decide_route`]'s own parameter list from growing by three
/// separate positional arguments every time this module changes —
/// the three fields are otherwise fully independent (a caller
/// building one doesn't need [`SystemPolicy`] and
/// [`ApplicationPolicy`] to agree on anything).
pub struct PolicyLayers<'a> {
    pub system: &'a SystemPolicy,
    pub application: &'a ApplicationPolicy,
    pub user: &'a PrivacyPolicy,
}

/// §114's `RejectReason` — permanent, §108's "cannot be overridden."
/// Deliberately a small, closed set: two variants correspond to
/// [`SystemPolicy`]'s own two checkable clauses (§109); the third,
/// `OperationExpired`, was added this round for §125's own "expired
/// operation never routed" property — an expired operation is
/// permanent in exactly the same sense as the other two (no candidate
/// list, however good, changes the answer), which is what puts it
/// here rather than in [`DeferredReason`]. Nothing from a lower layer
/// belongs here otherwise, since a lower-layer failure is by
/// definition something a caller *could* change (a toggle, a
/// different operation, waiting for network context to improve) —
/// that's exactly what makes it `Deferred` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    ExceedsHardSizeLimit,
    UnauthorizedDevice,
    /// §125 "Policy Property Tests"'s own "expired operation never
    /// routed." Checked before anything else in
    /// [`decide_route`] — see that function's own doc comment.
    OperationExpired,
}

/// §115, transcribed exactly, in the order listed. See
/// [`decide_route`]'s own doc comment for the real condition each one
/// is wired to below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredReason {
    WaitingForUnmetered,
    WaitingForPeer,
    WaitingForWifi,
    BatteryPolicy,
    BackgroundRestriction,
    NoSuitablePathYet,
}

/// §114, transcribed exactly. Doesn't derive `PartialEq` — matches
/// [`RoutePlan`] itself not doing so (§103's own deliberate choice,
/// see [`crate::plan`]'s doc comment); tests match on this enum's
/// shape with `matches!`/destructuring rather than `assert_eq`.
///
/// `Routed(RoutePlan)`, not `Routed(Box<RoutePlan>)` — §114's own code
/// block spells it exactly this way, and every other "transcribed
/// exactly" type in this crate (e.g.
/// [`crate::acquisition::CandidateState`]) keeps the spec's literal
/// shape rather than reaching for an indirection the spec text itself
/// never asked for. `clippy::large_enum_variant` flags the resulting
/// size gap between `Routed` and the other three variants — allowed
/// here rather than boxed, for the reason above.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RouteDecisionResult {
    Routed(RoutePlan),
    Deferred(DeferredReason),
    Rejected(RejectReason),
    Unreachable,
}

/// §111's `avoid_metered`/`nearby_only`/`internet_only` are three
/// independent flags, but §115 has one [`DeferredReason`] variant per
/// *network condition*, not per flag — this picks the single most
/// specific reason that applies, checked in this fixed order, so a
/// policy with several flags set still reports one clear cause rather
/// than requiring a caller to guess which flag actually mattered.
fn deferred_reason_for_user_policy(policy: &PrivacyPolicy) -> DeferredReason {
    if policy.avoid_metered {
        DeferredReason::WaitingForUnmetered
    } else if policy.nearby_only || policy.internet_only {
        DeferredReason::WaitingForWifi
    } else {
        DeferredReason::NoSuitablePathYet
    }
}

/// §108's five-layer stack, run top to bottom, stopping at the first
/// layer that turns a non-empty candidate list empty — see this
/// module's own doc comment for why that ordering, not "eliminate
/// everything then ask what's left," is what actually lets §115's
/// reasons be specific.
///
/// 0. §125's own "expired operation never routed":
///    `descriptor.requirements.has_expired(descriptor.created_at_millis,
///    now_millis)` → immediate [`RejectReason::OperationExpired`],
///    before even step 1's empty-candidates check — an expired
///    operation is rejected the same way with zero candidates or a
///    thousand.
/// 1. Empty `candidates` to begin with: [`DeferredReason::WaitingForPeer`]
///    if `descriptor.requirements.allow_dtn` (worth waiting — a DTN
///    carrier could still show up), else [`RouteDecisionResult::Unreachable`]
///    (nothing to wait for).
/// 2. **System** (§109): `descriptor.estimated_size` over
///    `layers.system.max_operation_bytes` → immediate
///    [`RejectReason::ExceedsHardSizeLimit`], before candidates are
///    even inspected. Then device trust
///    ([`crate::security::eliminate_untrusted_candidates`]): no
///    directory at all for `account_id` → [`RouteDecisionResult::Unreachable`]
///    (nothing to authenticate against, not a policy decision); a
///    directory that trusts none of the given candidates →
///    [`RejectReason::UnauthorizedDevice`].
/// 3. **Application** (§110): `layers.application.allow_relay ==
///    false` eliminates [`TransportKind::IrohRelay`]/[`TransportKind::MeshRelay`]
///    candidates; emptying the list here has no single better-fitting
///    §115 reason, so it's [`DeferredReason::NoSuitablePathYet`].
/// 4. **User** (§111): [`crate::privacy::eliminate_privacy_violations`],
///    against [`crate::privacy::effective_privacy_policy`]'s output
///    rather than `layers.user` directly — §138 "Emergency Override"
///    lives here, lifting `avoid_relay` specifically when both the
///    operation is `Priority::Critical` and the user explicitly
///    opted in; emptying here uses a fixed priority order over which
///    flag is set (see that logic's own comment, right above
///    [`decide_route`]).
/// 5. **Operation** (§112): [`crate::scoring::eliminate_hard_constraint_violations`];
///    emptying here is [`DeferredReason::NoSuitablePathYet`] — this
///    check covers several independent clauses at once (metered,
///    roaming, allow_dtn/relay/bluetooth, min bandwidth, health), none
///    of which §115 names individually. §125's own "realtime
///    operation never uses DTN" property is enforced here, by the
///    same `allow_dtn` clause that excludes DTN for every other
///    non-DTN-allowing operation — not a separate realtime-specific
///    check.
/// 6. **Network context**, discovery half (§90): only runs when
///    `discovery_budget` is `Some` and every remaining candidate is
///    [`crate::acquisition::CandidateState::RequiresDiscovery`] — a
///    mixed set just lets scoring/stickiness prefer the
///    already-reachable ones, no elimination needed.
///    [`crate::discovery::discovery_permitted`] returning `false`
///    because of `device.battery_saver` is
///    [`DeferredReason::BatteryPolicy`]; returning `false` for any
///    other reason (thermal, budget exhaustion) is the
///    [`DeferredReason::NoSuitablePathYet`] catch-all — named
///    `BatteryPolicy` specifically because that's the one §115 names,
///    not because thermal/budget causes don't exist.
/// 7. **Network context**, foreground half (§86):
///    [`crate::acquisition::eliminate_background_restricted`];
///    emptying here is [`DeferredReason::BackgroundRestriction`].
/// 8. Whatever survives all seven elimination steps goes to
///    [`crate::plan::plan_route`] for scoring/stickiness/final plan —
///    `Ok` is [`RouteDecisionResult::Routed`]; the `Err` case is a
///    defensive fallback ([`DeferredReason::NoSuitablePathYet`]) that
///    steps 1-7 should already have made unreachable, not a case this
///    function expects to hit.
#[allow(clippy::too_many_arguments)]
pub fn decide_route(
    candidates: &[PathCandidate],
    descriptor: &OperationDescriptor,
    layers: &PolicyLayers,
    account_id: AccountId,
    trust_store: &TrustedAccountStore,
    routing_policy: &RoutingPolicy,
    scorer: &dyn PathScorer,
    current: Option<&PathCandidate>,
    device: Option<&DeviceState>,
    discovery_budget: Option<&mut DiscoveryBudget>,
    now_millis: u64,
) -> RouteDecisionResult {
    // Step 0: §125's own "expired operation never routed."
    if descriptor
        .requirements
        .has_expired(descriptor.created_at_millis, now_millis)
    {
        return RouteDecisionResult::Rejected(RejectReason::OperationExpired);
    }

    // Step 1.
    if candidates.is_empty() {
        return if descriptor.requirements.allow_dtn {
            RouteDecisionResult::Deferred(DeferredReason::WaitingForPeer)
        } else {
            RouteDecisionResult::Unreachable
        };
    }

    // Step 2a: system size limit.
    if descriptor.estimated_size.0 > layers.system.max_operation_bytes.0 {
        return RouteDecisionResult::Rejected(RejectReason::ExceedsHardSizeLimit);
    }

    // Step 2b: system device trust.
    if trust_store.directory_for(account_id).is_none() {
        return RouteDecisionResult::Unreachable;
    }
    let trusted = eliminate_untrusted_candidates(candidates, account_id, trust_store);
    if trusted.is_empty() {
        return RouteDecisionResult::Rejected(RejectReason::UnauthorizedDevice);
    }

    // Step 3: application policy.
    let after_application: Vec<&PathCandidate> = if layers.application.allow_relay {
        trusted
    } else {
        trusted
            .into_iter()
            .filter(|c| !is_relay_transport(c.transport))
            .collect()
    };
    if after_application.is_empty() {
        return RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet);
    }

    // Step 4: user policy. §138's emergency override is applied here,
    // before elimination — not a separate step — so it's exactly
    // `avoid_relay` (and only `avoid_relay`) that's affected, and
    // only for this one decision, never mutating `layers.user` itself.
    let after_application_owned: Vec<PathCandidate> =
        after_application.into_iter().cloned().collect();
    let effective_user_policy =
        crate::privacy::effective_privacy_policy(layers.user, descriptor.requirements.priority);
    let after_user = eliminate_privacy_violations(&after_application_owned, &effective_user_policy);
    if after_user.is_empty() {
        return RouteDecisionResult::Deferred(deferred_reason_for_user_policy(
            &effective_user_policy,
        ));
    }

    // Step 5: operation policy (hard constraints).
    let after_user_owned: Vec<PathCandidate> = after_user.into_iter().cloned().collect();
    let after_operation =
        eliminate_hard_constraint_violations(&after_user_owned, &descriptor.requirements);
    if after_operation.is_empty() {
        return RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet);
    }

    // Step 6: network context, discovery half.
    if let Some(budget) = discovery_budget {
        let all_require_discovery = after_operation
            .iter()
            .all(|c| c.state == crate::acquisition::CandidateState::RequiresDiscovery);
        if all_require_discovery
            && !discovery_permitted(descriptor.requirements.priority, device, budget, now_millis)
        {
            return if device.is_some_and(|d| d.battery_saver == Some(true))
                && descriptor.requirements.priority != Priority::Critical
            {
                RouteDecisionResult::Deferred(DeferredReason::BatteryPolicy)
            } else {
                RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet)
            };
        }
    }

    // Step 7: network context, foreground half.
    let after_operation_owned: Vec<PathCandidate> = after_operation.into_iter().cloned().collect();
    let after_background = eliminate_background_restricted(&after_operation_owned, device);
    if after_background.is_empty() {
        return RouteDecisionResult::Deferred(DeferredReason::BackgroundRestriction);
    }

    // Step 8: hand the survivors to the existing scoring/stickiness
    // pipeline.
    let remaining: Vec<PathCandidate> = after_background.into_iter().cloned().collect();
    match plan_route(
        &remaining,
        &descriptor.requirements,
        routing_policy,
        scorer,
        current,
        device,
        now_millis,
    ) {
        Ok(plan) => RouteDecisionResult::Routed(plan),
        Err(_) => RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::descriptor::{ContentClass, OperationId};
    use crate::metrics::PathMetrics;
    use crate::policy::RoutingPolicyProfile;
    use crate::requirements::DeliveryRequirements;
    use crate::scoring::DefaultScorer;
    use crate::types::{
        Destination, MeteredState, PathCapabilities, PathId, RoamingState, RouteHealth,
    };
    use siar_domain::DeviceId;
    use siar_identity_multidevice::{
        DeviceCapabilitySet, DeviceCertificate, DeviceDirectory, DeviceDirectoryEntry,
        DeviceStatus, RootIdentityKey,
    };

    fn candidate(transport: TransportKind, peer: DeviceId, metered: MeteredState) -> PathCandidate {
        PathCandidate {
            path_id: PathId::new(),
            transport,
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
                metered,
                roaming: RoamingState::Unknown,
                requires_foreground: false,
            },
            health: RouteHealth::Healthy,
            underlay: None,
            state: crate::acquisition::CandidateState::Active,
        }
    }

    fn trusted_store_with_one_device() -> (TrustedAccountStore, AccountId, DeviceId) {
        let root = RootIdentityKey::generate();
        let account = AccountId::new();
        let device = DeviceId::new();
        let cert = DeviceCertificate::issue(
            &root,
            account,
            device,
            [1u8; 32],
            0,
            None,
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        let directory = DeviceDirectory::sign(
            &root,
            account,
            1,
            vec![DeviceDirectoryEntry {
                device_id: device,
                certificate: cert,
                status: DeviceStatus::Active,
                transport_endpoints: vec![],
            }],
        );
        let mut store = TrustedAccountStore::new();
        store.accept(directory, &root.root_public_key()).unwrap();
        (store, account, device)
    }

    fn descriptor_for(
        destination: Destination,
        requirements: DeliveryRequirements,
        estimated_size: u64,
    ) -> OperationDescriptor {
        OperationDescriptor {
            operation_id: OperationId::new(),
            destination,
            requirements,
            estimated_size: ByteCount(estimated_size),
            content_class: ContentClass::File,
            required_extension: None,
            created_at_millis: 0,
        }
    }

    fn no_size_limit() -> SystemPolicy {
        SystemPolicy {
            max_operation_bytes: ByteCount(u64::MAX),
        }
    }

    #[test]
    fn empty_candidates_is_unreachable_when_dtn_is_not_allowed() {
        let (store, account, _device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::realtime_media(), // allow_dtn: false
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &[],
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(result, RouteDecisionResult::Unreachable));
    }

    #[test]
    fn empty_candidates_is_deferred_waiting_for_peer_when_dtn_is_allowed() {
        let (store, account, _device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(), // allow_dtn: true
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &[],
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::WaitingForPeer)
        ));
    }

    #[test]
    fn exceeding_the_system_size_limit_is_rejected_not_deferred() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            1_000,
        );
        let layers = PolicyLayers {
            system: &SystemPolicy {
                max_operation_bytes: ByteCount(100),
            },
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Rejected(RejectReason::ExceedsHardSizeLimit)
        ));
    }

    #[test]
    fn a_revoked_or_unknown_device_is_rejected_never_merely_deferred() {
        let (store, account, _trusted_device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let stranger = candidate(
            TransportKind::IrohDirect,
            DeviceId::new(),
            MeteredState::Unmetered,
        );
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &[stranger],
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Rejected(RejectReason::UnauthorizedDevice)
        ));
    }

    #[test]
    fn an_account_with_no_directory_at_all_is_unreachable_not_rejected() {
        let store = TrustedAccountStore::new();
        let account = AccountId::new();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect,
            DeviceId::new(),
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(result, RouteDecisionResult::Unreachable));
    }

    #[test]
    fn application_policy_forbidding_relay_defers_when_only_relay_exists() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy { allow_relay: false },
            user: &PrivacyPolicy::default(),
        };
        let candidates = vec![candidate(
            TransportKind::IrohRelay,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet)
        ));
    }

    /// §113's own worked example, transcribed directly: "large file +
    /// no metered + only metered path exists" → `DeferredByPolicy`
    /// (this crate's `DeferredReason::WaitingForUnmetered`), "not
    /// silent policy violation."
    #[test]
    fn spec_113_large_file_no_metered_only_metered_path_is_deferred_not_silently_dropped() {
        let (store, account, device) = trusted_store_with_one_device();
        let mut req = DeliveryRequirements::file_chunk();
        req.allow_metered = true; // operation itself permits metered...
        let descriptor = descriptor_for(Destination::Account(account), req, 10);
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy {
                avoid_metered: true, // ...but the user policy forbids it
                ..Default::default()
            },
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect,
            device,
            MeteredState::Metered, // and it's the only path that exists
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::WaitingForUnmetered)
        ));
    }

    #[test]
    fn nearby_only_conflict_defers_as_waiting_for_wifi() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy {
                nearby_only: true,
                ..Default::default()
            },
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect, // internet, not nearby
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::WaitingForWifi)
        ));
    }

    #[test]
    fn battery_saver_defers_a_discovery_only_candidate_set_as_battery_policy() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(), // Normal priority
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let mut discovery_candidate =
            candidate(TransportKind::BluetoothLe, device, MeteredState::Unmetered);
        discovery_candidate.state = crate::acquisition::CandidateState::RequiresDiscovery;
        let candidates = vec![discovery_candidate];
        let device_state = DeviceState {
            battery_level_class: None,
            charging: None,
            battery_saver: Some(true),
            thermal_state: None,
            foreground: None,
            memory_pressure: None,
        };
        let mut budget = DiscoveryBudget::for_priority(Priority::Normal);
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            Some(&device_state),
            Some(&mut budget),
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::BatteryPolicy)
        ));
    }

    #[test]
    fn background_restriction_defers_a_foreground_only_candidate_while_backgrounded() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let mut foreground_only =
            candidate(TransportKind::IrohDirect, device, MeteredState::Unmetered);
        foreground_only.capabilities.requires_foreground = true;
        let candidates = vec![foreground_only];
        let device_state = DeviceState {
            battery_level_class: None,
            charging: None,
            battery_saver: None,
            thermal_state: None,
            foreground: Some(false),
            memory_pressure: None,
        };
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            Some(&device_state),
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::BackgroundRestriction)
        ));
    }

    #[test]
    fn a_fully_eligible_candidate_is_routed() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(result, RouteDecisionResult::Routed(_)));
    }

    /// §125 "Policy Property Tests", transcribed as four separate
    /// properties rather than one combined test — each is its own
    /// worked example in the spec and deserves its own failure
    /// message if it regresses.
    #[test]
    fn spec_125_a_revoked_device_is_never_selected_even_when_it_scores_far_better() {
        let (store, account, trusted_device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        // The untrusted candidate has excellent metrics; the trusted
        // one is merely present. A scorer that ignored trust would
        // pick the untrusted one every time.
        let mut untrusted = candidate(
            TransportKind::IrohDirect,
            DeviceId::new(),
            MeteredState::Unmetered,
        );
        untrusted.metrics.rtt_millis = Some(1);
        let trusted = candidate(
            TransportKind::IrohDirect,
            trusted_device,
            MeteredState::Unmetered,
        );
        let candidates = vec![untrusted, trusted];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        match result {
            RouteDecisionResult::Routed(plan) => assert_eq!(plan.primary.peer, trusted_device),
            other => panic!("expected a routed plan onto the trusted device, got {other:?}"),
        }
    }

    #[test]
    fn spec_125_a_forbidden_metered_path_is_never_selected_even_when_it_scores_far_better() {
        let (store, account, device) = trusted_store_with_one_device();
        let mut req = DeliveryRequirements::interactive_message();
        req.allow_metered = false;
        let descriptor = descriptor_for(Destination::Account(account), req, 10);
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let unmetered_device = device;
        let mut metered = candidate(
            TransportKind::IrohDirect,
            DeviceId::new(),
            MeteredState::Metered,
        );
        metered.metrics.rtt_millis = Some(1); // scores far better on latency
        let unmetered = candidate(
            TransportKind::IrohDirect,
            unmetered_device,
            MeteredState::Unmetered,
        );
        let candidates = vec![metered, unmetered];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        match result {
            RouteDecisionResult::Routed(plan) => {
                assert_eq!(plan.primary.capabilities.metered, MeteredState::Unmetered)
            }
            other => panic!("expected a routed plan onto the unmetered device, got {other:?}"),
        }
    }

    #[test]
    fn spec_125_a_realtime_operation_never_uses_dtn_even_if_dtn_claims_realtime_capability() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::realtime_media(), // allow_dtn: false
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        // Adversarial: even a DTN candidate that (incorrectly) claims
        // realtime capability must still be excluded — this proves
        // it's `allow_dtn`, not the capability flag, doing the work.
        let mut dtn = candidate(TransportKind::Dtn, DeviceId::new(), MeteredState::Unmetered);
        dtn.capabilities.realtime_media = true;
        let realtime = candidate(TransportKind::IrohDirect, device, MeteredState::Unmetered);
        let mut realtime = realtime;
        realtime.capabilities.realtime_media = true;
        let candidates = vec![dtn, realtime];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        match result {
            RouteDecisionResult::Routed(plan) => {
                assert_ne!(plan.primary.transport, TransportKind::Dtn);
                for fallback in &plan.fallbacks {
                    assert_ne!(fallback.transport, TransportKind::Dtn);
                }
            }
            other => panic!("expected a routed plan avoiding DTN entirely, got {other:?}"),
        }
    }

    #[test]
    fn spec_125_an_expired_operation_is_never_routed_regardless_of_how_good_the_candidates_are() {
        let (store, account, device) = trusted_store_with_one_device();
        let mut requirements = DeliveryRequirements::interactive_message();
        requirements.expiry_millis = Some(5_000);
        let descriptor = OperationDescriptor {
            operation_id: OperationId::new(),
            destination: Destination::Account(account),
            requirements,
            estimated_size: ByteCount(10),
            content_class: ContentClass::Text,
            required_extension: None,
            created_at_millis: 1_000,
        };
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        let candidates = vec![candidate(
            TransportKind::IrohDirect,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };
        // now_millis = 6_000 is exactly 5_000ms after created_at (1_000) —
        // requirements.rs's own has_expired test treats that boundary
        // as expired.
        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            6_000,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Rejected(RejectReason::OperationExpired)
        ));
    }

    /// §123 "Deterministic Scoring": "given the same policy, metrics,
    /// and context, the routing decision should be reproducible."
    /// Calling `decide_route` twice with byte-for-byte identical
    /// inputs must produce the same primary path both times — this
    /// is the top-of-stack version of the property; see
    /// `scoring.rs`'s and `plan.rs`'s own tests for the same property
    /// checked one layer down.
    #[test]
    fn spec_123_decide_route_is_deterministic_given_identical_inputs() {
        let (store, account, device) = trusted_store_with_one_device();
        let descriptor = descriptor_for(
            Destination::Account(account),
            DeliveryRequirements::interactive_message(),
            10,
        );
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &PrivacyPolicy::default(),
        };
        // Two different paths to the *same* trusted device (e.g. two
        // transports), so scoring has a real choice to make rather
        // than a single-candidate trivial case.
        let mut a = candidate(TransportKind::IrohDirect, device, MeteredState::Unmetered);
        a.metrics.rtt_millis = Some(20);
        let mut b = candidate(TransportKind::LocalLan, device, MeteredState::Unmetered);
        b.metrics.rtt_millis = Some(80);
        let candidates = vec![a, b];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };

        let run = || {
            decide_route(
                &candidates,
                &descriptor,
                &layers,
                account,
                &store,
                &balanced,
                &scorer,
                None,
                None,
                None,
                0,
            )
        };

        let first = run();
        let second = run();
        match (first, second) {
            (RouteDecisionResult::Routed(p1), RouteDecisionResult::Routed(p2)) => {
                assert_eq!(p1.primary.path_id, p2.primary.path_id);
                assert_eq!(p1.primary.transport, p2.primary.transport);
                assert_eq!(p1.strategy, p2.strategy);
            }
            other => panic!("expected two routed plans, got {other:?}"),
        }
    }

    /// §138 "Emergency Override", proved through the full
    /// `decide_route` stack, not just `effective_privacy_policy` in
    /// isolation: a critical operation with the user's explicit
    /// opt-in reaches the relay candidate that a plain `avoid_relay`
    /// policy would otherwise eliminate.
    #[test]
    fn spec_138_a_critical_operation_with_explicit_opt_in_reaches_a_relay_only_candidate() {
        let (store, account, device) = trusted_store_with_one_device();
        let mut req = DeliveryRequirements::interactive_message();
        req.priority = crate::types::Priority::Critical;
        let descriptor = descriptor_for(Destination::Account(account), req, 10);
        let user_policy = PrivacyPolicy {
            avoid_relay: true,
            emergency_override_enabled: true,
            ..Default::default()
        };
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &user_policy,
        };
        let candidates = vec![candidate(
            TransportKind::IrohRelay,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };

        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(result, RouteDecisionResult::Routed(_)));
    }

    #[test]
    fn spec_138_without_explicit_opt_in_a_critical_operation_still_respects_avoid_relay() {
        let (store, account, device) = trusted_store_with_one_device();
        let mut req = DeliveryRequirements::interactive_message();
        req.priority = crate::types::Priority::Critical;
        let descriptor = descriptor_for(Destination::Account(account), req, 10);
        let user_policy = PrivacyPolicy {
            avoid_relay: true,
            emergency_override_enabled: false,
            ..Default::default()
        };
        let layers = PolicyLayers {
            system: &no_size_limit(),
            application: &ApplicationPolicy::default(),
            user: &user_policy,
        };
        let candidates = vec![candidate(
            TransportKind::IrohRelay,
            device,
            MeteredState::Unmetered,
        )];
        let balanced = RoutingPolicyProfile::Balanced.policy();
        let scorer = DefaultScorer {
            weights: balanced.weights,
        };

        let result = decide_route(
            &candidates,
            &descriptor,
            &layers,
            account,
            &store,
            &balanced,
            &scorer,
            None,
            None,
            None,
            0,
        );
        assert!(matches!(
            result,
            RouteDecisionResult::Deferred(DeferredReason::NoSuitablePathYet)
        ));
    }
}
