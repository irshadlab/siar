//! §89-93: coarse resource classes, privacy profiles, progressive
//! disclosure, and fingerprinting resistance.
//!
//! An exact capability set is a device fingerprint (§93). Disclosure is
//! staged: before authentication a peer sees only what is needed to
//! negotiate (`Required` capabilities and the Core/Security namespaces);
//! more is revealed as trust grows, and `Minimal` profiles also round
//! resource limits *down* to a power of two (§89). Rounding never goes
//! up, so a node can't be induced into honouring a limit it didn't offer.

use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
use crate::id::CapabilityNamespace;
use crate::set::CapabilitySet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DisclosureStage {
    PreAuth,
    Authenticated,
    Trusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyProfile {
    /// Reveal everything once past pre-auth negotiation needs.
    Open,
    Standard,
    /// Minimum viable disclosure; coarsens resource limits.
    Minimal,
}

/// Round down to a power of two (0 stays 0).
pub fn coarsen_u64(v: u64) -> u64 {
    if v == 0 {
        0
    } else {
        1u64 << (63 - v.leading_zeros())
    }
}

/// §89 coarse classes, for UI/diagnostics where exact numbers are unneeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceClass {
    Tiny,
    Small,
    Medium,
    Large,
}

impl ResourceClass {
    pub fn from_bytes(bytes: u64) -> Self {
        match bytes {
            0..=65_535 => Self::Tiny,
            65_536..=4_194_303 => Self::Small,
            4_194_304..=268_435_455 => Self::Medium,
            _ => Self::Large,
        }
    }
}

fn stage_for(d: &CapabilityDescriptor, profile: PrivacyProfile) -> DisclosureStage {
    use CapabilityNamespace as N;
    if d.requirement == CapabilityRequirement::Required
        || matches!(d.id.namespace, N::Core | N::Security)
    {
        return DisclosureStage::PreAuth; // needed to negotiate at all
    }
    // Hardware/resource detail is the strongest fingerprinting signal.
    let sensitive = matches!(d.id.namespace, N::Platform | N::Resource | N::Media);
    match (profile, sensitive) {
        (PrivacyProfile::Open, _) => DisclosureStage::PreAuth,
        (PrivacyProfile::Standard, false) => DisclosureStage::PreAuth,
        (PrivacyProfile::Standard, true) => DisclosureStage::Authenticated,
        (PrivacyProfile::Minimal, false) => DisclosureStage::Authenticated,
        (PrivacyProfile::Minimal, true) => DisclosureStage::Trusted,
    }
}

fn coarsen_params(d: &CapabilityDescriptor) -> CapabilityDescriptor {
    let mut out = d.clone();
    if d.id.namespace == CapabilityNamespace::Resource {
        out.parameters = match &d.parameters {
            CapabilityParameters::U32(v) => {
                CapabilityParameters::U32(coarsen_u64(u64::from(*v)) as u32)
            }
            CapabilityParameters::U64(v) => CapabilityParameters::U64(coarsen_u64(*v)),
            CapabilityParameters::RangeU32 { min, max } => CapabilityParameters::RangeU32 {
                // min rounds *up-safe*? A range promise shrinks inward only:
                // keep min, round max down but never below min.
                min: *min,
                max: (coarsen_u64(u64::from(*max)) as u32).max(*min),
            },
            other => other.clone(),
        };
    }
    out
}

/// What to put in the advertisement at `stage`.
pub fn disclose(
    full: &CapabilitySet,
    stage: DisclosureStage,
    profile: PrivacyProfile,
) -> CapabilitySet {
    let mut out = CapabilitySet::new();
    for d in full.iter() {
        if stage_for(d, profile) <= stage {
            let d = if profile == PrivacyProfile::Minimal {
                coarsen_params(d)
            } else {
                d.clone()
            };
            // Subset of an already-bounded set: cannot exceed the bound.
            out.insert(d).expect("subset of a valid set fits");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::CapabilityId;
    use crate::version::CapabilityVersion;

    fn d(
        ns: CapabilityNamespace,
        code: u32,
        req: CapabilityRequirement,
        p: CapabilityParameters,
    ) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new(ns, code),
            CapabilityVersion::new(1, 0),
            req,
            p,
        )
    }

    fn full() -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for x in [
            d(
                CapabilityNamespace::Core,
                1,
                CapabilityRequirement::Required,
                CapabilityParameters::None,
            ),
            d(
                CapabilityNamespace::Files,
                1,
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ),
            d(
                CapabilityNamespace::Platform,
                1,
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ),
            d(
                CapabilityNamespace::Resource,
                1,
                CapabilityRequirement::Optional,
                CapabilityParameters::U64(1_000_000),
            ),
        ] {
            s.insert(x).unwrap();
        }
        s
    }

    #[test]
    fn preauth_reveals_only_what_negotiation_needs_under_minimal() {
        let out = disclose(&full(), DisclosureStage::PreAuth, PrivacyProfile::Minimal);
        assert_eq!(out.len(), 1);
        assert!(out.contains(&CapabilityId::new(CapabilityNamespace::Core, 1)));
    }

    #[test]
    fn disclosure_is_monotonic_in_stage() {
        for profile in [
            PrivacyProfile::Open,
            PrivacyProfile::Standard,
            PrivacyProfile::Minimal,
        ] {
            let (a, b, c) = (
                disclose(&full(), DisclosureStage::PreAuth, profile).len(),
                disclose(&full(), DisclosureStage::Authenticated, profile).len(),
                disclose(&full(), DisclosureStage::Trusted, profile).len(),
            );
            assert!(a <= b && b <= c, "{profile:?}: {a} {b} {c}");
        }
        assert_eq!(
            disclose(&full(), DisclosureStage::PreAuth, PrivacyProfile::Open).len(),
            4
        );
    }

    #[test]
    fn minimal_rounds_limits_down_never_up() {
        let out = disclose(&full(), DisclosureStage::Trusted, PrivacyProfile::Minimal);
        let r = out
            .get(&CapabilityId::new(CapabilityNamespace::Resource, 1))
            .unwrap();
        assert_eq!(r.parameters, CapabilityParameters::U64(524_288));
        let std = disclose(&full(), DisclosureStage::Trusted, PrivacyProfile::Standard);
        assert_eq!(
            std.get(&CapabilityId::new(CapabilityNamespace::Resource, 1))
                .unwrap()
                .parameters,
            CapabilityParameters::U64(1_000_000)
        );
    }

    #[test]
    fn coarsening_and_classes() {
        assert_eq!(coarsen_u64(0), 0);
        assert_eq!(coarsen_u64(1), 1);
        assert_eq!(coarsen_u64(1023), 512);
        assert_eq!(coarsen_u64(u64::MAX), 1 << 63);
        assert_eq!(ResourceClass::from_bytes(10), ResourceClass::Tiny);
        assert_eq!(ResourceClass::from_bytes(1 << 30), ResourceClass::Large);
    }
}
