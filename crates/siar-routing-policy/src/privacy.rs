//! §49 "Privacy Policy", §50 "Direct vs Relay Preference", §52 "Wi-Fi
//! Direct/Aware" threshold policy.
//!
//! §51 "LAN Preference", §53/§54 "Bluetooth Classic"/"Bluetooth LE",
//! §55 "Mesh Forwarding", §56 "DTN Routing Boundary" are read and
//! accounted for in this module's doc comments (below) rather than
//! each getting its own function — §51's preference already falls out
//! of [`crate::setup::static_setup_cost`] rating LAN [`crate::setup::SetupCost::Cheap`]
//! plus LAN's typical [`crate::types::RouteHealth::Healthy`] rating,
//! not a separately-weighted term; §53/§54's throughput distinction is
//! exactly [`crate::setup::static_setup_cost`]'s `BluetoothClassic`
//! (Expensive, pairing) vs `BluetoothLe` (Moderate, lighter GATT
//! connect) split plus whatever [`crate::types::PathCapabilities`]
//! flags a caller honestly sets per candidate — no *additional* hard
//! constraint is added here beyond [`eliminate_unjustified_expensive_setup`]
//! (which already covers `BluetoothClassic` alongside the Wi-Fi
//! variants); §56's three named routing-side questions ("DTN allowed?
//! priority? expiry? replication budget?") are now all real fields on
//! [`crate::requirements::DeliveryRequirements`]
//! (`allow_dtn`/`priority`/`expiry_millis`/`dtn_replication_budget`)
//! — nothing further to add here, and actual DTN peer-selection stays
//! out of scope per this crate's own top doc comment on Phase 5.
//!
//! §55 "Mesh Forwarding" is the one honest gap in this list: the spec
//! wants a mesh candidate to carry next-hop, estimated route utility,
//! hop budget, and relay trust policy as distinct fields, not just
//! compete as an ordinary [`crate::candidate::PathCandidate`] under
//! `TransportKind::MeshRelay`. That richer representation doesn't
//! exist yet — named here rather than forced into a shallow stand-in.

use serde::{Deserialize, Serialize};

use crate::candidate::PathCandidate;
use crate::requirements::DeliveryRequirements;
use crate::types::{DeliveryClass, MeteredState, Priority, TransportKind};

/// §49: "Privacy policy should be explicit: prefer direct, avoid
/// relay, avoid metered, [effectively-]nearby-only, internet-only. Do
/// not make privacy decisions implicit." Every field defaults to
/// `false` — an all-`false` policy imposes no restriction at all,
/// matching how every other optional policy knob in this crate
/// (`DeliveryRequirements`'s own `allow_*` fields) defaults to
/// permissive rather than restrictive. `Serialize`/`Deserialize`
/// added this round for §179 "Route Policy Persistence"'s own
/// "persist user/application settings" — this crate has no
/// persistence layer of its own (it never reads or writes a file),
/// so the actual save/load is a caller's job; the derive just makes
/// that job possible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyPolicy {
    pub prefer_direct: bool,
    pub avoid_relay: bool,
    pub avoid_metered: bool,
    pub nearby_only: bool,
    pub internet_only: bool,
    /// §138 "Emergency Override": the user's own opt-in, checked by
    /// [`effective_privacy_policy`] — *not* a flag routing sets for
    /// itself. Defaults `false`, so an application that never
    /// surfaces this setting to the user gets the same behavior as
    /// before this field existed: `avoid_relay` always holds, even
    /// for a `Priority::Critical` operation.
    pub emergency_override_enabled: bool,
}

fn is_nearby_transport(t: TransportKind) -> bool {
    matches!(
        t,
        TransportKind::LocalLan
            | TransportKind::WifiDirect
            | TransportKind::WifiAware
            | TransportKind::BluetoothClassic
            | TransportKind::BluetoothLe
            | TransportKind::MeshRelay
    )
}

fn is_internet_transport(t: TransportKind) -> bool {
    matches!(t, TransportKind::IrohDirect | TransportKind::IrohRelay)
}

