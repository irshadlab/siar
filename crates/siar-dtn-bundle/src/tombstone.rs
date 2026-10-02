//! §154-156: delivery/cancel tombstones so stale peers can't reintroduce
//! a completed bundle. Exact IDs only — a Bloom filter here would turn a
//! false positive into a wrongly rejected live bundle (§30 forbids
//! probabilistic structures for such decisions).

use std::collections::HashMap;

use crate::types::BundleId;

/// §155: must exceed the typical encounter delay horizon; not permanent.
pub const DEFAULT_TOMBSTONE_TTL_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneReason {
    Delivered,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReintroductionVerdict {
    Accept,
    /// §156: reject; for `Delivered`, a delivery summary is useful to the
    /// stale peer so it can retire its own copy.
    Reject {
        reason: TombstoneReason,
        send_delivery_summary: bool,
    },
}

#[derive(Debug)]
pub struct TombstoneTable {
    entries: HashMap<BundleId, (TombstoneReason, u64)>,
    ttl_millis: u64,
    max_entries: usize,
}

impl TombstoneTable {
    pub fn new(ttl_millis: u64, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            ttl_millis,
            max_entries: max_entries.max(1),
        }
    }

    pub fn insert(&mut self, id: BundleId, reason: TombstoneReason, now_millis: u64) {
        let expires = now_millis.saturating_add(self.ttl_millis);
        if !self.entries.contains_key(&id) && self.entries.len() >= self.max_entries {
            self.purge_expired(now_millis);
            if self.entries.len() >= self.max_entries {
                // Still full: drop the soonest-to-expire (least protective) one.
                if let Some(k) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, v)| v.1)
                    .map(|(k, _)| *k)
                {
                    self.entries.remove(&k);
                }
            }
        }
        self.entries
            .entry(id)
            .and_modify(|e| {
                // Delivered outranks Cancelled; expiry never shortens.
                if reason == TombstoneReason::Delivered {
                    e.0 = reason;
                }
                e.1 = e.1.max(expires);
            })
            .or_insert((reason, expires));
    }

    pub fn screen_incoming(&self, id: &BundleId, now_millis: u64) -> ReintroductionVerdict {
        match self.entries.get(id) {
            Some(&(reason, exp)) if exp > now_millis => ReintroductionVerdict::Reject {
                reason,
                send_delivery_summary: reason == TombstoneReason::Delivered,
            },
            _ => ReintroductionVerdict::Accept,
        }
    }

    /// Returns how many were removed. Safe to call from the periodic GC (§153).
    pub fn purge_expired(&mut self, now_millis: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, (_, exp)| *exp > now_millis);
        before - self.entries.len()
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

    #[test]
    fn delivered_bundle_is_rejected_with_summary_until_ttl() {
        let mut t = TombstoneTable::new(1_000, 10);
        let id = BundleId::new();
        t.insert(id, TombstoneReason::Delivered, 0);
        assert_eq!(
            t.screen_incoming(&id, 999),
            ReintroductionVerdict::Reject {
                reason: TombstoneReason::Delivered,
                send_delivery_summary: true
            }
        );
        assert_eq!(t.screen_incoming(&id, 1_000), ReintroductionVerdict::Accept);
    }

    #[test]
    fn cancelled_is_rejected_without_summary_and_unknown_is_accepted() {
        let mut t = TombstoneTable::new(1_000, 10);
        let id = BundleId::new();
        t.insert(id, TombstoneReason::Cancelled, 0);
        assert_eq!(
            t.screen_incoming(&id, 1),
            ReintroductionVerdict::Reject {
                reason: TombstoneReason::Cancelled,
                send_delivery_summary: false
            }
        );
        assert_eq!(
            t.screen_incoming(&BundleId::new(), 1),
            ReintroductionVerdict::Accept
        );
    }

    #[test]
    fn delivered_upgrades_cancelled_and_expiry_never_shortens() {
        let mut t = TombstoneTable::new(1_000, 10);
        let id = BundleId::new();
        t.insert(id, TombstoneReason::Cancelled, 500);
        t.insert(id, TombstoneReason::Delivered, 0);
        assert!(matches!(
            t.screen_incoming(&id, 1_200),
            ReintroductionVerdict::Reject {
                reason: TombstoneReason::Delivered,
                ..
            }
        ));
    }

    #[test]
    fn table_is_bounded_and_purge_removes_expired() {
        let mut t = TombstoneTable::new(100, 3);
        for i in 0..10 {
            t.insert(BundleId::new(), TombstoneReason::Delivered, i);
        }
        assert_eq!(t.len(), 3);
        assert_eq!(t.purge_expired(10_000), 3);
        assert!(t.is_empty());
    }
}
