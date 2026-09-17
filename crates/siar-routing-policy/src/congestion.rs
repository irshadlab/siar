//! Congestion-ceiling signal — ported from the retired
//! `siar-routing::scheduler::PriorityScheduler::congestion_ceiling`
//! (see `MIGRATION.md`, step 4). next.md §93's original reasoning:
//! "BLE scheduler may send only P0-P3 under congestion" — backlog in
//! a bounded queue's lower tiers is itself a legitimate congestion
//! signal (the same active-queue-management reading networking's own
//! RED/CoDel algorithms use: the sender is producing work faster than
//! the link is draining it), not a placeholder standing in for a real
//! network-level RTT/loss measurement.
//!
//! **Redesigned, not verbatim-ported**: the original `PriorityScheduler`
//! bundled *storage* (seven `VecDeque`s holding real queued items) with
//! this occupancy signal. Real tiered storage in this crate already
//! belongs to `siar_protocol_ext::scheduler::FairScheduler` (see
//! [`crate::dispatch::RouteDispatchQueue`], which wraps it) — adding a
//! second competing queue type here would be exactly the "two crates
//! doing the same job" problem `MIGRATION.md` was written to avoid,
//! not a faithful port. [`CongestionTracker`] is occupancy-only: a
//! caller that already enqueues/dequeues through its own
//! `FairScheduler`/`RouteDispatchQueue` reports each event here
//! (`record_enqueued`/`record_dequeued`), and this type turns the
//! resulting counts into "should I stop admitting low-priority
//! traffic right now" — the same producer-side-signal relationship
//! [`crate::fairness::RoundRobinFairQueue`]'s own doc comment already
//! documents toward [`crate::dispatch::RouteDispatchQueue`], not a
//! second store of the items themselves.
//!
//! Built on `siar_protocol_ext::lifecycle::TrafficPriority` (6 tiers,
//! already this crate's established priority vocabulary via
//! [`crate::dispatch::traffic_priority_for`]) rather than the
//! original's own 7-tier `SchedulePriority` — introducing a third
//! priority enum into one crate (`Priority` and `TrafficPriority`
//! already exist) would be needless abstraction this crate's own
//! stated design posture avoids elsewhere. `TrafficPriority` has no
//! tier as narrow as the original's `P3Voice`/`P4Thumbnail`/`P5Files`
//! split; `Bulk` is this vocabulary's nearest equivalent to the
//! original's `P4Thumbnail`/`P5Files` pairing, `Background` to
//! `P6BackgroundSync` — both throttled tiers, same as the original.

use std::collections::HashMap;

use siar_protocol_ext::lifecycle::TrafficPriority;

/// Occupancy for one caller-owned set of tiered queues (typically one
/// per [`crate::types::TransportKind`], mirroring
/// [`crate::dispatch::PerTransportDispatchQueue`]'s own per-transport
/// split — a stalled Bluetooth queue's congestion reading must not
/// affect a healthy Iroh one).
pub struct CongestionTracker {
    occupancy: HashMap<TrafficPriority, usize>,
    capacity_per_tier: usize,
}

/// next.md §93's own dividing line: which tiers a congested link
/// should stop sending. Non-urgent traffic piling up is what
/// throttling should address; backlog in `Critical`/`Control`/
/// `Interactive` instead means urgent traffic itself is the
/// bottleneck, and narrowing the ceiling further wouldn't help —
/// there's nothing less urgent left to shed.
const THROTTLED_TIERS: [TrafficPriority; 2] = [TrafficPriority::Bulk, TrafficPriority::Background];

impl CongestionTracker {
    pub fn new(capacity_per_tier: usize) -> Self {
        assert!(
            capacity_per_tier >= 1,
            "a zero-capacity tier could never admit anything, making occupancy meaningless"
        );
        Self {
            occupancy: HashMap::new(),
            capacity_per_tier,
        }
    }

    /// Reports that one item was admitted into `tier`'s queue
    /// elsewhere (e.g. a successful [`crate::dispatch::
    /// RouteDispatchQueue::enqueue`]) — this tracker has no queue of
    /// its own to actually hold the item.
    pub fn record_enqueued(&mut self, tier: TrafficPriority) {
        let count = self.occupancy.entry(tier).or_insert(0);
        *count = (*count + 1).min(self.capacity_per_tier);
    }