/// §49's four hard clauses (`prefer_direct` is soft — see
/// [`direct_preference_bonus`] — so it has no place in a hard-constraint
/// check, same reasoning [`crate::scoring::passes_hard_constraints`]'s
/// own doc comment already gives for keeping soft preferences out of
/// that function). `avoid_metered` treats [`MeteredState::Unknown`]
/// the same as [`MeteredState::Metered`] — only a candidate the
/// platform has actually confirmed [`MeteredState::Unmetered`] passes,
/// matching [`crate::scoring::passes_hard_constraints`]'s own
/// conservative-default reasoning for the same enum.
pub fn passes_privacy_policy(candidate: &PathCandidate, policy: &PrivacyPolicy) -> bool {
    if policy.avoid_relay && candidate.transport == TransportKind::IrohRelay {
        return false;
    }
    if policy.avoid_metered && candidate.capabilities.metered != MeteredState::Unmetered {
        return false;
    }
    if policy.nearby_only && !is_nearby_transport(candidate.transport) {
        return false;
    }
    if policy.internet_only && !is_internet_transport(candidate.transport) {
        return false;
    }
    true
}

/// §138 "Emergency Override": "may override user preference to avoid
/// relay, only if the user explicitly enabled such emergency
/// behavior... do not silently violate user privacy choices." Both
/// conditions are checked, not just the priority: a `Critical`
/// operation with `emergency_override_enabled: false` still respects
/// `avoid_relay` exactly as written — the "explicitly enabled" half
/// is what keeps this from being routing quietly deciding an
/// emergency justifies overriding privacy on the user's behalf. Only
/// `avoid_relay` is overridden — `avoid_metered`/`nearby_only`/
/// `internet_only` are untouched, since the spec's own text names
/// relay specifically and nothing else.
pub fn effective_privacy_policy(
    policy: &PrivacyPolicy,
    priority: crate::types::Priority,
) -> PrivacyPolicy {
    if policy.emergency_override_enabled && priority == crate::types::Priority::Critical {
        PrivacyPolicy {
            avoid_relay: false,
            ..*policy
        }
    } else {
        *policy
    }
}

/// §49 applied to a whole list — same composable shape as
/// [`crate::security::eliminate_untrusted_candidates`]: a caller with
/// a `PrivacyPolicy` on hand calls this (typically alongside that
/// function and [`crate::scoring::eliminate_hard_constraint_violations`]);
/// a caller with no privacy restrictions configured just doesn't,
/// since the all-default `PrivacyPolicy` would keep every candidate
/// anyway.
pub fn eliminate_privacy_violations<'a>(
    candidates: &'a [PathCandidate],
    policy: &PrivacyPolicy,
) -> Vec<&'a PathCandidate> {
    candidates
        .iter()
        .filter(|c| passes_privacy_policy(c, policy))
        .collect()
}

/// §50 "Direct vs Relay Preference": `prefer_direct`'s soft half — a
/// bonus [`crate::scoring::DefaultScorer`] doesn't fold in on its own
/// (this crate's general policy-composition seam, same as
/// [`eliminate_privacy_violations`]/[`crate::security::eliminate_untrusted_candidates`]:
/// a caller wanting this folded into scoring adds it to whatever
/// `RouteScore` it already computed, rather than this module reaching
/// into [`crate::policy::PolicyWeights`] to add yet another field for
/// something that, unlike §44/§45, isn't part of §24's own formula).
/// Neutral (`0.5`) when `prefer_direct` is off, so a caller can always
/// add this in without it silently becoming a no-preference-vs-active-
/// preference asymmetry.
pub fn direct_preference_bonus(transport: TransportKind, policy: &PrivacyPolicy) -> f64 {
    if !policy.prefer_direct {
        return 0.5;
    }
    if transport == TransportKind::IrohRelay {
        0.0
    } else {
        1.0
    }
}

