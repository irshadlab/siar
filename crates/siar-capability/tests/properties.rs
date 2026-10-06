//! Property tests for the §57-60, 75-76, 84-86, 97-98, 104-107 modules.

use std::collections::BTreeSet;

use proptest::prelude::*;

use siar_capability::cache::{CapabilityCache, StalenessPolicy};
use siar_capability::descriptor::{
    CapabilityDescriptor, CapabilityParameters, CapabilityRequirement,
};
use siar_capability::direction::{
    cross_negotiate, directional_descriptor, negotiate_directional, CapabilityDirection,
};
use siar_capability::epoch::{CapabilityEpoch, PeerCapabilityView};
use siar_capability::error::CapabilityNegotiationError as E;
use siar_capability::failure::{classify, FailureAction, NegotiationStage};
use siar_capability::hash::CapabilitySetHash;
use siar_capability::id::{CapabilityId, CapabilityNamespace as Ns};
use siar_capability::lazy::{
    CoreAdvertisement, ExtensionState, ExtensionSummary, HierarchicalSession,
};
use siar_capability::policy::CapabilityPolicy;
use siar_capability::registry::{
    CapabilityDefinition, CapabilityRegistry, ParameterSchema, SecurityClass,
};
use siar_capability::resume::{decide, ResumeDecision, ResumeInputs, ResumePolicy};
use siar_capability::set::CapabilitySet;
use siar_capability::trust::{may_rely_on, CapabilitySource, Purpose};
use siar_capability::ui_adapter::{
    availability, PeerKnowledge, UiAvailability, UiFeature, UiFeatureMap,
};
use siar_capability::version::CapabilityVersion;

const V1: CapabilityVersion = CapabilityVersion::new(1, 0);

fn dir(i: u8) -> CapabilityDirection {
    [
        CapabilityDirection::Send,
        CapabilityDirection::Receive,
        CapabilityDirection::Both,
    ][usize::from(i) % 3]
}

