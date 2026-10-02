//! §158 lock contention: sharded counters for hot-path *statistics*.
//!
//! Correctness first: limits and permits stay on exact atomics
//! (`BoundedCounter`, CAS-checked) — a sharded counter can't enforce a
//! cap without a global check. Use this for counters that are written on
//! every operation but read rarely (bytes sent, frames dropped, retries):
//! each thread bumps its own cache-line-padded shard, so writers never
//! contend or false-share, and a read sums the shards.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[repr(align(64))]
#[derive(Debug, Default)]
struct Shard(AtomicU64);

static NEXT_SHARD_HINT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static HINT: usize = NEXT_SHARD_HINT.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug)]
pub struct ShardedCounter {
    shards: Box<[Shard]>,
}

impl ShardedCounter {
    /// `shards` is typically the core count; it is clamped to at least 1.
    pub fn new(shards: usize) -> Self {
        Self {
            shards: (0..shards.max(1)).map(|_| Shard::default()).collect(),
        }
    }

    #[inline]
    pub fn add(&self, n: u64) {
        let i = HINT.with(|h| *h) % self.shards.len();
        self.shards[i].0.fetch_add(n, Ordering::Relaxed);
    }

    /// Exact once writers are quiescent; a consistent lower bound while
    /// they run. Saturates instead of wrapping.
    pub fn sum(&self) -> u64 {
        let total: u128 = self
            .shards
            .iter()
            .map(|s| u128::from(s.0.load(Ordering::Relaxed)))
            .sum();
        u64::try_from(total).unwrap_or(u64::MAX)
    }

    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_adds_sum_exactly() {
        let c = Arc::new(ShardedCounter::new(8));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let c = Arc::clone(&c);
                std::thread::spawn(move || {
                    for _ in 0..100_000 {
                        c.add(1);
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(c.sum(), 800_000);
    }

    #[test]
    fn shards_are_cache_line_padded() {
        assert_eq!(std::mem::align_of::<Shard>(), 64);
        assert_eq!(std::mem::size_of::<Shard>(), 64);
    }

    #[test]
    fn zero_shards_is_clamped_and_sum_saturates() {
        let c = ShardedCounter::new(0);
        assert_eq!(c.shard_count(), 1);
        c.add(u64::MAX);
        c.add(5); // wraps inside the shard (documented: u64 stats)
        let two = ShardedCounter::new(2);
        two.shards[0].0.store(u64::MAX, Ordering::Relaxed);
        two.shards[1].0.store(u64::MAX, Ordering::Relaxed);
        assert_eq!(two.sum(), u64::MAX);
    }
}
