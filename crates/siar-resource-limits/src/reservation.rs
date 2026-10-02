//! §91-95 multi-resource reservation, ordered acquisition, deadlock
//! avoidance, permit bundles and expiry; §96-97 cancellation and panic
//! safety.
//!
//! A transfer needs memory *and* a stream *and* a file descriptor.
//! Acquiring them one at a time in caller-chosen order is how two tasks
//! each holding half of what the other needs deadlock. Here:
//! - requests are merged per kind and acquired in the one canonical
//!   [`ResourceKind`] order (§92-93), regardless of how the caller listed
//!   them;
//! - acquisition never blocks and is all-or-nothing: on any shortfall
//!   every already-taken amount is returned before the error is reported
//!   (no hold-and-wait, the third deadlock condition);
//! - the [`PermitBundle`] releases everything in `Drop`, so cancellation
//!   (a dropped future) and unwinding panics cannot leak capacity.

use std::sync::Arc;

use crate::permits::BoundedCounter;
use crate::types::ResourceKind;

#[derive(Debug, Clone)]
pub struct ResourcePools {
    counters: Arc<[BoundedCounter; ResourceKind::COUNT]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReservationError {
    #[error("{kind:?} exhausted: requested {requested}, available {available}")]
    Exhausted {
        kind: ResourceKind,
        requested: u64,
        available: u64,
    },
    #[error("requested amount for {0:?} overflows")]
    Overflow(ResourceKind),
}

impl ResourcePools {
    /// Kinds not listed get capacity 0: an unconfigured resource fails
    /// closed rather than silently being unlimited.
    pub fn new(limits: &[(ResourceKind, u64)]) -> Self {
        let mut max = [0u64; ResourceKind::COUNT];
        for &(k, v) in limits {
            max[k.index()] = v;
        }
        Self {
            counters: Arc::new(max.map(BoundedCounter::new)),
        }
    }

    pub fn available(&self, kind: ResourceKind) -> u64 {
        self.counters[kind.index()].available()
    }

    pub fn in_use(&self, kind: ResourceKind) -> u64 {
        self.counters[kind.index()].in_use()
    }

    pub fn acquire(
        &self,
        request: &[(ResourceKind, u64)],
        deadline_millis: Option<u64>,
    ) -> Result<PermitBundle, ReservationError> {
        let mut amounts = [0u64; ResourceKind::COUNT];
        for &(k, n) in request {
            let slot = &mut amounts[k.index()];
            *slot = slot.checked_add(n).ok_or(ReservationError::Overflow(k))?;
        }
        for (i, kind) in ResourceKind::ALL.iter().enumerate() {
            let n = amounts[i];
            if n == 0 || self.counters[i].try_reserve(n) {
                continue;
            }
            let available = self.counters[i].available();
            // Roll back what we already hold (all lower-index kinds).
            for (counter, &held) in self.counters.iter().zip(&amounts).take(i) {
                if held != 0 {
                    counter.release(held);
                }
            }
            return Err(ReservationError::Exhausted {
                kind: *kind,
                requested: n,
                available,
            });
        }
        Ok(PermitBundle {
            pools: Arc::clone(&self.counters),
            amounts,
            deadline_millis,
        })
    }
}

#[derive(Debug)]
#[must_use = "dropping the bundle releases every reserved resource"]
pub struct PermitBundle {
    pools: Arc<[BoundedCounter; ResourceKind::COUNT]>,
    amounts: [u64; ResourceKind::COUNT],
    deadline_millis: Option<u64>,
}

impl PermitBundle {
    pub fn amount(&self, kind: ResourceKind) -> u64 {
        self.amounts[kind.index()]
    }

    /// §95: a bundle held past its deadline should be revoked by its
    /// owner (a stalled holder otherwise pins capacity forever).
    pub fn is_expired(&self, now_millis: u64) -> bool {
        self.deadline_millis.is_some_and(|d| now_millis >= d)
    }

