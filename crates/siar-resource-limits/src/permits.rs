//! §38 "Memory Pool", §39 "Buffer Ownership", §40 "Memory Permit",
//! §41 "Stream Permit".
//!
//! §39's own instruction — "Prefer explicit RAII permits... Dropping
//! permit releases accounting. This reduces leaks." — is the actual
//! design constraint this module exists to satisfy: a permit that
//! forgot to be released (an early return, a panic unwind, a bug) must
//! still give its capacity back automatically. That rules out a plain
//! "acquire, remember to call release()" API — this module never
//! offers one.
//!
//! §41: "Same pattern" as §38-40's memory pool, applied to streams —
//! [`StreamLimiter`]/[`StreamPermit`] share an internal bounded-counter
//! primitive with [`BufferPool`]/[`MemoryPermit`] rather than
//! duplicating the same atomic compare-and-swap admission logic twice
//! under two names.

use crate::admission::{AdmissionResult, DeferredReason, DropReason, ResourceOwner};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The shared atomic "how much of a hard cap is currently checked
/// out" primitive both [`BufferPool`] and [`StreamLimiter`] (and,
/// reused directly rather than re-implemented, `cpu_pool`'s worker
/// semaphores) are built on. `Arc` (not a borrowed reference) so a
/// permit can outlive the borrow that created it and still know where
/// to return its capacity on drop — the whole point of the RAII
/// pattern §39 asks for. `pub(crate)` rather than private: this is an
/// internal building block other modules in this crate legitimately
/// share, not a public API surface of its own.
#[derive(Debug, Clone)]
pub(crate) struct BoundedCounter {
    max: u64,
    allocated: Arc<AtomicU64>,
}

impl BoundedCounter {
    pub(crate) fn new(max: u64) -> Self {
        Self {
            max,
            allocated: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Atomic check-and-reserve: succeeds only if reserving `amount`
    /// would not exceed `max`, and the check-then-add happens as one
    /// atomic compare-exchange loop so two concurrent callers can't
    /// both observe headroom and both succeed past the cap (the classic
    /// TOCTOU bug a plain "if allocated + amount <= max { add }" two-step
    /// would have under real concurrency).
    pub(crate) fn try_reserve(&self, amount: u64) -> bool {
        let mut current = self.allocated.load(Ordering::Acquire);
        loop {
            let Some(next) = current.checked_add(amount) else {
                return false;
            };
            if next > self.max {
                return false;
            }
            match self.allocated.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Saturating: an accounting bug that over-releases must not wrap
    /// the counter to ~`u64::MAX` (which would deny every later request).
    pub(crate) fn release(&self, amount: u64) {
        let r = self
            .allocated
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                Some(cur.saturating_sub(amount))
            });
        debug_assert!(
            r.is_ok_and(|prev| prev >= amount),
            "released more than was reserved"
        );
    }

    pub(crate) fn in_use(&self) -> u64 {
        self.allocated.load(Ordering::Acquire)
    }

    pub(crate) fn available(&self) -> u64 {
        self.max.saturating_sub(self.in_use())
    }
}

/// §38's `BufferPool { max_bytes, block_sizes }`. `block_sizes` (the
/// spec's own hint toward fixed-size-class pooling for allocator
/// efficiency) isn't modeled here — this module only builds the
/// *admission* half (bounding total outstanding bytes, §39's RAII
/// release), not a real fixed-size-class allocator, which is a
/// distinct and considerably larger piece of work than this pass
/// attempts.
#[derive(Debug, Clone)]
pub struct BufferPool {
    counter: BoundedCounter,
}

impl BufferPool {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            counter: BoundedCounter::new(max_bytes),
        }
    }

    pub fn available_bytes(&self) -> u64 {
        self.counter.available()
    }

    /// §38: "Borrowing buffer waits or fails according to priority."
    /// This module builds the synchronous *fails* half — `durable`
    /// work that can't be admitted right now gets
    /// [`AdmissionResult::Deferred`] (worth the caller retrying, the
    /// same signal `admission::admit`/`queue::BoundedPriorityQueue`
    /// already use), non-durable work gets [`AdmissionResult::Dropped`].
    /// An actual async "waits" mode belongs with whatever executor
    /// integration a caller brings — not built here, since this crate
    /// otherwise has no async dependency at all and adding one just
    /// for this one function would be a much bigger structural change
    /// than this pass's scope.
    pub fn acquire(
        &self,
        bytes: u64,
        owner: ResourceOwner,
        durable: bool,
    ) -> Result<MemoryPermit, AdmissionResult> {
        if self.counter.try_reserve(bytes) {
            Ok(MemoryPermit {
                bytes,
                owner,
                counter: self.counter.clone(),
            })
        } else if durable {
            Err(AdmissionResult::Deferred(DeferredReason::AwaitingBudget))
        } else {
            Err(AdmissionResult::Dropped(DropReason::Stale))
        }
    }
}

/// §40, extended only with the internal `counter` handle a real RAII
/// release needs — `bytes`/`owner` are exactly §40's own two fields.
/// "No manual `release()` ideally" (§40's own closing line) is why
/// there is no public `release` method here at all: returning capacity
/// happens exclusively via [`Drop`].
#[derive(Debug)]
pub struct MemoryPermit {
    bytes: u64,
    owner: ResourceOwner,
    counter: BoundedCounter,
}

impl MemoryPermit {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn owner(&self) -> &ResourceOwner {
        &self.owner
    }
}

