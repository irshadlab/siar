//! §84 no capability trust from a server alone, §85 relay capability
//! advertisement, §86 capability attestation.
//!
//! Capability information has a *source*, and what you may do with it
//! depends on the source:
//!
//! | source                   | may be used for        |
//! |--------------------------|------------------------|
//! | authenticated negotiation | the contract (decisions, security) |
//! | directory server          | hints only             |
//! | relay self-report         | hints only, clamped    |
//! | platform attestation      | supplemental display   |
//!
//! The separation is by *type*: hints live in [`HintStore`] / [`RelayHint`],
//! which expose only likelihoods and routing values — there is no method
//! that turns a hint into a [`CapabilitySet`] contract — so code cannot
//! accidentally base a security decision on a hint.

use crate::cache::{CapabilityCache, PeerKey, StalenessPolicy, Support};
use crate::epoch::{CapabilityEpoch, PeerCapabilityView};
use crate::id::CapabilityId;
use crate::set::CapabilitySet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySource {
    AuthenticatedPeer,
    DirectoryHint,
    RelaySelfReport,
    PlatformAttestation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Ordering route candidates, pre-warming, UI hints.
    Hint,
    /// Allowing/denying an operation or any security-sensitive decision.
    Contract,
}

/// The single rule behind the table above.
pub fn may_rely_on(source: CapabilitySource, purpose: Purpose) -> bool {
    match (source, purpose) {
        (CapabilitySource::AuthenticatedPeer, _) => true,
        (_, Purpose::Hint) => true,
        (_, Purpose::Contract) => false,
    }
}

// ------------------------------------------------------------- §84

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintedSupport {
    Likely,
    Unlikely,
    Unknown,
}

/// Capability hints published by a directory. Useful to decide *whom to
/// try first*; never proof of anything.
#[derive(Debug)]
pub struct HintStore {
    cache: CapabilityCache,
}

impl HintStore {
    pub fn new(max_entries: usize, policy: StalenessPolicy) -> Self {
        Self {
            cache: CapabilityCache::new(max_entries, policy),
        }
    }

    pub fn put_directory_hint(&mut self, peer: PeerKey, claimed: CapabilitySet, now_millis: u64) {
        self.cache.put(
            peer,
            PeerCapabilityView::from_snapshot(CapabilityEpoch::default(), claimed),
            now_millis,
        );
    }

    /// Deliberately returns a *likelihood*: even a fresh directory entry
    /// is only `Likely`, never `Yes`.
    pub fn suggest(&self, peer: &PeerKey, cap: &CapabilityId, now_millis: u64) -> HintedSupport {
        match self.cache.supports(peer, cap, now_millis) {
            Support::Yes | Support::MaybeStale => HintedSupport::Likely,
            Support::No => HintedSupport::Unlikely,
            Support::Unknown => HintedSupport::Unknown,
        }
    }

    pub fn forget(&mut self, peer: &PeerKey) -> bool {
        self.cache.invalidate(peer)
    }
}

// ------------------------------------------------------------- §85

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelayStorageClass {
    Volatile,
    Persistent,
    Archival,
}

/// What a relay says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayAdvertisement {
    pub max_bundle_bytes: u64,
    pub is_gateway: bool,
    pub storage_class: RelayStorageClass,
}

/// Bounds we are willing to *believe* even as a hint. A relay claiming a
/// 100 TiB bundle limit does not get routing preference for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayHintLimits {
    pub max_credible_bundle_bytes: u64,
}

impl Default for RelayHintLimits {
    fn default() -> Self {
        Self {
            max_credible_bundle_bytes: 16 * 1024 * 1024,
        }
    }
}

/// A clamped, routing-only view of a relay's self-report. It has no
/// accessor for a "contract"; `for_routing` is the whole interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayHint {
    max_bundle_bytes: u64,
    is_gateway: bool,
    storage_class: RelayStorageClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingView {
    pub max_bundle_bytes: u64,
    pub is_gateway: bool,
    pub storage_class: RelayStorageClass,
}

impl RelayHint {
    pub fn from_advertisement(a: RelayAdvertisement, limits: RelayHintLimits) -> Self {
        Self {
            max_bundle_bytes: a.max_bundle_bytes.min(limits.max_credible_bundle_bytes),
            is_gateway: a.is_gateway,
            storage_class: a.storage_class,
        }
    }

    pub fn for_routing(&self) -> RoutingView {
        RoutingView {
            max_bundle_bytes: self.max_bundle_bytes,
            is_gateway: self.is_gateway,
            storage_class: self.storage_class,
        }
    }

    /// Asking a relay hint to back a security-sensitive decision is
    /// refused by construction.
    pub fn for_purpose(&self, purpose: Purpose) -> Option<RoutingView> {
        may_rely_on(CapabilitySource::RelaySelfReport, purpose).then(|| self.for_routing())
    }
}

// ------------------------------------------------------------- §86

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AttestedProperties {
    pub hardware_backed_keys: bool,
    pub secure_boot: bool,
}

/// Outcome of verifying platform attestation evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attestation {
    Absent,
    Invalid,
    Verified(AttestedProperties),
}

/// Platform adapters implement this; the crate does not know how to
/// verify an Android key attestation or Apple App Attest chain.
pub trait AttestationVerifier {
    fn verify(&self, evidence: &[u8]) -> Attestation;
}