    /// Returns one kind early (e.g. the stream, once the body is sent)
    /// without waiting for the whole bundle to drop. Saturates.
    pub fn release_kind(&mut self, kind: ResourceKind) -> u64 {
        let n = std::mem::take(&mut self.amounts[kind.index()]);
        if n != 0 {
            self.pools[kind.index()].release(n);
        }
        n
    }
}

impl Drop for PermitBundle {
    fn drop(&mut self) {
        for (i, n) in self.amounts.iter().enumerate() {
            if *n != 0 {
                self.pools[i].release(*n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ResourceKind::*;

    fn pools() -> ResourcePools {
        ResourcePools::new(&[(Memory, 100), (Streams, 2), (FileDescriptors, 4)])
    }

    #[test]
    fn all_or_nothing_with_full_rollback() {
        let p = pools();
        let held = p.acquire(&[(Streams, 2)], None).unwrap();
        // Memory would succeed, Streams cannot: nothing may stay reserved.
        let err = p.acquire(&[(Memory, 50), (Streams, 1)], None).unwrap_err();
        assert_eq!(
            err,
            ReservationError::Exhausted {
                kind: Streams,
                requested: 1,
                available: 0
            }
        );
        assert_eq!(
            p.in_use(Memory),
            0,
            "memory taken before the failure was rolled back"
        );
        drop(held);
        assert!(p.acquire(&[(Memory, 50), (Streams, 1)], None).is_ok());
    }

    #[test]
    fn caller_order_cannot_change_acquisition_order_or_outcome() {
        let p = pools();
        let a = p
            .acquire(&[(FileDescriptors, 1), (Memory, 10), (Streams, 1)], None)
            .unwrap();
        let b = p
            .acquire(&[(Streams, 1), (Memory, 10), (FileDescriptors, 1)], None)
            .unwrap();
        assert_eq!(a.amount(Memory), b.amount(Memory));
        assert_eq!(p.in_use(Streams), 2);
    }

    #[test]
    fn dropping_releases_everything_including_on_panic() {
        let p = pools();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _b = p.acquire(&[(Memory, 80), (Streams, 2)], None).unwrap();
            panic!("task blew up");
        }));
        assert!(r.is_err());
        assert_eq!(p.in_use(Memory), 0);
        assert_eq!(p.in_use(Streams), 0);
    }

    #[test]
    fn duplicate_kinds_are_merged_and_overflow_is_an_error() {
        let p = pools();
        let b = p.acquire(&[(Memory, 30), (Memory, 30)], None).unwrap();
        assert_eq!(b.amount(Memory), 60);
        assert_eq!(
            p.acquire(&[(Memory, u64::MAX), (Memory, 1)], None)
                .unwrap_err(),
            ReservationError::Overflow(Memory)
        );
    }

    #[test]
    fn unconfigured_resources_fail_closed() {
        let p = pools();
        assert!(matches!(
            p.acquire(&[(Network, 1)], None),
            Err(ReservationError::Exhausted { kind: Network, .. })
        ));
    }

    #[test]
    fn expiry_and_early_partial_release() {
        let p = pools();
        let mut b = p.acquire(&[(Memory, 10), (Streams, 1)], Some(500)).unwrap();
        assert!(!b.is_expired(499) && b.is_expired(500));
        assert_eq!(b.release_kind(Streams), 1);
        assert_eq!(b.release_kind(Streams), 0, "second release is a no-op");
        assert_eq!(p.in_use(Streams), 0);
        assert_eq!(p.in_use(Memory), 10);
    }

    #[test]
    fn concurrent_acquirers_never_exceed_the_cap() {
        let p = ResourcePools::new(&[(Memory, 1_000), (Streams, 10)]);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let mut held = 0u32;
                    for _ in 0..500 {
                        if let Ok(b) = p.acquire(&[(Memory, 100), (Streams, 1)], None) {
                            assert!(p.in_use(Streams) <= 10 && p.in_use(Memory) <= 1_000);
                            held += 1;
                            drop(b);
                        }
                    }
                    held
                })
            })
            .collect();
        let total: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert!(total > 0);
        assert_eq!((p.in_use(Memory), p.in_use(Streams)), (0, 0));
    }
}