proptest! {
    /// Oracle: re-derive the cross rule from first principles.
    #[test]
    fn cross_negotiation_matches_the_send_receive_pairing_oracle(l in 0u8..3, r in 0u8..3, lminor in 0u16..5, rminor in 0u16..5, rmajor in 1u16..3) {
        let (ld, rd) = (dir(l), dir(r));
        let local = directional_descriptor(CapabilityId::new(Ns::Media, 1), CapabilityVersion::new(1, lminor), CapabilityRequirement::Optional, ld);
        let remote = directional_descriptor(CapabilityId::new(Ns::Media, 1), CapabilityVersion::new(rmajor, rminor), CapabilityRequirement::Optional, rd);
        let out = cross_negotiate(&local, &remote).unwrap();
        if rmajor != 1 {
            prop_assert_eq!(out, None);
        } else {
            let (send, recv) = (ld.can_send() && rd.can_receive(), ld.can_receive() && rd.can_send());
            match out {
                None => prop_assert!(!send && !recv),
                Some(o) => {
                    prop_assert_eq!((o.we_send, o.we_receive), (send, recv));
                    prop_assert_eq!(o.version, CapabilityVersion::new(1, lminor.min(rminor)));
                }
            }
        }
    }

    /// Whatever the sets, directional ids never appear in the general
    /// output, and every directional outcome is a real pairing.
    #[test]
    fn directional_split_is_clean(l in prop::collection::vec((0u32..4, 0u8..3), 0..4), r in prop::collection::vec((0u32..4, 0u8..3), 0..4)) {
        let mut reg = CapabilityRegistry::new();
        let ids: BTreeSet<CapabilityId> = (0..4).map(|c| CapabilityId::new(Ns::Media, c)).collect();
        for id in &ids {
            reg.register(CapabilityDefinition { id: *id, name: "x", max_version: V1, parameter_schema: ParameterSchema::BitSet, security_class: SecurityClass::Functional });
        }
        let build = |v: &[(u32, u8)]| {
            let mut s = CapabilitySet::new();
            for (c, d) in v {
                let id = CapabilityId::new(Ns::Media, *c);
                if !s.contains(&id) { s.insert(directional_descriptor(id, V1, CapabilityRequirement::Optional, dir(*d))).unwrap(); }
            }
            s
        };
        let (a, b) = (build(&l), build(&r));
        let n = negotiate_directional(&a, &b, &reg, &CapabilityPolicy::new(), &ids).unwrap();
        prop_assert!(n.general.is_empty());
        for (id, o) in &n.directional {
            prop_assert!(a.contains(id) && b.contains(id));
            prop_assert!(o.we_send || o.we_receive);
        }
        // mirror: swapping the sides swaps send/receive for every shared id
        let m = negotiate_directional(&b, &a, &reg, &CapabilityPolicy::new(), &ids).unwrap();
        prop_assert_eq!(n.directional.keys().collect::<Vec<_>>(), m.directional.keys().collect::<Vec<_>>());
        for (id, o) in &n.directional {
            prop_assert_eq!((o.we_send, o.we_receive), (m.directional[id].we_receive, m.directional[id].we_send));
        }
    }

    /// UI availability for a negotiated set equals a brute-force check;
    /// an unknown peer is never Unavailable for a mapped feature.
    #[test]
    fn ui_availability_matches_a_reference_implementation(
        have in prop::collection::vec((0u32..6, 0u16..4), 0..6),
        need in prop::collection::vec((0u32..6, 0u16..4), 1..4),
    ) {
        let mut set = CapabilitySet::new();
        for (c, minor) in &have {
            let id = CapabilityId::new(Ns::Files, *c);
            if !set.contains(&id) { set.insert(CapabilityDescriptor::new(id, CapabilityVersion::new(1, *minor), CapabilityRequirement::Optional, CapabilityParameters::None)).unwrap(); }
        }
        let mut map = UiFeatureMap::new();
        for (c, minor) in &need { map.require(UiFeature::SendFile, CapabilityId::new(Ns::Files, *c), CapabilityVersion::new(1, *minor)); }
        let expected = need.iter().all(|(c, m)| set.get(&CapabilityId::new(Ns::Files, *c)).is_some_and(|d| d.version >= CapabilityVersion::new(1, *m)));
        let got = availability(&map, UiFeature::SendFile, PeerKnowledge::Negotiated(&set));
        prop_assert_eq!(got == UiAvailability::Available, expected);
        prop_assert_eq!(got == UiAvailability::Unavailable, !expected);
        prop_assert_eq!(availability(&map, UiFeature::SendFile, PeerKnowledge::Unknown), UiAvailability::MayWork);
    }

    /// A cached view may only disable an action on a *fresh* absence.
    #[test]
    fn cache_disables_only_on_fresh_absence(age in 0u64..(3 * 24 * 3_600_000), has in any::<bool>()) {
        let id = CapabilityId::new(Ns::Files, 1);
        let mut s = CapabilitySet::new();
        if has { s.insert(CapabilityDescriptor::new(id, V1, CapabilityRequirement::Optional, CapabilityParameters::None)).unwrap(); }
        let mut cache = CapabilityCache::new(4, StalenessPolicy::default());
        let peer = [1u8; 32];
        cache.put(peer, PeerCapabilityView::from_snapshot(CapabilityEpoch(1), s), 0);
        let mut map = UiFeatureMap::new();
        map.require(UiFeature::SendFile, id, V1);
        let a = availability(&map, UiFeature::SendFile, PeerKnowledge::Cached { cache: &cache, peer: &peer, now_millis: age });
        let fresh = age < StalenessPolicy::default().fresh_for_millis;
        if a == UiAvailability::Unavailable { prop_assert!(fresh && !has); }
        if fresh && has { prop_assert_eq!(a, UiAvailability::Available); }
    }

    /// Session state machine under arbitrary operation sequences.
    #[test]
    fn hierarchical_session_invariants(ops in prop::collection::vec((0u8..5, 0u8..4, 0u8..8), 1..80)) {
        let nss = [Ns::Files, Ns::Media, Ns::Dtn, Ns::Presence];
        let adv = |list: &[Ns]| CoreAdvertisement::new(list.iter().map(|n| ExtensionSummary { namespace: *n, version: V1 }).collect()).unwrap();
        let mut s = HierarchicalSession::establish(&adv(&[Ns::Files, Ns::Media, Ns::Dtn]), &adv(&[Ns::Files, Ns::Media, Ns::Presence]));
        let supported: BTreeSet<Ns> = s.supported_extensions().map(|(n, _)| *n).collect();
        let mut terminated_seen = false;
        for (op, n, e) in ops {
            let ns = nss[usize::from(n)];
            match op {
                0 => { let _ = s.begin(ns); }
                1 => {
                    let in_flight = !s.is_terminated() && matches!(s.state(&ns), ExtensionState::Negotiating);
                    let before = s.state(&ns).clone();
                    let _ = s.finish(ns, Ok(CapabilitySet::new()));
                    if !in_flight { prop_assert_eq!(s.state(&ns), &before); }
                }
                2 => {
                    let err = match e {
                        0 => E::Cancelled, 1 => E::LimitExceeded, 2 => E::DowngradeSuspected, 3 => E::Malformed,
                        4 => E::MissingRequired(CapabilityId::new(ns, 1)), 5 => E::TranscriptMismatch,
                        6 => E::SecurityPolicy(CapabilityId::new(ns, 1)), _ => E::UnsupportedCore,
                    };
                    let in_flight = !s.is_terminated() && matches!(s.state(&ns), ExtensionState::Negotiating);
                    let before = s.state(&ns).clone();
                    let a = s.finish(ns, Err(err.clone()));
                    if in_flight {
                        prop_assert_eq!(a, classify(&err, NegotiationStage::Extension));
                    } else {
                        prop_assert_eq!(a, FailureAction::Ignore, "a result for something not in flight is a no-op");
                        prop_assert_eq!(s.state(&ns), &before);
                    }
                }
                3 => s.reset(&ns),
                _ => {}
            }
            terminated_seen |= s.is_terminated();
            prop_assert_eq!(s.is_terminated(), terminated_seen, "termination is permanent");
            for ns in nss {
                if s.get(&ns).is_some() {
                    prop_assert!(supported.contains(&ns), "{ns:?} became usable without being supported");
                    prop_assert!(!s.is_terminated());
                    prop_assert!(matches!(s.state(&ns), ExtensionState::Ready(_)));
                }
                if s.is_terminated() { prop_assert!(!s.is_usable(&ns)); }
            }
        }
        // nothing outside the intersection ever became usable by being merely begun
        let mut fresh = HierarchicalSession::establish(&adv(&[Ns::Files]), &adv(&[Ns::Media]));
        prop_assert!(fresh.begin(Ns::Files).is_err());
    }

    #[test]
    fn resume_shortcut_requires_every_condition(
        enabled in any::<bool>(), lsup in any::<bool>(), rsup in any::<bool>(), auth in any::<bool>(),
        have_cached in any::<bool>(), have_claim in any::<bool>(), same in any::<bool>(),
    ) {
        let mk = |c: u32| { let mut s = CapabilitySet::new(); s.insert(CapabilityDescriptor::new(CapabilityId::new(Ns::Files, c), V1, CapabilityRequirement::Optional, CapabilityParameters::None)).unwrap(); CapabilitySetHash::of(&s) };
        let (a, b) = (mk(1), mk(if same { 1 } else { 2 }));
        let i = ResumeInputs { local_supports_resume: lsup, remote_supports_resume: rsup, session_authenticated: auth, cached: have_cached.then_some(&a), claimed: have_claim.then_some(&b) };
        let (d, why) = decide(ResumePolicy { enabled }, &i);
        let all = enabled && lsup && rsup && auth && have_cached && have_claim && same;
        prop_assert_eq!(d == ResumeDecision::ReuseCached, all);
        prop_assert_eq!(why.is_none(), all);
    }

    #[test]
    fn only_authenticated_peers_back_contracts(src in 0usize..4) {
        let s = [CapabilitySource::AuthenticatedPeer, CapabilitySource::DirectoryHint, CapabilitySource::RelaySelfReport, CapabilitySource::PlatformAttestation][src];
        prop_assert!(may_rely_on(s, Purpose::Hint));
        prop_assert_eq!(may_rely_on(s, Purpose::Contract), s == CapabilitySource::AuthenticatedPeer);
    }

    #[test]
    fn security_class_capabilities_are_never_extension_local_failures(code in any::<u32>()) {
        let id = CapabilityId::new(Ns::Security, code);
        for e in [E::MissingRequired(id), E::VersionMismatch(id)] {
            prop_assert_eq!(classify(&e, NegotiationStage::Extension), FailureAction::TerminateSession);
        }
    }
}
