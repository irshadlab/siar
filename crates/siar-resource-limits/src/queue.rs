//! §17 "Bounded Queue Principle", §18 "Queue Categories", §19
//! "Priority Queues", §20 "Queue Capacity by Priority", §21
//! "Backpressure Semantics".
//!
//! [`BoundedPriorityQueue`] is deliberately *not* a fairness/dispatch
//! scheduler — that's already `siar_protocol_ext::scheduler::
//! FairScheduler`'s job (weighted round-robin dequeue order, built in
//! an earlier pass). This type answers a different question, the one
//! §17-20 are actually about: *can this item be admitted at all, right
//! now, given each priority tier's own reserved capacity* — §18's "each
//! [queue category] should have independent capacity" and §20's "Bulk
//! traffic must not consume capacity needed for receipts/SOS/control
//! frames." Six fully separate per-tier bounded queues (one `VecDeque`
//! each) make that structural rather than a policy a caller could
//! forget to apply: `Bulk` enqueues can never fill `Critical`'s slots
//! because they're not the same queue. A caller wanting weighted-fair
//! *dispatch order* across tiers feeds admitted items into a
//! `FairScheduler` next, the same layering
//! `siar_routing_policy::dispatch::RouteDispatchQueue` already
//! established (admission is one step, fairness is a separate later
//! step).

use crate::admission::{AdmissionResult, DeferredReason, DropReason, WorkPriority};
use std::collections::VecDeque;

fn tier_index(priority: WorkPriority) -> usize {
    match priority {
        WorkPriority::Critical => 0,
        WorkPriority::Control => 1,
        WorkPriority::Interactive => 2,
        WorkPriority::Normal => 3,
        WorkPriority::Bulk => 4,
        WorkPriority::Background => 5,
    }
}

const TIER_COUNT: usize = 6;
const TIER_ORDER: [WorkPriority; TIER_COUNT] = [
    WorkPriority::Critical,
    WorkPriority::Control,
    WorkPriority::Interactive,
    WorkPriority::Normal,
    WorkPriority::Bulk,
    WorkPriority::Background,
];

/// Per-tier capacity (§20's "Critical reserve / Control reserve /
/// shared normal pool / Bulk limited / Background limited"). §20
/// gives the *shape* of this policy but no concrete numbers for any
/// tier — every field here is a caller-supplied choice, not a
/// transcribed spec value. [`QueueCapacities::conservative_default`]
/// is this module's own reasoned starting point (documented inline),
/// not a spec default, since the spec doesn't give one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueCapacities {
    pub critical: usize,
    pub control: usize,
    pub interactive: usize,
    pub normal: usize,
    pub bulk: usize,
    pub background: usize,
}

impl QueueCapacities {
    pub(crate) fn capacity_of(&self, priority: WorkPriority) -> usize {
        self.get(priority)
    }

    fn get(&self, priority: WorkPriority) -> usize {
        match priority {
            WorkPriority::Critical => self.critical,
            WorkPriority::Control => self.control,
            WorkPriority::Interactive => self.interactive,
            WorkPriority::Normal => self.normal,
            WorkPriority::Bulk => self.bulk,
            WorkPriority::Background => self.background,
        }
    }

    /// A deliberately lopsided default matching §20's own ordering
    /// ("Bulk traffic must not consume capacity needed for receipts/
    /// SOS/control frames"): small-but-guaranteed reserves at the top
    /// two tiers, generously-sized shared middle tiers, and tight caps
    /// at the bottom — not a spec number, this crate's own reasoned
    /// starting point for a caller that hasn't measured its own
    /// workload yet.
    pub fn conservative_default() -> Self {
        Self {
            critical: 32,
            control: 32,
            interactive: 256,
            normal: 256,
            bulk: 64,
            background: 16,
        }
    }
}

/// A bounded, six-tier admission queue (§17-20). Each tier enforces
/// its own capacity independently — see this module's own doc comment
/// for why that, not a shared pool with weights, is the right shape
/// for *admission* as opposed to *dispatch*.
#[derive(Debug, Clone)]
pub struct BoundedPriorityQueue<T> {
    capacities: QueueCapacities,
    tiers: [VecDeque<T>; TIER_COUNT],
}

impl<T> BoundedPriorityQueue<T> {
    pub fn new(capacities: QueueCapacities) -> Self {
        Self {
            capacities,
            tiers: Default::default(),
        }
    }

