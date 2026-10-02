//! §78 admission by cost estimate, §79 estimate accuracy.
//!
//! "Do not trust application-supplied estimate blindly": a declared cost
//! is (1) checked against availability *before* any work starts, (2)
//! scaled up by how badly that owner has underestimated before, and (3)
//! trued-up against the actual cost, so a reservation neither starves
//! others (over-reserve) nor lets an owner overrun (under-reserve).

use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cost {
    pub memory_bytes: u64,
    pub storage_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CostReject {
    #[error("needs {needed} storage bytes, {available} available")]
    Storage { needed: u64, available: u64 },
    #[error("needs {needed} memory bytes, {available} available")]
    Memory { needed: u64, available: u64 },
}

/// §78: reject before hashing/reading if it cannot possibly fit.
pub fn admit_by_cost(
    cost: Cost,
    memory_available: u64,
    storage_available: u64,
) -> Result<(), CostReject> {
    if cost.storage_bytes > storage_available {
        return Err(CostReject::Storage {
            needed: cost.storage_bytes,
            available: storage_available,
        });
    }
    if cost.memory_bytes > memory_available {
        return Err(CostReject::Memory {
            needed: cost.memory_bytes,
            available: memory_available,
        });
    }
    Ok(())
}

/// Result of comparing what was reserved with what was really used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrueUp {
    Exact,
    /// Over-reserved: return this many bytes to the pool.
    Release(u64),
    /// Under-reserved: must acquire this many more (may fail).
    NeedMore(u64),
}

pub fn true_up(reserved: u64, actual: u64) -> TrueUp {
    match actual.cmp(&reserved) {
        std::cmp::Ordering::Equal => TrueUp::Exact,
        std::cmp::Ordering::Less => TrueUp::Release(reserved - actual),
        std::cmp::Ordering::Greater => TrueUp::NeedMore(actual - reserved),
    }
}

/// Fixed-point (1/256) multiplier: 256 = trust the estimate, 1024 = 4×.
const ONE: u64 = 256;
const MAX_FACTOR: u64 = 4 * ONE;

/// Learns, per owner, how much real cost exceeds the declared cost.
/// Only underestimation raises the factor; an owner who over-declares
/// earns back at most 1.0× — never a discount below what it declared.
#[derive(Debug)]
pub struct EstimateLedger<K> {
    factors: HashMap<K, u64>,
    max_owners: usize,
}

impl<K: Hash + Eq + Clone> EstimateLedger<K> {
    pub fn new(max_owners: usize) -> Self {
        Self {
            factors: HashMap::new(),
            max_owners: max_owners.max(1),
        }
    }

    /// Cost to reserve for a declaration (saturating).
    pub fn effective(&self, owner: &K, declared: u64) -> u64 {
        let f = self.factors.get(owner).copied().unwrap_or(ONE);
        (u128::from(declared) * u128::from(f) / u128::from(ONE)).min(u128::from(u64::MAX)) as u64
    }

    /// Feed back what the operation really cost.
    pub fn record(&mut self, owner: K, declared: u64, actual: u64) {
        if declared == 0 && actual == 0 {
            return;
        }
        // A zero declaration with real cost is the worst underestimate.
        let sample = if declared == 0 {
            MAX_FACTOR
        } else {
            (u128::from(actual) * u128::from(ONE) / u128::from(declared))
                .clamp(u128::from(ONE), u128::from(MAX_FACTOR)) as u64
        };
        if !self.factors.contains_key(&owner) && self.factors.len() >= self.max_owners {
            // Forget the best-behaved owner (lowest factor) to stay bounded.
            if let Some(k) = self
                .factors
                .iter()
                .min_by_key(|(_, f)| **f)
                .map(|(k, _)| k.clone())
            {
                self.factors.remove(&k);
            }
        }
        let f = self.factors.entry(owner).or_insert(ONE);
        *f = (*f * 3 + sample) / 4; // EWMA, new sample weight 1/4
        *f = (*f).clamp(ONE, MAX_FACTOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_gb_file_is_rejected_before_any_work() {
        let c = Cost {
            memory_bytes: 1 << 20,
            storage_bytes: 5 << 30,
        };
        assert_eq!(
            admit_by_cost(c, 1 << 30, 1 << 30),
            Err(CostReject::Storage {
                needed: 5 << 30,
                available: 1 << 30
            })
        );
        assert!(admit_by_cost(
            Cost {
                memory_bytes: 1,
                storage_bytes: 1
            },
            1,
            1
        )
        .is_ok());
    }

    #[test]
    fn true_up_both_directions() {
        assert_eq!(true_up(100, 100), TrueUp::Exact);
        assert_eq!(true_up(100, 40), TrueUp::Release(60));
        assert_eq!(true_up(100, 130), TrueUp::NeedMore(30));
    }

    #[test]
    fn habitual_underestimators_are_scaled_up_but_capped() {
        let mut l = EstimateLedger::new(8);
        assert_eq!(l.effective(&"app", 1_000), 1_000, "no history: trusted");
        for _ in 0..20 {
            l.record("app", 1_000, 10_000);
        }
        let e = l.effective(&"app", 1_000);
        assert!(e > 3_500 && e <= 4_000, "converges to the 4x cap, got {e}");
        assert_eq!(l.effective(&"other", 1_000), 1_000, "per-owner");
    }

    #[test]
    fn accurate_or_over_declaring_owners_never_get_a_discount() {
        let mut l = EstimateLedger::new(8);
        for _ in 0..50 {
            l.record("app", 10_000, 10);
        }
        assert_eq!(l.effective(&"app", 1_000), 1_000);
    }

    #[test]
    fn factor_recovers_when_the_owner_improves() {
        let mut l = EstimateLedger::new(8);
        for _ in 0..10 {
            l.record("app", 100, 400);
        }
        let high = l.effective(&"app", 100);
        for _ in 0..30 {
            l.record("app", 100, 100);
        }
        assert!(l.effective(&"app", 100) < high / 2);
    }

    #[test]
    fn zero_declaration_with_real_cost_and_boundedness() {
        let mut l = EstimateLedger::new(2);
        l.record("a", 0, 5);
        assert!(l.effective(&"a", 100) > 100);
        l.record("b", 10, 10);
        l.record("c", 10, 50);
        assert!(l.factors.len() <= 2);
        assert_eq!(l.effective(&"x", u64::MAX), u64::MAX);
    }
}
