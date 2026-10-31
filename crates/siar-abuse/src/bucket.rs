//! A tiny, deterministic rate limiter: a token bucket in integer milliseconds.
//! No floating point, no wall clock — the caller supplies `now_ms`, so tests
//! are exact and a clock that jumps backwards can never mint credit.

/// Allows `capacity` operations in a burst, then one per `period_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket {
    capacity: u32,
    period_ms: u64,
    /// Stored credit in milliseconds, in `0..=capacity * period_ms`.
    credit_ms: u64,
    last_ms: u64,
}

impl Bucket {
    /// A full bucket. `period_ms == 0` means "unlimited".
    #[must_use]
    pub const fn new(capacity: u32, period_ms: u64, now_ms: u64) -> Self {
        Self {
            capacity,
            period_ms,
            credit_ms: capacity as u64 * period_ms,
            last_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: u64) {
        // A backwards clock adds nothing (and never subtracts).
        let dt = now_ms.saturating_sub(self.last_ms);
        self.last_ms = self.last_ms.max(now_ms);
        let max = u64::from(self.capacity).saturating_mul(self.period_ms);
        self.credit_ms = self.credit_ms.saturating_add(dt).min(max);
    }

    /// Try to spend `n` operations. `Err(retry_after_ms)` leaves state unchanged.
    pub fn take(&mut self, n: u32, now_ms: u64) -> Result<(), u64> {
        if self.period_ms == 0 {
            return Ok(());
        }
        self.refill(now_ms);
        let cost = self.period_ms.saturating_mul(u64::from(n.max(1)));
        if self.credit_ms >= cost {
            self.credit_ms -= cost;
            Ok(())
        } else {
            Err(cost - self.credit_ms)
        }
    }

    /// Operations currently available (for tests and diagnostics).
    #[must_use]
    pub fn available(&self) -> u32 {
        if self.period_ms == 0 {
            u32::MAX
        } else {
            u32::try_from(self.credit_ms / self.period_ms).unwrap_or(u32::MAX)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_steady_rate() {
        let mut b = Bucket::new(3, 1_000, 0);
        for _ in 0..3 {
            assert!(b.take(1, 0).is_ok());
        }
        assert_eq!(b.take(1, 0), Err(1_000));
        assert_eq!(
            b.take(1, 400),
            Err(600),
            "partial refill reports the exact wait"
        );
        assert!(b.take(1, 1_000).is_ok());
        assert!(b.take(1, 1_000).is_err());
    }

    #[test]
    fn credit_is_capped_so_idleness_does_not_bank_unlimited_burst() {
        let mut b = Bucket::new(2, 1_000, 0);
        assert_eq!(b.available(), 2);
        b.take(2, 0).unwrap();
        assert!(b.take(1, 1_000_000).is_ok());
        assert!(b.take(1, 1_000_000).is_ok());
        assert!(
            b.take(1, 1_000_000).is_err(),
            "still only `capacity` after a very long wait"
        );
    }

    #[test]
    fn a_backwards_clock_mints_nothing() {
        let mut b = Bucket::new(1, 1_000, 10_000);
        b.take(1, 10_000).unwrap();
        assert!(b.take(1, 0).is_err());
        assert!(b.take(1, 5_000).is_err());
    }

    #[test]
    fn a_failed_take_changes_nothing() {
        let mut b = Bucket::new(1, 1_000, 0);
        b.take(1, 0).unwrap();
        let before = b;
        assert!(b.take(1, 0).is_err());
        assert_eq!(b, before);
    }

    #[test]
    fn zero_period_is_unlimited() {
        let mut b = Bucket::new(0, 0, 0);
        for _ in 0..10_000 {
            assert!(b.take(1, 0).is_ok());
        }
    }
}
