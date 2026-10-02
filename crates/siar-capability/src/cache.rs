//! §49-53: bounded peer-capability cache with explicit staleness.
//!
//! "Do not present stale support as guaranteed" (§51): lookups return
//! [`Support`], which separates confirmed-fresh support from
//! possibly-outdated support and from not knowing at all.

use std::collections::HashMap;

use crate::epoch::PeerCapabilityView;
use crate::id::CapabilityId;

/// Opaque 32-byte peer key (e.g. a device-identity hash).
pub type PeerKey = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Fresh and advertised.
    Yes,
    /// Advertised, but the observation is old. Show as "may work".
    MaybeStale,
    /// Fresh and not advertised.
    No,
    /// Nothing cached, or too old to say anything.
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct StalenessPolicy {
    pub fresh_for_millis: u64,
    pub stale_for_millis: u64,
}

impl Default for StalenessPolicy {
    fn default() -> Self {
        Self {
            fresh_for_millis: 5 * 60_000,
            stale_for_millis: 24 * 60 * 60_000,
        }
    }
}

struct Entry {
    view: PeerCapabilityView,
    observed_at_millis: u64,
}

pub struct CapabilityCache {
    entries: HashMap<PeerKey, Entry>,
    max_entries: usize,
    policy: StalenessPolicy,
}

impl CapabilityCache {
    pub fn new(max_entries: usize, policy: StalenessPolicy) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: max_entries.max(1),
            policy,
        }
    }

    pub fn put(&mut self, peer: PeerKey, view: PeerCapabilityView, now_millis: u64) {
        if !self.entries.contains_key(&peer) && self.entries.len() >= self.max_entries {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.observed_at_millis)
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            peer,
            Entry {
                view,
                observed_at_millis: now_millis,
            },
        );
    }

    /// §135: drop on identity change, key rotation, or session failure.
    pub fn invalidate(&mut self, peer: &PeerKey) -> bool {
        self.entries.remove(peer).is_some()
    }

    pub fn freshness(&self, peer: &PeerKey, now_millis: u64) -> Freshness {
        let Some(e) = self.entries.get(peer) else {
            return Freshness::Unknown;
        };
        let age = now_millis.saturating_sub(e.observed_at_millis);
        if age < self.policy.fresh_for_millis {
            Freshness::Fresh
        } else if age < self.policy.stale_for_millis {
            Freshness::Stale
        } else {
            Freshness::Unknown
        }
    }

    pub fn supports(&self, peer: &PeerKey, cap: &CapabilityId, now_millis: u64) -> Support {
        let Some(e) = self.entries.get(peer) else {
            return Support::Unknown;
        };
        let has = e.view.set().contains(cap);
        match (self.freshness(peer, now_millis), has) {
            (Freshness::Fresh, true) => Support::Yes,
            (Freshness::Fresh, false) => Support::No,
            // A stale *absence* proves nothing: the peer may have upgraded (§52).
            (Freshness::Stale, true) => Support::MaybeStale,
            (Freshness::Stale, false) | (Freshness::Unknown, _) => Support::Unknown,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::epoch::CapabilityEpoch;
    use crate::id::CapabilityNamespace;
    use crate::set::CapabilitySet;
    use crate::version::CapabilityVersion;

    fn cap(code: u32) -> CapabilityId {
        CapabilityId::new(CapabilityNamespace::Messaging, code)
    }

    fn view_with(code: u32) -> PeerCapabilityView {
        let mut s = CapabilitySet::new();
        s.insert(CapabilityDescriptor::new(
            cap(code),
            CapabilityVersion::new(1, 0),
            CapabilityRequirement::Optional,
            CapabilityParameters::None,
        ))
        .unwrap();
        PeerCapabilityView::from_snapshot(CapabilityEpoch(1), s)
    }

    fn cache() -> CapabilityCache {
        CapabilityCache::new(
            2,
            StalenessPolicy {
                fresh_for_millis: 100,
                stale_for_millis: 1_000,
            },
        )
    }

    #[test]
    fn fresh_stale_unknown_progression() {
        let mut c = cache();
        c.put([1; 32], view_with(1), 0);
        assert_eq!(c.supports(&[1; 32], &cap(1), 50), Support::Yes);
        assert_eq!(c.supports(&[1; 32], &cap(2), 50), Support::No);
        assert_eq!(c.supports(&[1; 32], &cap(1), 500), Support::MaybeStale);
        assert_eq!(
            c.supports(&[1; 32], &cap(2), 500),
            Support::Unknown,
            "stale absence is not 'No'"
        );
        assert_eq!(c.supports(&[1; 32], &cap(1), 5_000), Support::Unknown);
        assert_eq!(c.supports(&[9; 32], &cap(1), 0), Support::Unknown);
    }

    #[test]
    fn bounded_evicts_oldest_observation() {
        let mut c = cache();
        c.put([1; 32], view_with(1), 0);
        c.put([2; 32], view_with(1), 10);
        c.put([3; 32], view_with(1), 20);
        assert_eq!(c.len(), 2);
        assert_eq!(c.freshness(&[1; 32], 20), Freshness::Unknown);
    }

    #[test]
    fn invalidate_removes() {
        let mut c = cache();
        c.put([1; 32], view_with(1), 0);
        assert!(c.invalidate(&[1; 32]));
        assert!(!c.invalidate(&[1; 32]));
    }
}