/// §52: "Wi-Fi Direct/Aware group creation should not happen for
/// every tiny message. Use threshold policy: large transfer, active
/// call, explicit nearby session." Three clauses, each mapped to a
/// real field: "active call" is
/// [`DeliveryClass::Realtime`]; "large transfer" is a `min_bandwidth`
/// floor (chosen as ≥1 Mbps — high enough that no ordinary text/voice-
/// note message crosses it, low enough that an actual bulk transfer
/// does); "explicit nearby session" is the
/// [`DeliveryRequirements::nearby_session_explicit`] flag added this
/// round specifically because this clause had nothing else to point
/// to. `Priority::Critical` is added as a fourth, spec-adjacent
/// clause: §33's own Emergency policy text ("increase discovery… allow
/// redundancy") already treats emergency traffic as the one case
/// that's allowed to pay expensive setup cost freely — see
/// [`crate::policy::RoutingPolicyProfile::Emergency`]'s `setup_cost`
/// weight, deliberately the lowest of any profile.
const LARGE_TRANSFER_BITRATE_BPS: u64 = 1_000_000;

pub fn justifies_expensive_setup(req: &DeliveryRequirements) -> bool {
    req.class == DeliveryClass::Realtime
        || req
            .min_bandwidth
            .is_some_and(|b| b.0 >= LARGE_TRANSFER_BITRATE_BPS)
        || req.nearby_session_explicit
        || req.priority == Priority::Critical
}

