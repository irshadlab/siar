//! §37-39 scheduler, §80-81 encounter capacity/deadline.
//!
//! Per encounter: strict priority for `Critical` and `ControlAck`, then
//! deficit-round-robin (weighted fairness, §37 "avoid total starvation")
//! over the remaining classes, all under the encounter's byte budget and
//! the transport's per-item size ceiling (§38). Short contacts (§81) shrink
//! the eligible item size so one giant file never eats a brief window.

use std::time::Duration;

use crate::types::BundleId;

/// Declaration order is scheduling order (§37).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrafficClass {
    Critical,
    ControlAck,
    Text,
    SmallEmergencyMedia,
    Voice,
    Thumbnail,
    BulkChunk,
}

impl TrafficClass {
    const ALL: [TrafficClass; 7] = [
        Self::Critical,
        Self::ControlAck,
        Self::Text,
        Self::SmallEmergencyMedia,
        Self::Voice,
        Self::Thumbnail,
        Self::BulkChunk,
    ];

    fn is_strict(self) -> bool {
        matches!(self, Self::Critical | Self::ControlAck)
    }

    /// DRR weight for the fair-share classes; bulk keeps a nonzero share.
    fn weight(self) -> u64 {
        match self {
            Self::Critical | Self::ControlAck => 0,
            Self::Text => 8,
            Self::SmallEmergencyMedia => 6,
            Self::Voice => 4,
            Self::Thumbnail => 2,
            Self::BulkChunk => 1,
        }
    }
}

/// §39, tunable via [`SchedulerPolicy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SizeClass {
    Tiny,
    Small,
    Medium,
    Large,
}