impl Attestation {
    /// Negotiation never waits on, or fails because of, attestation:
    /// absent, invalid or verified, the session proceeds identically.
    pub const fn negotiation_may_proceed(&self) -> bool {
        true
    }

    /// Properties safe to *display or log*. Unverified evidence yields
    /// nothing.
    pub fn properties(&self) -> AttestedProperties {
        match self {
            Self::Verified(p) => *p,
            Self::Absent | Self::Invalid => AttestedProperties::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityNamespace;
    use crate::version::CapabilityVersion;

    const FILES: CapabilityId = CapabilityId::new(CapabilityNamespace::Files, 1);

    fn set_with(id: CapabilityId) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        s.insert(CapabilityDescriptor::new(
            id,
            CapabilityVersion::new(1, 0),
            CapabilityRequirement::Optional,
            CapabilityParameters::None,
        ))
        .unwrap();
        s
    }

    #[test]
    fn only_an_authenticated_peer_can_back_a_contract_decision() {
        for src in [
            CapabilitySource::DirectoryHint,
            CapabilitySource::RelaySelfReport,
            CapabilitySource::PlatformAttestation,
        ] {
            assert!(may_rely_on(src, Purpose::Hint), "{src:?} is fine as a hint");
            assert!(
                !may_rely_on(src, Purpose::Contract),
                "{src:?} must never back a contract"
            );
        }
        assert!(may_rely_on(
            CapabilitySource::AuthenticatedPeer,
            Purpose::Contract
        ));
        assert!(may_rely_on(
            CapabilitySource::AuthenticatedPeer,
            Purpose::Hint
        ));
    }

    #[test]
    fn directory_hints_are_likelihoods_never_yes() {
        let mut h = HintStore::new(8, StalenessPolicy::default());
        let p = [1u8; 32];
        assert_eq!(h.suggest(&p, &FILES, 0), HintedSupport::Unknown);
        h.put_directory_hint(p, set_with(FILES), 0);
        assert_eq!(h.suggest(&p, &FILES, 1), HintedSupport::Likely);
        assert_eq!(
            h.suggest(&p, &CapabilityId::new(CapabilityNamespace::Dtn, 1), 1),
            HintedSupport::Unlikely
        );
        // a long-stale absence proves nothing
        assert_eq!(
            h.suggest(
                &p,
                &CapabilityId::new(CapabilityNamespace::Dtn, 1),
                10 * 24 * 3_600_000
            ),
            HintedSupport::Unknown
        );
        assert!(h.forget(&p));
        assert_eq!(h.suggest(&p, &FILES, 2), HintedSupport::Unknown);
    }

    #[test]
    fn a_lying_directory_cannot_grant_anything_a_hint_has_no_contract_form() {
        // The API surface itself: HintStore only returns HintedSupport.
        let mut h = HintStore::new(8, StalenessPolicy::default());
        h.put_directory_hint([2; 32], set_with(FILES), 0);
        let s: HintedSupport = h.suggest(&[2; 32], &FILES, 0);
        assert_ne!(s, HintedSupport::Unknown);
        // (no conversion from HintedSupport to a negotiated CapabilitySet exists)
    }

    #[test]
    fn relay_self_reports_are_clamped_and_refused_for_security_purposes() {
        let liar = RelayAdvertisement {
            max_bundle_bytes: u64::MAX,
            is_gateway: true,
            storage_class: RelayStorageClass::Archival,
        };
        let h = RelayHint::from_advertisement(liar, RelayHintLimits::default());
        assert_eq!(h.for_routing().max_bundle_bytes, 16 * 1024 * 1024);
        assert!(h.for_routing().is_gateway);
        assert!(h.for_purpose(Purpose::Hint).is_some());
        assert_eq!(h.for_purpose(Purpose::Contract), None);
        let modest = RelayAdvertisement {
            max_bundle_bytes: 1024,
            is_gateway: false,
            storage_class: RelayStorageClass::Volatile,
        };
        assert_eq!(
            RelayHint::from_advertisement(modest, RelayHintLimits::default())
                .for_routing()
                .max_bundle_bytes,
            1024,
            "believable values pass through"
        );
    }

    #[test]
    fn attestation_never_gates_negotiation_and_unverified_evidence_shows_nothing() {
        let good = Attestation::Verified(AttestedProperties {
            hardware_backed_keys: true,
            secure_boot: true,
        });
        for a in [Attestation::Absent, Attestation::Invalid, good] {
            assert!(a.negotiation_may_proceed());
        }
        assert_eq!(
            Attestation::Absent.properties(),
            AttestedProperties::default()
        );
        assert_eq!(
            Attestation::Invalid.properties(),
            AttestedProperties::default()
        );
        assert!(good.properties().hardware_backed_keys);
    }

    #[test]
    fn verifier_trait_is_pluggable() {
        struct Fake;
        impl AttestationVerifier for Fake {
            fn verify(&self, evidence: &[u8]) -> Attestation {
                match evidence {
                    [] => Attestation::Absent,
                    [1, ..] => Attestation::Verified(AttestedProperties {
                        hardware_backed_keys: true,
                        secure_boot: false,
                    }),
                    _ => Attestation::Invalid,
                }
            }
        }
        assert_eq!(Fake.verify(&[]), Attestation::Absent);
        assert!(matches!(Fake.verify(&[1, 9]), Attestation::Verified(_)));
        assert_eq!(Fake.verify(&[9]), Attestation::Invalid);
    }
}