/// §52's threshold as a hard elimination over `WifiDirect`/`WifiAware`/
/// `BluetoothClassic` (§53's pairing cost is the same "don't pay this
/// for a tiny message" concern §52 names for Wi-Fi) — candidates on
/// any other transport are untouched.
pub fn eliminate_unjustified_expensive_setup<'a>(
    candidates: &'a [PathCandidate],
    req: &DeliveryRequirements,
) -> Vec<&'a PathCandidate> {
    if justifies_expensive_setup(req) {
        return candidates.iter().collect();
    }
    candidates
        .iter()
        .filter(|c| {
            !matches!(
                c.transport,
                TransportKind::WifiDirect
                    | TransportKind::WifiAware
                    | TransportKind::BluetoothClassic
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate::TransportEndpoint;
    use crate::metrics::{Bitrate, PathMetrics};
    use crate::requirements::DeliveryRequirements;
    use crate::types::{MeteredState, PathCapabilities, PathId, RouteHealth};
    use siar_domain::DeviceId;

    fn candidate(transport: TransportKind, metered: MeteredState) -> PathCandidate {
        PathCandidate {
            path_id: PathId::new(),
            transport,
            peer: DeviceId::new(),
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
                roaming: crate::types::RoamingState::Unknown,
                requires_foreground: false,
            },
            health: RouteHealth::Healthy,
            underlay: None,
            state: crate::acquisition::CandidateState::Active,
        }
    }

    #[test]
    fn avoid_relay_eliminates_only_iroh_relay() {
        let policy = PrivacyPolicy {
            avoid_relay: true,
            ..Default::default()
        };
        assert!(!passes_privacy_policy(
            &candidate(TransportKind::IrohRelay, MeteredState::Unmetered),
            &policy
        ));
        assert!(passes_privacy_policy(
            &candidate(TransportKind::IrohDirect, MeteredState::Unmetered),
            &policy
        ));
    }

    #[test]
    fn nearby_only_eliminates_internet_transports() {
        let policy = PrivacyPolicy {
            nearby_only: true,
            ..Default::default()
        };
        assert!(!passes_privacy_policy(
            &candidate(TransportKind::IrohDirect, MeteredState::Unmetered),
            &policy
        ));
        assert!(passes_privacy_policy(
            &candidate(TransportKind::LocalLan, MeteredState::Unmetered),
            &policy
        ));
    }

    #[test]
    fn internet_only_eliminates_nearby_transports() {
        let policy = PrivacyPolicy {
            internet_only: true,
            ..Default::default()
        };
        assert!(!passes_privacy_policy(
            &candidate(TransportKind::WifiDirect, MeteredState::Unmetered),
            &policy
        ));
        assert!(passes_privacy_policy(
            &candidate(TransportKind::IrohRelay, MeteredState::Unmetered),
            &policy
        ));
    }

    #[test]
    fn avoid_metered_reads_the_candidates_own_capability_flag() {
        let policy = PrivacyPolicy {
            avoid_metered: true,
            ..Default::default()
        };
        assert!(!passes_privacy_policy(
            &candidate(TransportKind::IrohDirect, MeteredState::Metered),
            &policy
        ));
    }

    #[test]
    fn a_default_policy_restricts_nothing() {
        let policy = PrivacyPolicy::default();
        for t in [
            TransportKind::IrohDirect,
            TransportKind::IrohRelay,
            TransportKind::WifiDirect,
            TransportKind::Dtn,
        ] {
            assert!(passes_privacy_policy(
                &candidate(t, MeteredState::Metered),
                &policy
            ));
        }
    }

    #[test]
    fn an_ordinary_text_message_does_not_justify_wifi_direct_setup() {
        let req = DeliveryRequirements::interactive_message();
        assert!(!justifies_expensive_setup(&req));
        let candidates = vec![
            candidate(TransportKind::WifiDirect, MeteredState::Unmetered),
            candidate(TransportKind::IrohDirect, MeteredState::Unmetered),
        ];
        let kept = eliminate_unjustified_expensive_setup(&candidates, &req);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].transport, TransportKind::IrohDirect);
    }

    #[test]
    fn an_active_call_justifies_wifi_direct_setup() {
        let req = DeliveryRequirements::realtime_media();
        assert!(justifies_expensive_setup(&req));
        let candidates = vec![candidate(
            TransportKind::WifiDirect,
            MeteredState::Unmetered,
        )];
        assert_eq!(
            eliminate_unjustified_expensive_setup(&candidates, &req).len(),
            1
        );
    }

    #[test]
    fn a_large_transfer_min_bandwidth_alone_justifies_expensive_setup() {
        let mut req = DeliveryRequirements::interactive_message();
        req.min_bandwidth = Some(Bitrate(2_000_000));
        assert!(justifies_expensive_setup(&req));
    }

    #[test]
    fn an_explicit_nearby_session_request_justifies_expensive_setup() {
        let mut req = DeliveryRequirements::interactive_message();
        req.nearby_session_explicit = true;
        assert!(justifies_expensive_setup(&req));
    }

    #[test]
    fn prefer_direct_bonus_is_neutral_when_the_policy_does_not_ask_for_it() {
        let policy = PrivacyPolicy::default();
        assert_eq!(
            direct_preference_bonus(TransportKind::IrohRelay, &policy),
            0.5
        );
    }

    #[test]
    fn prefer_direct_bonus_penalizes_relay_and_rewards_everything_else() {
        let policy = PrivacyPolicy {
            prefer_direct: true,
            ..Default::default()
        };
        assert_eq!(
            direct_preference_bonus(TransportKind::IrohRelay, &policy),
            0.0
        );
        assert_eq!(
            direct_preference_bonus(TransportKind::IrohDirect, &policy),
            1.0
        );
    }

    /// §138 "Emergency Override", both of its own guard conditions.
    #[test]
    fn spec_138_emergency_override_requires_both_explicit_opt_in_and_critical_priority() {
        let opted_in = PrivacyPolicy {
            avoid_relay: true,
            emergency_override_enabled: true,
            ..Default::default()
        };
        assert!(
            !effective_privacy_policy(&opted_in, crate::types::Priority::Critical).avoid_relay,
            "opted in + critical should lift avoid_relay"
        );
        assert!(
            effective_privacy_policy(&opted_in, crate::types::Priority::Normal).avoid_relay,
            "opted in but not critical must still respect avoid_relay"
        );

        let not_opted_in = PrivacyPolicy {
            avoid_relay: true,
            emergency_override_enabled: false,
            ..Default::default()
        };
        assert!(
            effective_privacy_policy(&not_opted_in, crate::types::Priority::Critical).avoid_relay,
            "critical alone, without explicit opt-in, must never override — no silent privacy violation"
        );
    }

    #[test]
    fn spec_138_the_override_touches_only_avoid_relay() {
        let policy = PrivacyPolicy {
            avoid_relay: true,
            avoid_metered: true,
            nearby_only: true,
            emergency_override_enabled: true,
            ..Default::default()
        };
        let effective = effective_privacy_policy(&policy, crate::types::Priority::Critical);
        assert!(!effective.avoid_relay);
        assert!(effective.avoid_metered);
        assert!(effective.nearby_only);
    }
}