    /// Reports that one item left `tier`'s queue elsewhere (e.g. a
    /// [`crate::dispatch::RouteDispatchQueue::dispatch_next`] that
    /// returned one). Saturates at zero rather than panicking on an
    /// unbalanced call — a caller that over-reports dequeues (double
    /// counting, or counting one this tracker never saw enqueued)
    /// shouldn't be able to crash congestion detection over it.
    pub fn record_dequeued(&mut self, tier: TrafficPriority) {
        if let Some(count) = self.occupancy.get_mut(&tier) {
            *count = count.saturating_sub(1);
        }
    }

    pub fn occupancy(&self, tier: TrafficPriority) -> usize {
        self.occupancy.get(&tier).copied().unwrap_or(0)
    }

    /// `occupancy_threshold` is the fraction of a tier's
    /// `capacity_per_tier` (`0.0..=1.0`) that counts as "backed up" —
    /// a caller-supplied parameter rather than a hardcoded constant,
    /// since the right threshold depends on `capacity_per_tier`, which
    /// this type doesn't dictate. Returns `Some(TrafficPriority::
    /// Normal)` — "throttle `Bulk`/`Background` only" — once any
    /// throttled tier is at or above the threshold; `None`
    /// (uncongested) otherwise.
    pub fn congestion_ceiling(&self, occupancy_threshold: f32) -> Option<TrafficPriority> {
        let congested = THROTTLED_TIERS.iter().any(|&tier| {
            let fill = self.occupancy(tier) as f32 / self.capacity_per_tier as f32;
            fill >= occupancy_threshold
        });
        if congested {
            Some(TrafficPriority::Normal)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrafficPriority::*;

    #[test]
    fn congestion_ceiling_is_none_when_throttled_tiers_are_below_threshold() {
        let mut tracker = CongestionTracker::new(10);
        tracker.record_enqueued(Bulk);
        assert_eq!(tracker.congestion_ceiling(0.5), None);
    }

    #[test]
    fn congestion_ceiling_triggers_once_a_throttled_tier_crosses_the_threshold() {
        let mut tracker = CongestionTracker::new(4);
        tracker.record_enqueued(Background);
        tracker.record_enqueued(Background);
        // 2/4 == the 0.5 threshold exactly — "at or above", not
        // "strictly above".
        assert_eq!(tracker.congestion_ceiling(0.5), Some(Normal));
    }

    #[test]
    fn congestion_ceiling_ignores_backlog_in_urgent_tiers() {
        let mut tracker = CongestionTracker::new(2);
        // Interactive completely full — real backlog, but not one
        // throttling could ever relieve.
        tracker.record_enqueued(Interactive);
        tracker.record_enqueued(Interactive);
        assert_eq!(tracker.congestion_ceiling(0.5), None);
    }

    #[test]
    fn congestion_ceiling_checks_every_throttled_tier_not_just_the_first() {
        let mut tracker = CongestionTracker::new(4);
        // Bulk stays under threshold; only Background is backed up.
        tracker.record_enqueued(Bulk);
        tracker.record_enqueued(Background);
        tracker.record_enqueued(Background);
        tracker.record_enqueued(Background);
        assert_eq!(tracker.congestion_ceiling(0.5), Some(Normal));
    }

    #[test]
    fn dequeue_relieves_congestion() {
        let mut tracker = CongestionTracker::new(2);
        tracker.record_enqueued(Background);
        tracker.record_enqueued(Background);
        assert_eq!(tracker.congestion_ceiling(0.5), Some(Normal));
        // 1/2 == 0.5 is still "at or above" the threshold — drain
        // fully to confirm the signal actually clears.
        tracker.record_dequeued(Background);
        tracker.record_dequeued(Background);
        assert_eq!(tracker.congestion_ceiling(0.5), None);
    }

    #[test]
    fn dequeue_on_an_empty_tier_saturates_at_zero_rather_than_panicking() {
        let mut tracker = CongestionTracker::new(4);
        tracker.record_dequeued(Bulk);
        assert_eq!(tracker.occupancy(Bulk), 0);
    }

    #[test]
    fn occupancy_is_tracked_independently_per_tier() {
        let mut tracker = CongestionTracker::new(4);
        tracker.record_enqueued(Critical);
        assert_eq!(tracker.occupancy(Critical), 1);
        assert_eq!(tracker.occupancy(Background), 0);
    }

    #[test]
    #[should_panic(expected = "zero-capacity")]
    fn zero_capacity_per_tier_is_rejected_up_front() {
        CongestionTracker::new(0);
    }
}