impl SizeClass {
    pub fn of(bytes: u64) -> Self {
        match bytes {
            0..=4_095 => Self::Tiny,
            4_096..=65_535 => Self::Small,
            65_536..=1_048_575 => Self::Medium,
            _ => Self::Large,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandwidthClass {
    /// BLE-like: small bundles only.
    Minimal,
    Low,
    Medium,
    /// Wi-Fi Direct / Iroh class.
    High,
}

impl BandwidthClass {
    /// Largest item this class should carry (§38).
    pub fn max_size_class(self) -> SizeClass {
        match self {
            Self::Minimal => SizeClass::Tiny,
            Self::Low => SizeClass::Small,
            Self::Medium => SizeClass::Medium,
            Self::High => SizeClass::Large,
        }
    }

    /// Nominal sustained bytes/second, used only to bound a budget by
    /// predicted contact duration. Deliberately conservative.
    pub fn nominal_bytes_per_sec(self) -> u64 {
        match self {
            Self::Minimal => 8 * 1024,
            Self::Low => 64 * 1024,
            Self::Medium => 512 * 1024,
            Self::High => 4 * 1024 * 1024,
        }
    }
}

/// §80.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncounterCapacity {
    pub max_bytes: u64,
    pub estimated_duration: Option<Duration>,
    pub bandwidth_class: BandwidthClass,
}

impl EncounterCapacity {
    /// Byte budget = min(declared cap, rate × predicted duration).
    pub fn byte_budget(&self) -> u64 {
        match self.estimated_duration {
            Some(d) => {
                let by_time = self
                    .bandwidth_class
                    .nominal_bytes_per_sec()
                    .saturating_mul(d.as_secs().max(1));
                self.max_bytes.min(by_time)
            }
            None => self.max_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerPolicy {
    /// DRR quantum per unit weight, bytes.
    pub quantum_bytes: u64,
    /// A contact predicted shorter than this only moves ≤ `Small` items (§81).
    pub short_contact: Duration,
}

impl Default for SchedulerPolicy {
    fn default() -> Self {
        Self {
            quantum_bytes: 2048,
            short_contact: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulableItem {
    pub id: BundleId,
    pub class: TrafficClass,
    pub size_bytes: u64,
}

/// Returns IDs in transmission order. Within a class, input order is
/// preserved — callers pre-sort by [`crate::inventory::select_missing`].
pub fn plan_encounter(
    items: &[SchedulableItem],
    capacity: &EncounterCapacity,
    policy: &SchedulerPolicy,
) -> Vec<BundleId> {
    let mut ceiling = capacity.bandwidth_class.max_size_class();
    if capacity
        .estimated_duration
        .is_some_and(|d| d < policy.short_contact)
    {
        ceiling = ceiling.min(SizeClass::Small);
    }
    let mut remaining = capacity.byte_budget();
    let mut out = Vec::new();

    let mut queues: Vec<std::collections::VecDeque<&SchedulableItem>> = TrafficClass::ALL
        .iter()
        .map(|c| {
            items
                .iter()
                .filter(|i| i.class == *c && SizeClass::of(i.size_bytes) <= ceiling)
                .collect()
        })
        .collect();

    // Strict classes first. A head that doesn't fit ends that class (no
    // reordering inside a class).
    for (idx, c) in TrafficClass::ALL.iter().enumerate() {
        if !c.is_strict() {
            continue;
        }
        while let Some(head) = queues[idx].front() {
            if head.size_bytes > remaining {
                break;
            }
            remaining -= head.size_bytes;
            out.push(head.id);
            queues[idx].pop_front();
        }
    }

    // Weighted DRR over the rest.
    let mut deficit = [0u64; 7];
    loop {
        let mut active: Vec<usize> = Vec::new();
        for (idx, c) in TrafficClass::ALL.iter().enumerate() {
            if c.is_strict() {
                continue;
            }
            if queues[idx]
                .front()
                .is_some_and(|h| h.size_bytes <= remaining)
            {
                active.push(idx);
            }
        }
        if active.is_empty() {
            break;
        }
        let mut progressed = false;
        for &idx in &active {
            deficit[idx] += TrafficClass::ALL[idx].weight() * policy.quantum_bytes;
            while let Some(head) = queues[idx].front() {
                if head.size_bytes > deficit[idx] || head.size_bytes > remaining {
                    break;
                }
                deficit[idx] -= head.size_bytes;
                remaining -= head.size_bytes;
                out.push(head.id);
                queues[idx].pop_front();
                progressed = true;
            }
            if queues[idx].is_empty() {
                deficit[idx] = 0;
            }
        }
        if !progressed {
            // Fast-forward: grant the rounds the closest head still needs,
            // so a large item can't spin the loop for millions of rounds.
            let rounds = active
                .iter()
                .filter_map(|&idx| {
                    let head = queues[idx].front()?;
                    let per_round = TrafficClass::ALL[idx].weight() * policy.quantum_bytes;
                    (per_round > 0).then(|| {
                        head.size_bytes
                            .saturating_sub(deficit[idx])
                            .div_ceil(per_round)
                    })
                })
                .min()
                .unwrap_or(1)
                .max(1);
            for &idx in &active {
                deficit[idx] = deficit[idx].saturating_add(
                    rounds.saturating_sub(1)
                        * TrafficClass::ALL[idx].weight()
                        * policy.quantum_bytes,
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(class: TrafficClass, size: u64) -> SchedulableItem {
        SchedulableItem {
            id: BundleId::new(),
            class,
            size_bytes: size,
        }
    }

    fn cap(bytes: u64, bw: BandwidthClass, secs: Option<u64>) -> EncounterCapacity {
        EncounterCapacity {
            max_bytes: bytes,
            estimated_duration: secs.map(Duration::from_secs),
            bandwidth_class: bw,
        }
    }

    #[test]
    fn critical_then_ack_precede_everything_else() {
        let bulk = item(TrafficClass::BulkChunk, 100);
        let text = item(TrafficClass::Text, 100);
        let ack = item(TrafficClass::ControlAck, 100);
        let crit = item(TrafficClass::Critical, 100);
        let order = plan_encounter(
            &[bulk.clone(), text.clone(), ack.clone(), crit.clone()],
            &cap(1 << 20, BandwidthClass::High, None),
            &SchedulerPolicy::default(),
        );
        assert_eq!(&order[..2], &[crit.id, ack.id]);
        assert_eq!(order.len(), 4);
    }

    #[test]
    fn bulk_is_not_starved_by_a_flood_of_text() {
        let mut items: Vec<_> = (0..200).map(|_| item(TrafficClass::Text, 1000)).collect();
        let bulk = item(TrafficClass::BulkChunk, 1000);
        items.push(bulk.clone());
        let order = plan_encounter(
            &items,
            &cap(100_000, BandwidthClass::High, None),
            &SchedulerPolicy::default(),
        );
        let pos = order
            .iter()
            .position(|id| *id == bulk.id)
            .expect("bulk scheduled");
        assert!(pos < 150, "bulk at {pos} — starved behind text");
    }

    #[test]
    fn ble_class_never_schedules_more_than_tiny_items() {
        let tiny = item(TrafficClass::Text, 1000);
        let medium = item(TrafficClass::Text, 100_000);
        let order = plan_encounter(
            &[medium, tiny.clone()],
            &cap(1 << 30, BandwidthClass::Minimal, None),
            &SchedulerPolicy::default(),
        );
        assert_eq!(order, vec![tiny.id]);
    }

    #[test]
    fn short_contact_skips_the_giant_file_and_sends_small_critical_first() {
        let giant = item(TrafficClass::BulkChunk, 2 * 1024 * 1024);
        let small = item(TrafficClass::Critical, 2_000);
        let order = plan_encounter(
            &[giant, small.clone()],
            &cap(1 << 30, BandwidthClass::High, Some(5)),
            &SchedulerPolicy::default(),
        );
        assert_eq!(order, vec![small.id]);
    }

    #[test]
    fn byte_budget_is_respected_and_time_bounds_it() {
        let c = cap(1 << 30, BandwidthClass::Minimal, Some(2));
        assert_eq!(c.byte_budget(), 16 * 1024);
        let items: Vec<_> = (0..20).map(|_| item(TrafficClass::Text, 3_000)).collect();
        let order = plan_encounter(&items, &c, &SchedulerPolicy::default());
        assert_eq!(order.len(), 5); // 5 * 3000 = 15000 <= 16384 < 18000
    }

    #[test]
    fn huge_item_terminates_quickly() {
        let big = item(TrafficClass::BulkChunk, 900_000);
        let order = plan_encounter(
            std::slice::from_ref(&big),
            &cap(u64::MAX, BandwidthClass::High, None),
            &SchedulerPolicy::default(),
        );
        assert_eq!(order, vec![big.id]);
    }
}
