//! §9 global runtime budget, §10 sub-budgets, §11 budget borrowing.
//!
//! One `GlobalBudget` per runtime; every subsystem draws from it, so none
//! assumes it owns the machine. Each subsystem has a *guarantee*
//! (its sub-budget). Using more than the guarantee is *borrowing*:
//! allowed only while borrowing is enabled, within a per-subsystem borrow
//! cap, and never past the global hard ceiling. Borrowing is revocable:
//! when a subsystem inside its own guarantee is refused because borrowers
//! filled the pool, the error names who is over their guarantee so the
//! runtime can make them give it back.

use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subsystem {
    Messaging,
    Files,
    Dtn,
    Calls,
    Extensions,
}

impl Subsystem {
    pub const COUNT: usize = 5;
    pub const ALL: [Subsystem; Self::COUNT] = [
        Self::Messaging,
        Self::Files,
        Self::Dtn,
        Self::Calls,
        Self::Extensions,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    #[error("global ceiling reached")]
    GlobalCeiling {
        /// Subsystems currently above their guarantee, with the excess —
        /// what could be revoked to satisfy a within-guarantee request.
        /// Empty if the request itself was a borrow.
        reclaimable: Vec<(Subsystem, u64)>,
    },
    #[error("borrowing is disabled under pressure")]
    BorrowingDisabled,
    #[error("borrow cap for this subsystem exceeded")]
    BorrowCapExceeded,
    #[error("amount overflows")]
    Overflow,
}

#[derive(Debug)]
struct State {
    used: [u64; Subsystem::COUNT],
    borrowing_enabled: bool,
}

#[derive(Debug)]
struct Inner {
    global_max: u64,
    guaranteed: [u64; Subsystem::COUNT],
    max_borrow: u64,
    state: Mutex<State>,
}

#[derive(Debug, Clone)]
pub struct GlobalBudget(Arc<Inner>);

#[derive(Debug)]
#[must_use = "dropping the permit returns the bytes to the global pool"]
pub struct BudgetPermit {
    inner: Arc<Inner>,
    sub: Subsystem,
    amount: u64,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl GlobalBudget {
    /// `guaranteed` need not sum to `global_max` (under-commit leaves a
    /// shared slack pool; over-commit is allowed because the global
    /// ceiling is what is enforced). `max_borrow` bounds how far any one
    /// subsystem may exceed its guarantee.
    pub fn new(global_max: u64, guaranteed: [u64; Subsystem::COUNT], max_borrow: u64) -> Self {
        Self(Arc::new(Inner {
            global_max,
            guaranteed,
            max_borrow,
            state: Mutex::new(State {
                used: [0; Subsystem::COUNT],
                borrowing_enabled: true,
            }),
        }))
    }

    pub fn try_acquire(&self, sub: Subsystem, amount: u64) -> Result<BudgetPermit, BudgetError> {
        let i = sub as usize;
        let mut st = lock(&self.0.state);
        let total: u64 = st.used.iter().sum();
        let new_total = total.checked_add(amount).ok_or(BudgetError::Overflow)?;
        let new_used = st.used[i]
            .checked_add(amount)
            .ok_or(BudgetError::Overflow)?;
        let within_guarantee = new_used <= self.0.guaranteed[i];

        if !within_guarantee {
            if !st.borrowing_enabled {
                return Err(BudgetError::BorrowingDisabled);
            }
            if new_used - self.0.guaranteed[i] > self.0.max_borrow {
                return Err(BudgetError::BorrowCapExceeded);
            }
        }
        if new_total > self.0.global_max {
            let reclaimable = if within_guarantee {
                Subsystem::ALL
                    .iter()
                    .filter_map(|s| {
                        let excess =
                            st.used[*s as usize].saturating_sub(self.0.guaranteed[*s as usize]);
                        (*s != sub && excess > 0).then_some((*s, excess))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            return Err(BudgetError::GlobalCeiling { reclaimable });
        }
        st.used[i] = new_used;
        drop(st);
        Ok(BudgetPermit {
            inner: Arc::clone(&self.0),
            sub,
            amount,
        })
    }

    /// §11: under pressure, stop *new* borrowing. Existing borrows stay
    /// until revoked/dropped.
    pub fn set_borrowing_enabled(&self, enabled: bool) {
        lock(&self.0.state).borrowing_enabled = enabled;
    }

    pub fn used(&self, sub: Subsystem) -> u64 {
        lock(&self.0.state).used[sub as usize]
    }

    pub fn total_used(&self) -> u64 {
        lock(&self.0.state).used.iter().sum()
    }

    /// Bytes `sub` currently holds beyond its guarantee.
    pub fn borrowed(&self, sub: Subsystem) -> u64 {
        self.used(sub)
            .saturating_sub(self.0.guaranteed[sub as usize])
    }
}

impl BudgetPermit {
    pub fn amount(&self) -> u64 {
        self.amount
    }

    pub fn subsystem(&self) -> Subsystem {
        self.sub
    }

    /// Give back `n` bytes now (revocation, or early release). Saturates.
    pub fn shrink_by(&mut self, n: u64) -> u64 {
        let give = n.min(self.amount);
        if give > 0 {
            let mut st = lock(&self.inner.state);
            let u = &mut st.used[self.sub as usize];
            *u = u.saturating_sub(give);
            self.amount -= give;
        }
        give
    }
}

impl Drop for BudgetPermit {
    fn drop(&mut self) {
        self.shrink_by(self.amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Subsystem::*;

    // spec §10 example: 256 MiB = 96 files + 64 calls + 32 msg + 32 dtn + 32 other
    const MIB: u64 = 1 << 20;
    fn budget() -> GlobalBudget {
        GlobalBudget::new(
            256 * MIB,
            [32 * MIB, 96 * MIB, 32 * MIB, 64 * MIB, 32 * MIB],
            64 * MIB,
        )
    }

    #[test]
    fn within_guarantee_always_succeeds_when_pool_has_room() {
        let b = budget();
        let p = b.try_acquire(Files, 96 * MIB).unwrap();
        assert_eq!(b.borrowed(Files), 0);
        assert_eq!(p.amount(), 96 * MIB);
    }

    #[test]
    fn borrowing_is_capped_and_never_passes_the_global_ceiling() {
        let b = budget();
        let _a = b.try_acquire(Files, 96 * MIB + 64 * MIB).unwrap();
        assert_eq!(b.borrowed(Files), 64 * MIB);
        assert_eq!(
            b.try_acquire(Files, 1).unwrap_err(),
            BudgetError::BorrowCapExceeded
        );
        let _c = b.try_acquire(Calls, 64 * MIB).unwrap();
        let _d = b.try_acquire(Dtn, 32 * MIB).unwrap();
        // 160+64+32 = 256 = ceiling
        assert!(matches!(
            b.try_acquire(Extensions, 1),
            Err(BudgetError::GlobalCeiling { .. })
        ));
        assert!(b.total_used() <= 256 * MIB);
    }

    #[test]
    fn a_lender_refused_inside_its_guarantee_is_told_who_to_revoke() {
        let b = budget();
        let mut files = b.try_acquire(Files, 160 * MIB).unwrap(); // 64 MiB borrowed
        let _calls = b.try_acquire(Calls, 64 * MIB).unwrap();
        let _dtn = b.try_acquire(Dtn, 32 * MIB).unwrap();
        // Messaging wants its own guaranteed 32 MiB but borrowers ate it.
        let err = b.try_acquire(Messaging, 32 * MIB).unwrap_err();
        assert_eq!(
            err,
            BudgetError::GlobalCeiling {
                reclaimable: vec![(Files, 64 * MIB)]
            }
        );
        // Revoke, then the lender succeeds.
        assert_eq!(files.shrink_by(64 * MIB), 64 * MIB);
        assert!(b.try_acquire(Messaging, 32 * MIB).is_ok());
    }

    #[test]
    fn disabling_borrowing_blocks_new_borrows_but_not_guaranteed_use() {
        let b = budget();
        b.set_borrowing_enabled(false);
        assert_eq!(
            b.try_acquire(Messaging, 33 * MIB).unwrap_err(),
            BudgetError::BorrowingDisabled
        );
        assert!(b.try_acquire(Messaging, 32 * MIB).is_ok());
    }

    #[test]
    fn drop_returns_budget_and_shrink_saturates() {
        let b = budget();
        let mut p = b.try_acquire(Dtn, 10).unwrap();
        assert_eq!(p.shrink_by(1_000), 10);
        assert_eq!(p.shrink_by(1), 0);
        drop(p);
        assert_eq!(b.total_used(), 0);
        assert_eq!(
            b.try_acquire(Dtn, u64::MAX).unwrap_err(),
            BudgetError::BorrowCapExceeded
        );
    }

    #[test]
    fn overflow_is_an_error_not_a_wrap() {
        let b = GlobalBudget::new(u64::MAX, [u64::MAX; 5], u64::MAX);
        let _p = b.try_acquire(Files, u64::MAX - 1).unwrap();
        assert_eq!(b.try_acquire(Files, 5).unwrap_err(), BudgetError::Overflow);
    }

    #[test]
    fn concurrent_use_never_exceeds_the_ceiling() {
        let b = GlobalBudget::new(1_000, [200; 5], 500);
        let hs: Vec<_> = Subsystem::ALL
            .into_iter()
            .map(|s| {
                let b = b.clone();
                std::thread::spawn(move || {
                    for _ in 0..2_000 {
                        if let Ok(p) = b.try_acquire(s, 150) {
                            assert!(b.total_used() <= 1_000);
                            drop(p);
                        }
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(b.total_used(), 0);
    }
}
