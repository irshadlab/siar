//! §58-61: realtime drop policy, typing/presence drop, coalescing and
//! the latest-value channel.
//!
//! For ephemeral state the newest value makes every older one worthless,
//! so the right backpressure response is replacement, not queueing.
//! All three structures are strictly bounded.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// §61: a one-slot channel. `publish` overwrites; `take` consumes.
#[derive(Debug)]
pub struct LatestValue<T> {
    slot: Option<T>,
    superseded: u64,
}

impl<T> Default for LatestValue<T> {
    fn default() -> Self {
        Self {
            slot: None,
            superseded: 0,
        }
    }
}

impl<T> LatestValue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if an unconsumed value was replaced.
    pub fn publish(&mut self, v: T) -> bool {
        let replaced = self.slot.replace(v).is_some();
        if replaced {
            self.superseded = self.superseded.saturating_add(1);
        }
        replaced
    }

    pub fn take(&mut self) -> Option<T> {
        self.slot.take()
    }

    pub fn peek(&self) -> Option<&T> {
        self.slot.as_ref()
    }

    /// How many values were dropped unseen (diagnostic, §106).
    pub fn superseded_count(&self) -> u64 {
        self.superseded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoalesceOutcome {
    Inserted,
    /// An older value for the same key was replaced in place.
    Replaced,
    /// A new key forced out the oldest key (which is lost).
    InsertedEvictedOldest,
}

/// §59-60: per-key latest value (typing indicator per conversation,
/// presence per peer). Keys keep first-seen order, so a chatty key can't
/// jump the queue by being updated.
#[derive(Debug)]
pub struct KeyedCoalescer<K, V> {
    map: HashMap<K, V>,
    order: VecDeque<K>,
    max_keys: usize,
}

impl<K: Hash + Eq + Clone, V> KeyedCoalescer<K, V> {
    pub fn new(max_keys: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            max_keys: max_keys.max(1),
        }
    }

    pub fn push(&mut self, key: K, value: V) -> CoalesceOutcome {
        if let Some(slot) = self.map.get_mut(&key) {
            *slot = value;
            return CoalesceOutcome::Replaced;
        }
        let mut evicted = false;
        if self.map.len() >= self.max_keys {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
                evicted = true;
            }
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
        if evicted {
            CoalesceOutcome::InsertedEvictedOldest
        } else {
            CoalesceOutcome::Inserted
        }
    }

    /// Takes everything, first-seen order.
    pub fn drain(&mut self) -> Vec<(K, V)> {
        let mut out = Vec::with_capacity(self.order.len());
        for k in self.order.drain(..) {
            if let Some(v) = self.map.remove(&k) {
                out.push((k, v));
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// §58: realtime media/audio frames. Drop-*oldest* (a late audio frame is
/// worse than a missing one) and drop anything past `max_age_millis`.
#[derive(Debug)]
pub struct RealtimeRing<T> {
    q: VecDeque<(u64, T)>,
    capacity: usize,
    max_age_millis: u64,
    dropped: u64,
}

impl<T> RealtimeRing<T> {
    pub fn new(capacity: usize, max_age_millis: u64) -> Self {
        Self {
            q: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
            max_age_millis,
            dropped: 0,
        }
    }

    pub fn push(&mut self, at_millis: u64, frame: T) {
        if self.q.len() >= self.capacity {
            self.q.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.q.push_back((at_millis, frame));
    }

    /// Next frame still fresh at `now`; stale ones are discarded, not played.
    pub fn pop_fresh(&mut self, now_millis: u64) -> Option<T> {
        while let Some((at, frame)) = self.q.pop_front() {
            if now_millis.saturating_sub(at) <= self.max_age_millis {
                return Some(frame);
            }
            self.dropped = self.dropped.saturating_add(1);
        }
        None
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.q.len()
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_value_keeps_only_the_newest_and_counts_the_rest() {
        let mut c = LatestValue::new();
        assert!(!c.publish(1));
        assert!(c.publish(2));
        assert!(c.publish(3));
        assert_eq!(c.take(), Some(3));
        assert_eq!(c.take(), None);
        assert_eq!(c.superseded_count(), 2);
    }

    #[test]
    fn coalescer_replaces_in_place_and_keeps_first_seen_order() {
        let mut c = KeyedCoalescer::new(8);
        assert_eq!(c.push("a", 1), CoalesceOutcome::Inserted);
        c.push("b", 1);
        assert_eq!(c.push("a", 2), CoalesceOutcome::Replaced);
        assert_eq!(c.drain(), vec![("a", 2), ("b", 1)]);
        assert!(c.is_empty());
    }

    #[test]
    fn coalescer_is_bounded_by_evicting_the_oldest_key() {
        let mut c = KeyedCoalescer::new(2);
        c.push(1, ());
        c.push(2, ());
        assert_eq!(c.push(3, ()), CoalesceOutcome::InsertedEvictedOldest);
        assert_eq!(c.len(), 2);
        let keys: Vec<_> = c.drain().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec![2, 3]);
    }

    #[test]
    fn realtime_ring_drops_oldest_when_full_and_stale_on_read() {
        let mut r = RealtimeRing::new(3, 100);
        for i in 0..5u64 {
            r.push(i * 10, i);
        }
        assert_eq!(r.len(), 3);
        assert_eq!(r.dropped_count(), 2);
        // frames 2,3,4 at t=20,30,40; at now=135 only >=35 is fresh -> frame 4
        assert_eq!(r.pop_fresh(135), Some(4));
        assert_eq!(r.pop_fresh(135), None);
        assert_eq!(r.dropped_count(), 4);
    }
}