impl Drop for MemoryPermit {
    fn drop(&mut self) {
        self.counter.release(self.bytes);
    }
}

/// §41: "Same pattern" as the memory pool, for stream slots instead of
/// bytes.
#[derive(Debug, Clone)]
pub struct StreamLimiter {
    counter: BoundedCounter,
}

impl StreamLimiter {
    pub fn new(max_streams: u64) -> Self {
        Self {
            counter: BoundedCounter::new(max_streams),
        }
    }

    pub fn available_streams(&self) -> u64 {
        self.counter.available()
    }

    pub fn acquire(
        &self,
        owner: ResourceOwner,
        durable: bool,
    ) -> Result<StreamPermit, AdmissionResult> {
        if self.counter.try_reserve(1) {
            Ok(StreamPermit {
                owner,
                counter: self.counter.clone(),
            })
        } else if durable {
            Err(AdmissionResult::Deferred(DeferredReason::AwaitingBudget))
        } else {
            Err(AdmissionResult::Dropped(DropReason::Stale))
        }
    }
}

#[derive(Debug)]
pub struct StreamPermit {
    owner: ResourceOwner,
    counter: BoundedCounter,
}

impl StreamPermit {
    pub fn owner(&self) -> &ResourceOwner {
        &self.owner
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        self.counter.release(1);
    }
}

#[cfg(test)]
mod release_tests {
    use super::*;

    #[test]
    #[cfg(not(debug_assertions))]
    fn over_release_saturates_at_zero() {
        let c = BoundedCounter::new(10);
        c.release(5);
        assert_eq!(c.in_use(), 0);
        assert!(c.try_reserve(10));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "released more than was reserved")]
    fn over_release_is_loud_in_debug_builds() {
        BoundedCounter::new(10).release(5);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_within_capacity_succeeds() {
        let pool = BufferPool::new(1024);
        let permit = pool.acquire(512, ResourceOwner::Files, true).unwrap();
        assert_eq!(permit.bytes(), 512);
        assert_eq!(pool.available_bytes(), 512);
    }

    #[test]
    fn dropping_a_permit_releases_its_bytes_automatically() {
        // §39's whole point: no manual release() call anywhere in
        // this test.
        let pool = BufferPool::new(1024);
        {
            let _permit = pool.acquire(1024, ResourceOwner::Files, true).unwrap();
            assert_eq!(pool.available_bytes(), 0);
        }
        assert_eq!(pool.available_bytes(), 1024);
    }

    #[test]
    fn durable_acquire_past_capacity_defers() {
        let pool = BufferPool::new(100);
        let _held = pool.acquire(100, ResourceOwner::Files, true).unwrap();

        let result = pool.acquire(1, ResourceOwner::Files, true).unwrap_err();
        assert_eq!(
            result,
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
    }

    #[test]
    fn non_durable_acquire_past_capacity_drops() {
        let pool = BufferPool::new(100);
        let _held = pool.acquire(100, ResourceOwner::Files, true).unwrap();

        let result = pool.acquire(1, ResourceOwner::Files, false).unwrap_err();
        assert_eq!(result, AdmissionResult::Dropped(DropReason::Stale));
    }

    #[test]
    fn a_rejected_acquire_does_not_reserve_any_capacity() {
        let pool = BufferPool::new(100);
        assert!(pool.acquire(101, ResourceOwner::Files, true).is_err());
        // Full capacity still available — the failed attempt above
        // must not have partially reserved anything.
        assert_eq!(pool.available_bytes(), 100);
    }

    #[test]
    fn stream_limiter_follows_the_same_acquire_release_pattern() {
        let limiter = StreamLimiter::new(2);
        let a = limiter.acquire(ResourceOwner::Calls, true).unwrap();
        let b = limiter.acquire(ResourceOwner::Calls, true).unwrap();
        assert_eq!(limiter.available_streams(), 0);

        let rejected = limiter.acquire(ResourceOwner::Calls, false).unwrap_err();
        assert_eq!(rejected, AdmissionResult::Dropped(DropReason::Stale));

        drop(a);
        assert_eq!(limiter.available_streams(), 1);
        let c = limiter.acquire(ResourceOwner::Calls, true).unwrap();
        assert_eq!(c.owner(), &ResourceOwner::Calls);
        drop(b);
        drop(c);
        assert_eq!(limiter.available_streams(), 2);
    }

    #[test]
    fn concurrent_reservations_never_overshoot_the_cap() {
        // Exercises `BoundedCounter::try_reserve`'s compare-exchange
        // loop under real thread contention, not just single-threaded
        // sequential calls — the TOCTOU bug this module's own doc
        // comment warns against would show up here as more than 50
        // permits being admitted against a 50-byte pool. Permits are
        // kept alive in `results` until after the assertions run —
        // returning only a bool from each thread would drop the
        // permit inside the thread closure and release its byte back
        // immediately, silently hiding an overshoot instead of
        // catching one.
        use std::thread;

        let pool = Arc::new(BufferPool::new(50));
        let handles: Vec<_> = (0..80)
            .map(|_| {
                let pool = Arc::clone(&pool);
                thread::spawn(move || pool.acquire(1, ResourceOwner::Core, false))
            })
            .collect();

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let admitted: Vec<_> = results.into_iter().filter_map(Result::ok).collect();

        assert_eq!(admitted.len(), 50);
        assert_eq!(pool.available_bytes(), 0);
        drop(admitted);
        assert_eq!(pool.available_bytes(), 50);
    }
}
