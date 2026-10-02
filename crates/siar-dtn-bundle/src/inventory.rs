//! §29 "Inventory Summary" and §31 "Reconciliation Flow".
//!
//! Small stores send an exact bounded recent-ID list; larger ones send a
//! [`BundleBloom`]. IBLT / Merkle-set reconciliation are listed by the
//! spec as "later" and are intentionally not attempted here.

use serde::{Deserialize, Serialize};

use crate::bloom::BundleBloom;
use crate::types::{BundleId, DtnPriority};

/// Above this many IDs, switch from an exact list to a Bloom filter.
pub const MAX_RECENT_IDS: usize = 256;
const BLOOM_FP_RATE: f64 = 0.01;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InventorySummary {
    RecentIds(Vec<BundleId>),
    Bloom(BundleBloom),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("inventory summary lists {0} ids, more than the {MAX_RECENT_IDS} allowed")]
pub struct SummaryTooLarge(pub usize);

impl InventorySummary {
    /// Builds the cheapest honest summary of `ids`.
    pub fn build(ids: &[BundleId], salt: u64) -> Self {
        if ids.len() <= MAX_RECENT_IDS {
            return Self::RecentIds(ids.to_vec());
        }
        let mut f = BundleBloom::with_capacity(ids.len(), BLOOM_FP_RATE, salt);
        for id in ids {
            f.insert(id);
        }
        Self::Bloom(f)
    }

    /// Reject oversized peer-supplied summaries before using them.
    pub fn validate(&self) -> Result<(), SummaryTooLarge> {
        match self {
            Self::RecentIds(v) if v.len() > MAX_RECENT_IDS => Err(SummaryTooLarge(v.len())),
            _ => Ok(()),
        }
    }

    /// Exact for `RecentIds`, probabilistic for `Bloom`. A `true` from a
    /// Bloom summary only ever suppresses a redundant send.
    pub fn probably_has(&self, id: &BundleId) -> bool {
        match self {
            Self::RecentIds(v) => v.contains(id),
            Self::Bloom(f) => f.may_contain(id),
        }
    }
}

/// What the local node knows about a bundle it could offer a peer (§31's
/// selection inputs that are available without transport/battery state;
/// those are applied by [`crate::scheduler`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferCandidate {
    pub id: BundleId,
    pub priority: DtnPriority,
    pub expires_at_millis: u64,
    pub replication_budget: u8,
    pub size_bytes: u32,
}

/// Bundles worth offering to a peer whose summary is `peer`: not already
/// held, not expired, still replicable. Highest priority first, then
/// soonest expiry (most urgent), then smallest first so tiny critical
/// objects precede bulk (§81).
pub fn select_missing<'a>(
    candidates: &'a [OfferCandidate],
    peer: &InventorySummary,
    now_millis: u64,
) -> Vec<&'a OfferCandidate> {
    let mut out: Vec<&OfferCandidate> = candidates
        .iter()
        .filter(|c| {
            c.expires_at_millis > now_millis
                && c.replication_budget > 0
                && !peer.probably_has(&c.id)
        })
        .collect();
    out.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.expires_at_millis.cmp(&b.expires_at_millis))
            .then(a.size_bytes.cmp(&b.size_bytes))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(priority: DtnPriority, expires: u64, budget: u8, size: u32) -> OfferCandidate {
        OfferCandidate {
            id: BundleId::new(),
            priority,
            expires_at_millis: expires,
            replication_budget: budget,
            size_bytes: size,
        }
    }

    #[test]
    fn small_inventories_are_exact_large_ones_use_bloom() {
        let few: Vec<_> = (0..10).map(|_| BundleId::new()).collect();
        assert!(matches!(
            InventorySummary::build(&few, 1),
            InventorySummary::RecentIds(_)
        ));
        let many: Vec<_> = (0..MAX_RECENT_IDS + 1).map(|_| BundleId::new()).collect();
        let s = InventorySummary::build(&many, 1);
        assert!(matches!(s, InventorySummary::Bloom(_)));
        assert!(many.iter().all(|id| s.probably_has(id)));
    }

    #[test]
    fn oversized_exact_list_from_a_peer_is_rejected() {
        let ids: Vec<_> = (0..MAX_RECENT_IDS + 1).map(|_| BundleId::new()).collect();
        assert!(InventorySummary::RecentIds(ids).validate().is_err());
    }

    #[test]
    fn selection_filters_and_orders() {
        let held = cand(DtnPriority::Sos, 9_000, 3, 10);
        let expired = cand(DtnPriority::Sos, 50, 3, 10);
        let no_budget = cand(DtnPriority::Sos, 9_000, 0, 10);
        let low = cand(DtnPriority::Low, 9_000, 1, 10);
        let sos_big = cand(DtnPriority::Sos, 9_000, 3, 5_000);
        let sos_small = cand(DtnPriority::Sos, 9_000, 3, 100);
        let peer = InventorySummary::RecentIds(vec![held.id]);
        let all = [
            held,
            expired,
            no_budget,
            low.clone(),
            sos_big.clone(),
            sos_small.clone(),
        ];
        let picked = select_missing(&all, &peer, 100);
        let ids: Vec<_> = picked.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![sos_small.id, sos_big.id, low.id]);
    }
}
