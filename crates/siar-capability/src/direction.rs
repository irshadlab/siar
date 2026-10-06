//! §75 asymmetric capabilities, §76 direction.
//!
//! "can encode AV1" and "can decode AV1" are different facts and must not
//! share one boolean. A directional capability carries a two-bit
//! [`CapabilityParameters::BitSet`] (bit 0 = I can send/produce, bit 1 =
//! I can receive/consume).
//!
//! **Why this needs its own negotiation.** The generic intersection rule
//! (§19) ANDs bitsets, which for direction gives the wrong answer: if I
//! can only send and you can only send, the AND says "send" survives —
//! yet nobody can receive. The correct rule is a *cross*: I may send only
//! if the peer can receive, and I may receive only if the peer can send.
//! [`negotiate_directional`] therefore pulls the named directional
//! capabilities out, negotiates everything else generically, and
//! cross-negotiates the directional ones; the result is from the local
//! device's point of view.

use std::collections::{BTreeMap, BTreeSet};

use crate::descriptor::{
    CapabilityBits, CapabilityDescriptor, CapabilityParameters, CapabilityRequirement,
};
use crate::error::CapabilityNegotiationError;
use crate::id::CapabilityId;
use crate::policy::CapabilityPolicy;
use crate::registry::CapabilityRegistry;
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

pub const SEND_BIT: u8 = 0;
pub const RECEIVE_BIT: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityDirection {
    Send,
    Receive,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DirectionError {
    #[error("directional capability must carry a bitset parameter")]
    NotABitSet,
    #[error("directional capability declares neither send nor receive")]
    NoDirection,
    #[error("directional bitset has undefined bits set")]
    UnknownBits,
}

impl CapabilityDirection {
    pub const fn can_send(self) -> bool {
        matches!(self, Self::Send | Self::Both)
    }

    pub const fn can_receive(self) -> bool {
        matches!(self, Self::Receive | Self::Both)
    }

    pub fn to_bits(self) -> CapabilityBits {
        let mut b = CapabilityBits::EMPTY;
        if self.can_send() {
            b = b.with_bit(SEND_BIT);
        }
        if self.can_receive() {
            b = b.with_bit(RECEIVE_BIT);
        }
        b
    }

    pub fn from_bits(bits: CapabilityBits) -> Result<Self, DirectionError> {
        if bits.0 & !0b11 != 0 {
            return Err(DirectionError::UnknownBits);
        }
        match (bits.has_bit(SEND_BIT), bits.has_bit(RECEIVE_BIT)) {
            (true, true) => Ok(Self::Both),
            (true, false) => Ok(Self::Send),
            (false, true) => Ok(Self::Receive),
            (false, false) => Err(DirectionError::NoDirection),
        }
    }
}

pub fn directional_descriptor(
    id: CapabilityId,
    version: CapabilityVersion,
    requirement: CapabilityRequirement,
    direction: CapabilityDirection,
) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        id,
        version,
        requirement,
        CapabilityParameters::BitSet(direction.to_bits()),
    )
}

pub fn read_direction(d: &CapabilityDescriptor) -> Result<CapabilityDirection, DirectionError> {
    match &d.parameters {
        CapabilityParameters::BitSet(b) => CapabilityDirection::from_bits(*b),
        _ => Err(DirectionError::NotABitSet),
    }
}

/// What the local device may do for one directional capability once
/// negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectionalOutcome {
    pub we_send: bool,
    pub we_receive: bool,
    pub version: CapabilityVersion,
}

impl DirectionalOutcome {
    pub fn direction(&self) -> Option<CapabilityDirection> {
        match (self.we_send, self.we_receive) {
            (true, true) => Some(CapabilityDirection::Both),
            (true, false) => Some(CapabilityDirection::Send),
            (false, true) => Some(CapabilityDirection::Receive),
            (false, false) => None,
        }
    }
}

