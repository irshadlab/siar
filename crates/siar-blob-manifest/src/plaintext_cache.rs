//! §127 "Local Decryption Cache" and §128 "Thumbnail Cache".
//!
//! Decrypted bytes are the one thing this subsystem would rather not
//! keep. [`PlaintextCache`] is therefore **off unless asked for**
//! ([`PlaintextPolicy::OnDemand`] is the default and holds nothing), and
//! when it is on it is always bounded three ways: by bytes, by age, and
//! on demand ([`PlaintextCache::shrink_to`] for storage pressure, §128).
//! Entries are zeroized when they leave, whether by eviction, expiry or
//! [`PlaintextCache::clear`].
//!
//! Eviction is least-recently-used in `O(log n)` (an ordered index of
//! use-ticks, not a scan). One type serves both §127 (a larger budget,
//! short TTL, keyed by blob) and §128 (a small budget, keyed by the
//! thumbnail's own blob): they are two instances with different limits.

use std::collections::{BTreeMap, HashMap};

use zeroize::Zeroizing;

use crate::ids::BlobId;

/// §127's three options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaintextPolicy {
    /// Decrypt each time it is needed; keep nothing. The privacy default.
    #[default]
    OnDemand,
    /// Keep recently used plaintext in memory, bounded.
    Temporary { max_bytes: u64, ttl_millis: u64 },
}

struct Entry {
    data: Zeroizing<Vec<u8>>,
    tick: u64,
    inserted_millis: u64,
}

pub struct PlaintextCache {
    policy: PlaintextPolicy,
    entries: HashMap<BlobId, Entry>,
    /// tick → blob, oldest first.
    order: BTreeMap<u64, BlobId>,
    next_tick: u64,
    used: u64,
}

impl PlaintextCache {
    pub fn new(policy: PlaintextPolicy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
            order: BTreeMap::new(),
            next_tick: 0,
            used: 0,
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn limits(&self) -> Option<(u64, u64)> {
        match self.policy {
            PlaintextPolicy::OnDemand => None,
            PlaintextPolicy::Temporary {
                max_bytes,
                ttl_millis,
            } => Some((max_bytes, ttl_millis)),
        }
    }

    /// Stores `bytes`. Returns `false` (and keeps nothing) when the
    /// policy is `OnDemand` or the item alone exceeds the budget.
    pub fn insert(&mut self, blob: BlobId, bytes: Vec<u8>, now_millis: u64) -> bool {
        let Some((max_bytes, _)) = self.limits() else {
            return false;
        };
        let len = bytes.len() as u64;
        if len > max_bytes {
            return false;
        }
        self.remove(blob);
        self.evict_expired(now_millis);
        self.shrink_to(max_bytes - len);
        let tick = self.bump();
        self.order.insert(tick, blob);
        self.entries.insert(
            blob,
            Entry {
                data: Zeroizing::new(bytes),
                tick,
                inserted_millis: now_millis,
            },
        );
        self.used += len;
        true
    }

    /// A hit refreshes the entry's recency; an expired entry is a miss
    /// and is wiped.
    pub fn get(&mut self, blob: BlobId, now_millis: u64) -> Option<&[u8]> {
        let (_, ttl) = self.limits()?;
        let expired = self
            .entries
            .get(&blob)
            .is_some_and(|e| now_millis.saturating_sub(e.inserted_millis) >= ttl);
        if expired {
            self.remove(blob);
            return None;
        }
        let tick = self.bump();
        let e = self.entries.get_mut(&blob)?;
        self.order.remove(&e.tick);
        e.tick = tick;
        self.order.insert(tick, blob);
        Some(e.data.as_slice())
    }

    /// Drops everything older than the TTL.
    pub fn evict_expired(&mut self, now_millis: u64) {
        let Some((_, ttl)) = self.limits() else {
            return;
        };
        let stale: Vec<BlobId> = self
            .entries
            .iter()
            .filter(|(_, e)| now_millis.saturating_sub(e.inserted_millis) >= ttl)
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            self.remove(k);
        }
    }

    /// §128 storage-pressure eviction: least recently used out until at
    /// most `target_bytes` remain.
    pub fn shrink_to(&mut self, target_bytes: u64) {
        while self.used > target_bytes {
            let Some((&tick, &blob)) = self.order.iter().next() else {
                break;
            };
            self.order.remove(&tick);
            self.drop_entry(blob);
        }
    }

