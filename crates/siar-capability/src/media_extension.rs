//! §40-44: media capabilities and codec selection.
//!
//! Each codec is a `Media`-namespace capability (`code` = codec number).
//! Selection walks a *shared catalog* in catalog order — never the
//! peer's advertised order — so both sides pick the same codec (§72/§74
//! determinism, same rule [`crate::security`] uses for algorithms), skips
//! codecs [`HealthTracker`] has suspended (§44), and returns the
//! negotiated version. §43: this picks a codec for an already-chosen
//! route; it never influences routing.

use crate::health::HealthTracker;
use crate::id::{CapabilityId, CapabilityNamespace};
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedCodec {
    pub id: CapabilityId,
    pub version: CapabilityVersion,
}

/// `catalog` lists codec numbers from most to least preferred, and must be
/// identical on both peers (it is a protocol constant, not peer input).
pub fn select_codec(
    catalog: &[u32],
    local: &CapabilitySet,
    remote: &CapabilitySet,
    health: &HealthTracker,
    now_millis: u64,
) -> Option<SelectedCodec> {
    catalog.iter().find_map(|&code| {
        let id = CapabilityId::new(CapabilityNamespace::Media, code);
        if !health.status(&id, now_millis).is_usable() {
            return None;
        }
        let version = local
            .get(&id)?
            .version
            .negotiate(remote.get(&id)?.version)?;
        Some(SelectedCodec { id, version })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::health::HealthPolicy;

    fn set(codes: &[(u32, u16)]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for &(c, major) in codes {
            s.insert(CapabilityDescriptor::new(
                CapabilityId::new(CapabilityNamespace::Media, c),
                CapabilityVersion::new(major, 0),
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        }
        s
    }

    #[test]
    fn catalog_order_wins_not_peer_order_and_it_is_symmetric() {
        let (a, b) = (set(&[(1, 1), (2, 1), (3, 1)]), set(&[(3, 1), (2, 1)]));
        let h = HealthTracker::new(HealthPolicy::default());
        let ab = select_codec(&[3, 2, 1], &a, &b, &h, 0).unwrap();
        let ba = select_codec(&[3, 2, 1], &b, &a, &h, 0).unwrap();
        assert_eq!(ab, ba);
        assert_eq!(ab.id.code, 3);
    }

    #[test]
    fn a_failing_codec_is_skipped_until_it_recovers() {
        let (a, b) = (set(&[(1, 1), (2, 1)]), set(&[(1, 1), (2, 1)]));
        let mut h = HealthTracker::new(HealthPolicy {
            failure_threshold: 1,
            base_cooldown_millis: 100,
            ..Default::default()
        });
        h.record_failure(CapabilityId::new(CapabilityNamespace::Media, 1), 0);
        assert_eq!(select_codec(&[1, 2], &a, &b, &h, 50).unwrap().id.code, 2);
        assert_eq!(select_codec(&[1, 2], &a, &b, &h, 100).unwrap().id.code, 1);
    }

    #[test]
    fn no_shared_codec_is_none_and_incompatible_major_is_skipped() {
        let h = HealthTracker::new(HealthPolicy::default());
        assert!(select_codec(&[1], &set(&[(1, 1)]), &set(&[(2, 1)]), &h, 0).is_none());
        assert!(select_codec(&[1], &set(&[(1, 1)]), &set(&[(1, 2)]), &h, 0).is_none());
    }
}