/// `None` = no usable direction (version incompatible, or the two sides'
/// directions don't pair up).
pub fn cross_negotiate(
    local: &CapabilityDescriptor,
    remote: &CapabilityDescriptor,
) -> Result<Option<DirectionalOutcome>, DirectionError> {
    let (l, r) = (read_direction(local)?, read_direction(remote)?);
    let Some(version) = local.version.negotiate(remote.version) else {
        return Ok(None);
    };
    let out = DirectionalOutcome {
        we_send: l.can_send() && r.can_receive(),
        we_receive: l.can_receive() && r.can_send(),
        version,
    };
    Ok((out.we_send || out.we_receive).then_some(out))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DirectionalNegotiation {
    /// Everything that is not directional, negotiated by the §19 rules.
    pub general: CapabilitySet,
    /// Directional capabilities, from the local device's point of view.
    pub directional: BTreeMap<CapabilityId, DirectionalOutcome>,
}

fn without(set: &CapabilitySet, ids: &BTreeSet<CapabilityId>) -> CapabilitySet {
    let mut out = CapabilitySet::new();
    for d in set.iter().filter(|d| !ids.contains(&d.id)) {
        // Subset of an already-bounded set.
        out.insert(d.clone()).expect("subset of a valid set fits");
    }
    out
}

pub fn negotiate_directional(
    local: &CapabilitySet,
    remote: &CapabilitySet,
    registry: &CapabilityRegistry,
    policy: &CapabilityPolicy,
    directional: &BTreeSet<CapabilityId>,
) -> Result<DirectionalNegotiation, CapabilityNegotiationError> {
    // Validate the full sets (schemas, dependencies) before splitting.
    registry.validate(local)?;
    registry.validate(remote)?;

    let general = crate::negotiate::negotiate(
        &without(local, directional),
        &without(remote, directional),
        registry,
        policy,
    )?;

    let mut out = BTreeMap::new();
    for id in directional {
        let (l, r) = (local.get(id), remote.get(id));
        let required = [l, r]
            .into_iter()
            .flatten()
            .any(|d| d.requirement == CapabilityRequirement::Required);
        let outcome = match (l, r) {
            (Some(l), Some(r)) if !policy.is_disabled(id) => {
                cross_negotiate(l, r).map_err(|_| CapabilityNegotiationError::Malformed)?
            }
            (Some(d), None) | (None, Some(d)) => {
                // Malformed directional descriptors are rejected even if only
                // one side has the capability.
                read_direction(d).map_err(|_| CapabilityNegotiationError::Malformed)?;
                None
            }
            _ => None,
        };
        match outcome {
            Some(o) => {
                out.insert(*id, o);
            }
            None if required => return Err(CapabilityNegotiationError::MissingRequired(*id)),
            None => {}
        }
    }
    Ok(DirectionalNegotiation {
        general,
        directional: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::CapabilityNamespace;
    use crate::registry::{CapabilityDefinition, ParameterSchema, SecurityClass};
    use CapabilityDirection::*;

    const AV1: CapabilityId = CapabilityId::new(CapabilityNamespace::Media, 1);
    const OPUS: CapabilityId = CapabilityId::new(CapabilityNamespace::Media, 2);
    const V1: CapabilityVersion = CapabilityVersion::new(1, 0);

    fn d(id: CapabilityId, dir: CapabilityDirection) -> CapabilityDescriptor {
        directional_descriptor(id, V1, CapabilityRequirement::Optional, dir)
    }

    fn registry() -> CapabilityRegistry {
        let mut r = CapabilityRegistry::new();
        for (id, name) in [(AV1, "av1"), (OPUS, "opus")] {
            r.register(CapabilityDefinition {
                id,
                name,
                max_version: V1,
                parameter_schema: ParameterSchema::BitSet,
                security_class: SecurityClass::Functional,
            });
        }
        r
    }

    fn set(items: &[CapabilityDescriptor]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for i in items {
            s.insert(i.clone()).unwrap();
        }
        s
    }

    #[test]
    fn direction_bits_roundtrip_and_reject_garbage() {
        for dir in [Send, Receive, Both] {
            assert_eq!(CapabilityDirection::from_bits(dir.to_bits()), Ok(dir));
        }
        assert_eq!(
            CapabilityDirection::from_bits(CapabilityBits::EMPTY),
            Err(DirectionError::NoDirection)
        );
        assert_eq!(
            CapabilityDirection::from_bits(CapabilityBits(0b100)),
            Err(DirectionError::UnknownBits)
        );
        assert_eq!(
            CapabilityDirection::from_bits(CapabilityBits(u64::MAX)),
            Err(DirectionError::UnknownBits)
        );
    }

    #[test]
    fn send_pairs_with_receive_not_with_send() {
        let enc_only = d(AV1, Send);
        let dec_only = d(AV1, Receive);
        let a = cross_negotiate(&enc_only, &dec_only).unwrap().unwrap();
        assert_eq!(
            (a.we_send, a.we_receive),
            (true, false),
            "I encode, you decode"
        );
        let b = cross_negotiate(&dec_only, &enc_only).unwrap().unwrap();
        assert_eq!((b.we_send, b.we_receive), (false, true), "mirror image");
        // Two encode-only devices: nothing can flow — and the generic AND would have said "send".
        assert_eq!(cross_negotiate(&enc_only, &enc_only).unwrap(), None);
        assert_eq!(cross_negotiate(&dec_only, &dec_only).unwrap(), None);
        assert_eq!(
            enc_only.parameters,
            CapabilityParameters::BitSet(Send.to_bits())
        );
    }

    #[test]
    fn both_with_one_way_peer_keeps_only_the_possible_direction() {
        let o = cross_negotiate(&d(AV1, Both), &d(AV1, Receive))
            .unwrap()
            .unwrap();
        assert_eq!(
            (o.we_send, o.we_receive, o.direction()),
            (true, false, Some(Send))
        );
        let o = cross_negotiate(&d(AV1, Both), &d(AV1, Both))
            .unwrap()
            .unwrap();
        assert_eq!(o.direction(), Some(Both));
    }

    #[test]
    fn the_cross_rule_is_mirror_symmetric() {
        for l in [Send, Receive, Both] {
            for r in [Send, Receive, Both] {
                let a = cross_negotiate(&d(AV1, l), &d(AV1, r)).unwrap();
                let b = cross_negotiate(&d(AV1, r), &d(AV1, l)).unwrap();
                assert_eq!(
                    a.map(|o| (o.we_send, o.we_receive)),
                    b.map(|o| (o.we_receive, o.we_send)),
                    "{l:?}/{r:?}"
                );
            }
        }
    }

    #[test]
    fn incompatible_versions_and_malformed_descriptors() {
        let v2 = directional_descriptor(
            AV1,
            CapabilityVersion::new(2, 0),
            CapabilityRequirement::Optional,
            Both,
        );
        assert_eq!(cross_negotiate(&d(AV1, Both), &v2).unwrap(), None);
        let wrong = CapabilityDescriptor::new(
            AV1,
            V1,
            CapabilityRequirement::Optional,
            CapabilityParameters::U32(3),
        );
        assert_eq!(
            cross_negotiate(&d(AV1, Both), &wrong),
            Err(DirectionError::NotABitSet)
        );
        let none = CapabilityDescriptor::new(
            AV1,
            V1,
            CapabilityRequirement::Optional,
            CapabilityParameters::BitSet(CapabilityBits::EMPTY),
        );
        assert_eq!(
            cross_negotiate(&none, &d(AV1, Both)),
            Err(DirectionError::NoDirection)
        );
    }

    #[test]
    fn full_negotiation_splits_directional_from_general_and_is_from_the_local_view() {
        let ids: BTreeSet<_> = [AV1, OPUS].into_iter().collect();
        let (reg, pol) = (registry(), CapabilityPolicy::new());
        let phone = set(&[d(AV1, Both), d(OPUS, Send)]);
        let camera = set(&[d(AV1, Send), d(OPUS, Both)]);
        let n = negotiate_directional(&phone, &camera, &reg, &pol, &ids).unwrap();
        assert_eq!(
            n.directional[&AV1].direction(),
            Some(Receive),
            "phone can decode what the camera encodes"
        );
        assert_eq!(
            n.directional[&OPUS].direction(),
            Some(Send),
            "phone sends audio; camera listens"
        );
        assert!(n.general.is_empty());
        let m = negotiate_directional(&camera, &phone, &reg, &pol, &ids).unwrap();
        assert_eq!(m.directional[&AV1].direction(), Some(Send));
        assert_eq!(
            m.directional[&OPUS].direction(),
            Some(Receive),
            "camera listens to the phone's audio"
        );
    }

    #[test]
    fn disabled_missing_and_required_cases() {
        let ids: BTreeSet<_> = [AV1].into_iter().collect();
        let reg = registry();
        let both = set(&[d(AV1, Both)]);
        let mut pol = CapabilityPolicy::new();
        pol.disable_user(AV1);
        assert!(
            negotiate_directional(&both, &both, &reg, &pol, &ids)
                .unwrap()
                .directional
                .is_empty(),
            "policy removes it"
        );
        let pol = CapabilityPolicy::new();
        assert!(
            negotiate_directional(&both, &CapabilitySet::new(), &reg, &pol, &ids)
                .unwrap()
                .directional
                .is_empty()
        );
        // required directional capability that cannot pair up fails closed
        let req_send = set(&[directional_descriptor(
            AV1,
            V1,
            CapabilityRequirement::Required,
            Send,
        )]);
        let send_only = set(&[d(AV1, Send)]);
        assert_eq!(
            negotiate_directional(&req_send, &send_only, &reg, &pol, &ids).unwrap_err(),
            CapabilityNegotiationError::MissingRequired(AV1)
        );
    }
}