    /// §21's backpressure decision, applied at the tier `priority`
    /// belongs to: admits if that tier has room; otherwise, per §22's
    /// durable/ephemeral split, `durable` work is [`AdmissionResult::Deferred`]
    /// (the caller should retry the enqueue later — this queue does
    /// *not* hold a separate unbounded "deferred" buffer for it, which
    /// would silently violate §17's own bounded principle) and
    /// non-durable work is [`AdmissionResult::Dropped`] (§22: "can be
    /// dropped when stale" — matching realtime media/typing/presence's
    /// own §58-60 guidance).
    pub fn enqueue(&mut self, priority: WorkPriority, item: T, durable: bool) -> AdmissionResult {
        let idx = tier_index(priority);
        let capacity = self.capacities.get(priority);
        if self.tiers[idx].len() < capacity {
            self.tiers[idx].push_back(item);
            return AdmissionResult::Accepted;
        }

        if durable {
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        } else {
            AdmissionResult::Dropped(DropReason::Stale)
        }
    }

    /// Pops the oldest item from the highest-priority non-empty tier —
    /// strict priority order, not weighted fairness (see this module's
    /// own doc comment for that distinction).
    pub fn dequeue(&mut self) -> Option<T> {
        for &priority in &TIER_ORDER {
            let idx = tier_index(priority);
            if let Some(item) = self.tiers[idx].pop_front() {
                return Some(item);
            }
        }
        None
    }

    pub fn len(&self, priority: WorkPriority) -> usize {
        self.tiers[tier_index(priority)].len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiers.iter().all(VecDeque::is_empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_capacities() -> QueueCapacities {
        QueueCapacities {
            critical: 1,
            control: 1,
            interactive: 1,
            normal: 1,
            bulk: 1,
            background: 1,
        }
    }

    #[test]
    fn each_tier_has_independent_capacity_bulk_full_does_not_block_critical() {
        // §18/§20's whole point: filling one tier must never affect
        // another's admission.
        let mut queue: BoundedPriorityQueue<&str> = BoundedPriorityQueue::new(tiny_capacities());
        assert_eq!(
            queue.enqueue(WorkPriority::Bulk, "bulk-1", true),
            AdmissionResult::Accepted
        );
        assert_eq!(
            queue.enqueue(WorkPriority::Bulk, "bulk-2", true),
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
        // Critical's own reserve is untouched by Bulk being full.
        assert_eq!(
            queue.enqueue(WorkPriority::Critical, "sos", true),
            AdmissionResult::Accepted
        );
    }

    #[test]
    fn durable_work_defers_when_its_tier_is_full() {
        let mut queue: BoundedPriorityQueue<u32> = BoundedPriorityQueue::new(tiny_capacities());
        queue.enqueue(WorkPriority::Normal, 1, true);
        let result = queue.enqueue(WorkPriority::Normal, 2, true);
        assert_eq!(
            result,
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
    }

    #[test]
    fn ephemeral_work_drops_when_its_tier_is_full() {
        // §22: ephemeral work (typing/video frame/presence) is dropped
        // when stale, not deferred/retained.
        let mut queue: BoundedPriorityQueue<u32> = BoundedPriorityQueue::new(tiny_capacities());
        queue.enqueue(WorkPriority::Interactive, 1, false);
        let result = queue.enqueue(WorkPriority::Interactive, 2, false);
        assert_eq!(result, AdmissionResult::Dropped(DropReason::Stale));
    }

    #[test]
    fn dequeue_drains_strictly_highest_priority_tier_first() {
        let mut queue: BoundedPriorityQueue<&str> =
            BoundedPriorityQueue::new(QueueCapacities::conservative_default());
        queue.enqueue(WorkPriority::Background, "bg", true);
        queue.enqueue(WorkPriority::Bulk, "bulk", true);
        queue.enqueue(WorkPriority::Critical, "sos", true);
        queue.enqueue(WorkPriority::Normal, "normal", true);

        assert_eq!(queue.dequeue(), Some("sos"));
        assert_eq!(queue.dequeue(), Some("normal"));
        assert_eq!(queue.dequeue(), Some("bulk"));
        assert_eq!(queue.dequeue(), Some("bg"));
        assert_eq!(queue.dequeue(), None);
    }

    #[test]
    fn fifo_order_within_a_single_tier_is_preserved() {
        let mut queue: BoundedPriorityQueue<u32> =
            BoundedPriorityQueue::new(QueueCapacities::conservative_default());
        queue.enqueue(WorkPriority::Normal, 1, true);
        queue.enqueue(WorkPriority::Normal, 2, true);
        queue.enqueue(WorkPriority::Normal, 3, true);

        assert_eq!(queue.dequeue(), Some(1));
        assert_eq!(queue.dequeue(), Some(2));
        assert_eq!(queue.dequeue(), Some(3));
    }

    #[test]
    fn accepted_enqueue_is_reflected_in_len_and_is_empty() {
        let mut queue: BoundedPriorityQueue<u32> = BoundedPriorityQueue::new(tiny_capacities());
        assert!(queue.is_empty());
        queue.enqueue(WorkPriority::Control, 7, true);
        assert_eq!(queue.len(WorkPriority::Control), 1);
        assert!(!queue.is_empty());
    }
}