    pub fn remove(&mut self, blob: BlobId) {
        if let Some(e) = self.entries.get(&blob) {
            self.order.remove(&e.tick);
        }
        self.drop_entry(blob);
    }

    /// Wipes everything (lock screen, logout, memory warning).
    pub fn clear(&mut self) {
        self.entries.clear(); // each `Zeroizing` wipes on drop
        self.order.clear();
        self.used = 0;
    }

    fn drop_entry(&mut self, blob: BlobId) {
        if let Some(e) = self.entries.remove(&blob) {
            self.used -= e.data.len() as u64;
        }
    }

    fn bump(&mut self) -> u64 {
        self.next_tick += 1;
        self.next_tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }

    fn temp(max_bytes: u64, ttl_millis: u64) -> PlaintextCache {
        PlaintextCache::new(PlaintextPolicy::Temporary {
            max_bytes,
            ttl_millis,
        })
    }

    #[test]
    fn the_default_keeps_nothing() {
        let mut c = PlaintextCache::new(PlaintextPolicy::default());
        assert!(!c.insert(id(1), vec![1; 10], 0));
        assert!(c.is_empty());
        assert!(c.get(id(1), 0).is_none());
    }

    #[test]
    fn a_hit_returns_the_bytes() {
        let mut c = temp(100, 1_000);
        assert!(c.insert(id(1), vec![5; 10], 0));
        assert_eq!(c.get(id(1), 10), Some(&[5u8; 10][..]));
        assert_eq!(c.used_bytes(), 10);
    }

    #[test]
    fn the_byte_budget_evicts_least_recently_used() {
        let mut c = temp(30, 1_000_000);
        for n in 1..=3 {
            assert!(c.insert(id(n), vec![n; 10], 0));
        }
        // Touch 1 so 2 is now the oldest.
        assert!(c.get(id(1), 1).is_some());
        assert!(c.insert(id(4), vec![4; 10], 2));
        assert!(c.get(id(2), 3).is_none(), "LRU went");
        assert!(c.get(id(1), 3).is_some());
        assert!(c.get(id(3), 3).is_some());
        assert!(c.get(id(4), 3).is_some());
        assert!(c.used_bytes() <= 30);
    }

    #[test]
    fn an_item_larger_than_the_budget_is_never_stored() {
        let mut c = temp(10, 1_000);
        assert!(!c.insert(id(1), vec![0; 11], 0));
        assert_eq!(c.used_bytes(), 0);
    }

    #[test]
    fn entries_expire_by_age() {
        let mut c = temp(100, 50);
        c.insert(id(1), vec![1; 5], 0);
        assert!(c.get(id(1), 49).is_some());
        assert!(c.get(id(1), 50).is_none(), "ttl reached");
        assert_eq!(c.used_bytes(), 0, "wiped, not just hidden");
        c.insert(id(2), vec![2; 5], 100);
        c.insert(id(3), vec![3; 5], 140);
        c.evict_expired(151);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn storage_pressure_can_shrink_to_a_target() {
        let mut c = temp(100, 1_000_000);
        for n in 1..=5 {
            c.insert(id(n), vec![n; 10], 0);
        }
        c.shrink_to(20);
        assert_eq!(c.used_bytes(), 20);
        assert!(c.get(id(5), 1).is_some(), "newest survive");
        assert!(c.get(id(1), 1).is_none());
        c.shrink_to(0);
        assert!(c.is_empty());
    }

    #[test]
    fn reinserting_replaces_without_double_counting() {
        let mut c = temp(100, 1_000);
        c.insert(id(1), vec![1; 40], 0);
        c.insert(id(1), vec![2; 10], 1);
        assert_eq!(c.used_bytes(), 10);
        assert_eq!(c.get(id(1), 2), Some(&[2u8; 10][..]));
    }

    #[test]
    fn clear_empties_everything() {
        let mut c = temp(100, 1_000);
        c.insert(id(1), vec![1; 10], 0);
        c.insert(id(2), vec![2; 10], 0);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.used_bytes(), 0);
        assert!(c.get(id(1), 1).is_none());
    }
}
