//! §99 starvation detection, §100 aging, §98 fairness.
//!
//! [`crate::queue::BoundedPriorityQueue`] dequeues strictly by priority:
//! under sustained Normal traffic a Bulk item never leaves. This queue
//! keeps the same per-tier capacities but lets an item's *effective*
//! rank improve by one tier per `step_millis` waited — never above
//! `floor`, so aged bulk can't outrank Critical/Control work (§56's
//! "strict priority risks" fixed without creating inversion the other
//! way). Dequeue inspects only the six tier heads: O(tiers), no scan.

use std::collections::VecDeque;

use crate::admission::{AdmissionResult, DeferredReason, DropReason, WorkPriority};
use crate::queue::QueueCapacities;

const TIERS: usize = 6;
const BY_INDEX: [WorkPriority; TIERS] = [
    WorkPriority::Critical,
    WorkPriority::Control,
    WorkPriority::Interactive,
    WorkPriority::Normal,
    WorkPriority::Bulk,
    WorkPriority::Background,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgingPolicy {
    /// One tier of promotion per this much waiting. 0 disables aging.
    pub step_millis: u64,
    /// Aging never promotes an item above this priority.
    pub floor: WorkPriority,
}

impl Default for AgingPolicy {
    fn default() -> Self {
        Self {
            step_millis: 2_000,
            floor: WorkPriority::Interactive,
        }
    }
}

#[derive(Debug)]
pub struct AgingQueue<T> {
    capacities: QueueCapacities,
    policy: AgingPolicy,
    tiers: [VecDeque<(u64, T)>; TIERS],
}

impl<T> AgingQueue<T> {
    pub fn new(capacities: QueueCapacities, policy: AgingPolicy) -> Self {
        Self {
            capacities,
            policy,
            tiers: Default::default(),
        }
    }

    pub fn enqueue(
        &mut self,
        priority: WorkPriority,
        item: T,
        durable: bool,
        now_millis: u64,
    ) -> AdmissionResult {
        let idx = priority as usize;
        if self.tiers[idx].len() < self.capacities.capacity_of(priority) {
            self.tiers[idx].push_back((now_millis, item));
            return AdmissionResult::Accepted;
        }
        if durable {
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        } else {
            AdmissionResult::Dropped(DropReason::Stale)
        }
    }

    fn effective_rank(&self, idx: usize, enqueued: u64, now: u64) -> usize {
        let floor = self.policy.floor as usize;
        if idx <= floor || self.policy.step_millis == 0 {
            return idx;
        }
        let steps =
            (now.saturating_sub(enqueued) / self.policy.step_millis).min(TIERS as u64) as usize;
        idx.saturating_sub(steps).max(floor)
    }

    pub fn dequeue(&mut self, now_millis: u64) -> Option<T> {
        let mut best: Option<(usize, u64, usize)> = None; // (rank, enqueued, tier)
        for (idx, tier) in self.tiers.iter().enumerate() {
            if let Some(&(enq, _)) = tier.front() {
                let cand = (self.effective_rank(idx, enq, now_millis), enq, idx);
                if best.is_none_or(|b| cand < b) {
                    best = Some(cand);
                }
            }
        }
        let (_, _, tier) = best?;
        self.tiers[tier].pop_front().map(|(_, v)| v)
    }

    /// §99: the priority tier whose head has waited longest past
    /// `threshold_millis`, with its wait — a starvation signal for
    /// diagnostics/alerting (head-of-line is the oldest item per tier).
    pub fn most_starved(
        &self,
        now_millis: u64,
        threshold_millis: u64,
    ) -> Option<(WorkPriority, u64)> {
        self.tiers
            .iter()
            .enumerate()
            .filter_map(|(idx, t)| {
                let waited = now_millis.saturating_sub(t.front()?.0);
                (waited > threshold_millis).then_some((BY_INDEX[idx], waited))
            })
            .max_by_key(|&(_, w)| w)
    }

    pub fn len(&self) -> usize {
        self.tiers.iter().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.tiers.iter().all(VecDeque::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    fn q() -> AgingQueue<&'static str> {
        AgingQueue::new(
            QueueCapacities::conservative_default(),
            AgingPolicy {
                step_millis: 100,
                floor: Interactive,
            },
        )
    }

    #[test]
    fn without_waiting_it_is_plain_strict_priority() {
        let mut q = q();
        q.enqueue(Bulk, "bulk", true, 0);
        q.enqueue(Normal, "normal", true, 0);
        q.enqueue(Critical, "crit", true, 0);
        assert_eq!(q.dequeue(0), Some("crit"));
        assert_eq!(q.dequeue(0), Some("normal"));
        assert_eq!(q.dequeue(0), Some("bulk"));
    }

    #[test]
    fn a_long_waiting_bulk_item_is_not_starved_by_fresh_normal_traffic() {
        let mut q = q();
        q.enqueue(Bulk, "old bulk", true, 0);
        q.enqueue(Normal, "fresh normal", true, 1_000);
        // old bulk waited 1000ms = 10 steps -> promoted to the floor
        // (Interactive), which beats fresh Normal.
        assert_eq!(q.dequeue(1_000), Some("old bulk"));
        assert_eq!(q.dequeue(1_000), Some("fresh normal"));
    }

    #[test]
    fn aging_never_lets_bulk_outrank_critical_or_control() {
        let mut q = q();
        q.enqueue(Background, "ancient", true, 0);
        q.enqueue(Control, "ctl", true, 1_000_000);
        q.enqueue(Critical, "crit", true, 1_000_000);
        assert_eq!(q.dequeue(1_000_000), Some("crit"));
        assert_eq!(q.dequeue(1_000_000), Some("ctl"));
        assert_eq!(q.dequeue(1_000_000), Some("ancient"));
    }

    #[test]
    fn equal_rank_is_fifo_by_enqueue_time() {
        let mut q = q();
        q.enqueue(Interactive, "second", true, 5);
        q.enqueue(Bulk, "first", true, 0);
        // At t=400 the bulk item reached the floor with an older timestamp.
        assert_eq!(q.dequeue(400), Some("first"));
    }

    #[test]
    fn capacity_is_still_per_tier_and_durable_defers_ephemeral_drops() {
        let mut q = AgingQueue::new(
            QueueCapacities {
                critical: 1,
                control: 1,
                interactive: 1,
                normal: 1,
                bulk: 1,
                background: 1,
            },
            AgingPolicy::default(),
        );
        assert_eq!(q.enqueue(Bulk, "a", true, 0), AdmissionResult::Accepted);
        assert_eq!(
            q.enqueue(Bulk, "b", true, 0),
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
        assert_eq!(
            q.enqueue(Bulk, "c", false, 0),
            AdmissionResult::Dropped(DropReason::Stale)
        );
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn starvation_is_reported_for_the_longest_waiting_tier() {
        let mut q = q();
        q.enqueue(Bulk, "x", true, 100);
        q.enqueue(Normal, "y", true, 900);
        assert_eq!(q.most_starved(1_000, 500), Some((Bulk, 900)));
        assert_eq!(q.most_starved(1_000, 5_000), None);
    }
}
