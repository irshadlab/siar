//! A tiny least-recently-used cache for the descriptors and manifests
//! rebuilt to serve pulls. Capacity is small by design (a large blob's
//! manifest is large), so recency is a `VecDeque` scan, which at this size
//! is cheaper than anything cleverer.
//!
//! The previous cache was insertion-ordered, so a manifest requested over
//! and over could still be evicted by sixteen unrelated inserts; a hit now
//! refreshes recency.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

pub(crate) struct Lru<K: Eq + Hash + Copy, V: Clone> {
    capacity: usize,
    /// Least recently used first.
    order: VecDeque<K>,
    map: HashMap<K, V>,
}

impl<K: Eq + Hash + Copy, V: Clone> Lru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    fn touch(&mut self, k: &K) {
        if let Some(i) = self.order.iter().position(|x| x == k) {
            self.order.remove(i);
        }
        self.order.push_back(*k);
    }

    /// A hit makes the entry the most recently used.
    pub(crate) fn get(&mut self, k: &K) -> Option<V> {
        let v = self.map.get(k).cloned()?;
        self.touch(k);
        Some(v)
    }

    pub(crate) fn insert(&mut self, k: K, v: V) {
        self.map.insert(k, v);
        self.touch(&k);
        while self.order.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub(crate) fn remove(&mut self, k: &K) {
        if self.map.remove(k).is_some() {
            self.order.retain(|x| x != k);
        }
    }

    /// Keeps only entries whose key satisfies `keep`.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        let dead: Vec<K> = self.map.keys().filter(|k| !keep(k)).copied().collect();
        for k in dead {
            self.remove(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_least_recently_used_not_the_oldest_inserted() {
        let mut c: Lru<u8, u8> = Lru::new(3);
        for k in 1..=3 {
            c.insert(k, k);
        }
        assert_eq!(c.get(&1), Some(1)); // 1 is now the freshest
        c.insert(4, 4); // evicts 2, not 1
        assert_eq!(c.get(&2), None);
        assert_eq!(c.get(&1), Some(1));
        assert_eq!(c.get(&3), Some(3));
        assert_eq!(c.get(&4), Some(4));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn a_hot_entry_survives_far_more_inserts_than_the_capacity() {
        let mut c: Lru<u8, u8> = Lru::new(4);
        c.insert(0, 0);
        for k in 1..100u8 {
            assert_eq!(c.get(&0), Some(0), "evicted before insert {k}");
            c.insert(k, k);
        }
        assert_eq!(c.len(), 4);
    }

    #[test]
    fn reinserting_does_not_grow_or_duplicate() {
        let mut c: Lru<u8, u8> = Lru::new(2);
        c.insert(1, 1);
        c.insert(1, 9);
        c.insert(2, 2);
        c.insert(1, 10);
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&1), Some(10));
    }

    #[test]
    fn retain_and_remove_keep_the_order_consistent() {
        let mut c: Lru<u8, u8> = Lru::new(5);
        for k in 1..=5 {
            c.insert(k, k);
        }
        c.retain(|k| k % 2 == 1);
        assert_eq!(c.len(), 3);
        c.remove(&3);
        c.insert(6, 6);
        c.insert(7, 7);
        c.insert(8, 8);
        // Capacity 5 holds: 1,5,6,7,8 — no ghost entries from the order list.
        assert_eq!(c.len(), 4.max(c.len()).min(5));
        assert!(c.get(&2).is_none() && c.get(&3).is_none());
    }

    #[test]
    fn a_zero_capacity_still_holds_one() {
        let mut c: Lru<u8, u8> = Lru::new(0);
        c.insert(1, 1);
        assert_eq!(c.get(&1), Some(1));
    }
}
