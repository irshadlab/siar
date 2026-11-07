//! §70 rate limiting: a token bucket per operator. Time is passed in, so the
//! behaviour is deterministic under test.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct RateConfig {
    /// Burst size.
    pub capacity: u32,
    /// Tokens added per second.
    pub per_second: u32,
}

impl Default for RateConfig {
    fn default() -> Self {
        Self {
            capacity: 20,
            per_second: 5,
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Only known operators ever get a bucket (unknown peers are refused before
/// this is consulted), so the map is bounded by the operator table.
pub struct RateLimiter {
    cfg: RateConfig,
    buckets: Mutex<HashMap<[u8; 32], Bucket>>,
}

impl RateLimiter {
    #[must_use]
    pub fn new(cfg: RateConfig) -> Self {
        Self {
            cfg,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Take one token for `who`; `false` means "over the limit".
    pub fn allow(&self, who: &[u8; 32], now: Instant) -> bool {
        let mut m = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let b = m.entry(*who).or_insert(Bucket {
            tokens: f64::from(self.cfg.capacity),
            last: now,
        });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens =
            (b.tokens + elapsed * f64::from(self.cfg.per_second)).min(f64::from(self.cfg.capacity));
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// How long until the next token (for tests/diagnostics).
    #[must_use]
    pub fn refill_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.cfg.per_second.max(1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_throttle_then_refill() {
        let rl = RateLimiter::new(RateConfig {
            capacity: 3,
            per_second: 1,
        });
        let who = [1u8; 32];
        let t0 = Instant::now();
        assert!(rl.allow(&who, t0) && rl.allow(&who, t0) && rl.allow(&who, t0));
        assert!(!rl.allow(&who, t0), "burst exhausted");
        assert!(!rl.allow(&who, t0 + Duration::from_millis(900)), "not yet");
        assert!(
            rl.allow(&who, t0 + Duration::from_millis(1000)),
            "one token back"
        );
        assert!(!rl.allow(&who, t0 + Duration::from_millis(1000)));
        // Long idle refills only up to capacity, never beyond.
        let later = t0 + Duration::from_secs(3600);
        assert!(rl.allow(&who, later) && rl.allow(&who, later) && rl.allow(&who, later));
        assert!(!rl.allow(&who, later));
    }

    #[test]
    fn operators_do_not_share_a_bucket() {
        let rl = RateLimiter::new(RateConfig {
            capacity: 1,
            per_second: 1,
        });
        let t = Instant::now();
        assert!(rl.allow(&[1; 32], t));
        assert!(!rl.allow(&[1; 32], t));
        assert!(rl.allow(&[2; 32], t), "another operator is unaffected");
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_mint_tokens() {
        let rl = RateLimiter::new(RateConfig {
            capacity: 1,
            per_second: 100,
        });
        let t = Instant::now() + Duration::from_secs(10);
        assert!(rl.allow(&[1; 32], t));
        assert!(!rl.allow(&[1; 32], t - Duration::from_secs(5)));
    }
}
