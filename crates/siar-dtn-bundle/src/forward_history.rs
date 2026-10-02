//! §142-144: loop prevention, bounded forward history, peer-bounce
//! prevention. "Never rely on hop limit alone."
//!
//! Both maps are bounded (per-bundle and total) so a relay that sees
//! many peers can't grow this without limit.

use std::collections::{HashMap, VecDeque};

use crate::bundle::DtnBundle;
use crate::forwarding::{
    decide_forwarding, peer_is_destination, EncounteredPeer, ForwardingDecision,
};
use crate::types::{BundleId, RouteToken};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardDirection {
    /// We handed the bundle to this peer.
    SentTo,
    /// This peer handed the bundle to us (so returning it is a bounce).
    ReceivedFrom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardResult {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardRecord {
    pub peer: RouteToken,
    pub at_millis: u64,
    pub direction: ForwardDirection,
    pub result: ForwardResult,
}

#[derive(Debug)]
pub struct ForwardHistory {
    per_bundle: HashMap<BundleId, VecDeque<ForwardRecord>>,
    max_per_bundle: usize,
    max_bundles: usize,
}

impl ForwardHistory {
    pub fn new(max_per_bundle: usize, max_bundles: usize) -> Self {
        Self {
            per_bundle: HashMap::new(),
            max_per_bundle: max_per_bundle.max(1),
            max_bundles: max_bundles.max(1),
        }
    }

    pub fn record(&mut self, id: BundleId, rec: ForwardRecord) {
        if !self.per_bundle.contains_key(&id) && self.per_bundle.len() >= self.max_bundles {
            // Evict the bundle whose most recent activity is oldest.
            if let Some(oldest) = self
                .per_bundle
                .iter()
                .min_by_key(|(_, q)| q.back().map_or(0, |r| r.at_millis))
                .map(|(k, _)| *k)
            {
                self.per_bundle.remove(&oldest);
            }
        }
        let q = self.per_bundle.entry(id).or_default();
        if q.len() >= self.max_per_bundle {
            q.pop_front();
        }
        q.push_back(rec);
    }

    /// True if `peer` already got this bundle from us, or gave it to us.
    /// A *failed* send is deliberately not counted: it stays retryable.
    pub fn peer_already_has(&self, id: &BundleId, peer: &RouteToken) -> bool {
        self.per_bundle.get(id).is_some_and(|q| {
            q.iter().any(|r| {
                &r.peer == peer
                    && (r.direction == ForwardDirection::ReceivedFrom
                        || r.result == ForwardResult::Succeeded)
            })
        })
    }

    pub fn forget(&mut self, id: &BundleId) {
        self.per_bundle.remove(id);
    }

    pub fn tracked_bundles(&self) -> usize {
        self.per_bundle.len()
    }
}

/// [`decide_forwarding`] with §144 bounce prevention applied: peers that
/// already have (or gave us) the bundle are never spray/gateway targets.
/// The bundle's actual destination is always kept so direct delivery
/// remains possible (a prior non-final hand-off is not delivery).
pub fn decide_forwarding_with_history(
    bundle: &DtnBundle,
    peers: &[EncounteredPeer],
    history: &ForwardHistory,
    now_millis: u64,
) -> ForwardingDecision {
    let eligible: Vec<EncounteredPeer> = peers
        .iter()
        .filter(|p| {
            peer_is_destination(&p.route_token, &bundle.destination)
                || !history.peer_already_has(&bundle.bundle_id, &p.route_token)
        })
        .cloned()
        .collect();
    decide_forwarding(bundle, &eligible, now_millis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::test_bundle;

    fn peer(b: u8, gw: bool) -> EncounteredPeer {
        EncounteredPeer {
            route_token: RouteToken(vec![b]),
            is_gateway: gw,
        }
    }

    fn rec(b: u8, at: u64, d: ForwardDirection, r: ForwardResult) -> ForwardRecord {
        ForwardRecord {
            peer: RouteToken(vec![b]),
            at_millis: at,
            direction: d,
            result: r,
        }
    }

    #[test]
    fn a_peer_that_gave_us_the_bundle_is_never_a_spray_target() {
        let b = test_bundle();
        let mut h = ForwardHistory::new(8, 8);
        h.record(
            b.bundle_id,
            rec(
                10,
                1,
                ForwardDirection::ReceivedFrom,
                ForwardResult::Succeeded,
            ),
        );
        let d = decide_forwarding_with_history(&b, &[peer(10, false), peer(11, false)], &h, 5);
        match d {
            ForwardingDecision::Spray { targets, .. } => {
                assert_eq!(targets, vec![RouteToken(vec![11])])
            }
            other => panic!("expected spray, got {other:?}"),
        }
    }

    #[test]
    fn failed_sends_stay_retryable_successful_ones_do_not() {
        let b = test_bundle();
        let mut h = ForwardHistory::new(8, 8);
        h.record(
            b.bundle_id,
            rec(1, 1, ForwardDirection::SentTo, ForwardResult::Failed),
        );
        assert!(!h.peer_already_has(&b.bundle_id, &RouteToken(vec![1])));
        h.record(
            b.bundle_id,
            rec(1, 2, ForwardDirection::SentTo, ForwardResult::Succeeded),
        );
        assert!(h.peer_already_has(&b.bundle_id, &RouteToken(vec![1])));
    }

    #[test]
    fn destination_is_still_reachable_after_an_earlier_handoff() {
        let b = test_bundle(); // destination token = [2]
        let mut h = ForwardHistory::new(8, 8);
        h.record(
            b.bundle_id,
            rec(2, 1, ForwardDirection::SentTo, ForwardResult::Succeeded),
        );
        let d = decide_forwarding_with_history(&b, &[peer(2, false)], &h, 5);
        assert!(matches!(d, ForwardingDecision::DeliverDirect { .. }));
    }

    #[test]
    fn history_is_bounded_both_ways() {
        let mut h = ForwardHistory::new(2, 3);
        let id = BundleId::new();
        for i in 0..5u8 {
            h.record(
                id,
                rec(
                    i,
                    u64::from(i),
                    ForwardDirection::SentTo,
                    ForwardResult::Succeeded,
                ),
            );
        }
        assert!(!h.peer_already_has(&id, &RouteToken(vec![0])));
        assert!(h.peer_already_has(&id, &RouteToken(vec![4])));
        for i in 0..10u64 {
            h.record(
                BundleId::new(),
                rec(
                    0,
                    100 + i,
                    ForwardDirection::SentTo,
                    ForwardResult::Succeeded,
                ),
            );
        }
        assert_eq!(h.tracked_bundles(), 3);
    }
}
