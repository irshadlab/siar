//! Bounded seen-bundle deduplication — ported from the retired
//! `siar-dtn::dedup::SeenBundles` (see `MIGRATION.md`, step 1). Real
//! gap this crate never had an equivalent for: next.md §31's original
//! reasoning applies exactly as much to a [`crate::bundle::DtnBundle`]
//! as it ever did to the old `MeshBundle` — "Without this, mesh
//! forwarding creates storms." A node needs to remember which
//! [`crate::types::BundleId`]s it's already processed or forwarded so
//! it doesn't re-process the same bundle arriving via multiple paths
//! (§189 Phase 3's still-unbuilt encounter protocol will produce
//! exactly that "same bundle, different peer" situation the moment it
//! exists).
//!
//! Bounded by entry count, not wall-clock age — same reasoning the
//! original carried over from `siar_calls::jitter::JitterBuffer`/
//! `siar_transport_ble::reassembly::ReassemblyBuffer`: this stays pure
//! and testable without mocking time, and unbounded memory growth from
//! a flood of distinct ids is what actually needs bounding here, not
//! elapsed time specifically.

use std::collections::{HashSet, VecDeque};

use crate::types::BundleId;

pub struct SeenBundles {
    capacity: usize,
    /// Oldest-first insertion order, so eviction always drops the
    /// least-recently-seen id — a `HashSet` alone can't offer that
    /// ordering.
    order: VecDeque<BundleId>,
    set: HashSet<BundleId>,
}

impl SeenBundles {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity >= 1,
            "a zero-capacity seen-set can never remember anything, defeating deduplication entirely"
        );
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            set: HashSet::with_capacity(capacity),
        }
    }

    /// Returns `true` if `id` had already been seen — the caller
    /// should drop this bundle rather than process or forward it
    /// again. Returns `false` on first sighting, in which case `id` is
    /// now recorded (evicting the oldest entry first if already at
    /// capacity).
    pub fn check_and_record(&mut self, id: BundleId) -> bool {
        if self.set.contains(&id) {
            return true;
        }
        if self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        self.order.push_back(id);
        self.set.insert(id);
        false
    }

    /// Read-only peek — `true` if `id` has already been recorded, same
    /// answer [`Self::check_and_record`] would give, without recording
    /// anything on a first sighting.
    pub fn contains(&self, id: BundleId) -> bool {
        self.set.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_is_not_seen_and_gets_recorded() {
        let mut seen = SeenBundles::new(4);
        let id = BundleId::new();
        assert!(!seen.check_and_record(id));
        assert!(seen.contains(id));
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn second_sighting_of_the_same_id_is_reported_seen() {
        let mut seen = SeenBundles::new(4);
        let id = BundleId::new();
        assert!(!seen.check_and_record(id));
        assert!(seen.check_and_record(id));
        // Still only one entry — re-seeing doesn't duplicate.
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn contains_does_not_record_on_a_miss() {
        let seen = SeenBundles::new(4);
        let id = BundleId::new();
        assert!(!seen.contains(id));
        assert!(seen.is_empty());
    }

    #[test]
    fn eviction_drops_the_oldest_id_once_at_capacity() {
        let mut seen = SeenBundles::new(2);
        let a = BundleId::new();
        let b = BundleId::new();
        let c = BundleId::new();
        assert!(!seen.check_and_record(a));
        assert!(!seen.check_and_record(b));
        // Capacity 2 already holds a, b — recording c evicts a.
        assert!(!seen.check_and_record(c));
        assert!(!seen.contains(a), "a should have been evicted");
        assert!(seen.contains(b));
        assert!(seen.contains(c));
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn re_seeing_an_id_does_not_refresh_its_eviction_order() {
        // Matches the original `siar-dtn` implementation's semantics
        // exactly: `check_and_record` on an already-seen id returns
        // early and never touches `order`, so it is not "renewed" —
        // it can still be evicted on its original schedule.
        let mut seen = SeenBundles::new(2);
        let a = BundleId::new();
        let b = BundleId::new();
        let c = BundleId::new();
        seen.check_and_record(a);
        seen.check_and_record(b);
        // Re-seeing `a` does not move it to the back of the queue.
        assert!(seen.check_and_record(a));
        seen.check_and_record(c);
        assert!(
            !seen.contains(a),
            "a's original eviction order still applies"
        );
        assert!(seen.contains(b));
        assert!(seen.contains(c));
    }

    #[test]
    #[should_panic(expected = "zero-capacity")]
    fn zero_capacity_is_rejected_up_front() {
        SeenBundles::new(0);
    }
}
