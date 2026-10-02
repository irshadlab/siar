//! §52 "Traffic Shaping", §53 "Token Bucket".
//!
//! §53's own sketch is conceptual only — a bare `rate`/`burst` struct
//! with no method bodies. This module is the real implementation: a
//! [`TokenBucket`] that actually refills over time and actually
//! rejects consumption past its available tokens, usable directly for
//! any of §53's three named purposes ("per-peer bytes/sec, per-extension
//! bandwidth, unknown-peer intake").
//!
//! Time is passed in explicitly as milliseconds (`now_millis: u64`),
//! matching every other time-sensitive module already in this
//! workspace (`siar_dtn_bundle::bundle::DtnBundle::is_expired`,
//! `siar_dtn_bundle::forwarding::decide_forwarding`,
//! `siar_event_log::ids::Timestamp`) — never a real-clock `Instant`
//! internally, so callers (and this module's own tests) can drive it
//! deterministically instead of sleeping in tests or fighting
//! non-monotonic wall-clock reads on a real device.

use serde::{Deserialize, Serialize};

/// A byte/token-rate limiter: refills continuously at `refill_rate`
/// tokens per second, up to `capacity` (§53's "burst"), and rejects
/// any [`TokenBucket::try_consume`] call it can't fully satisfy
/// (partial consumption is never silently allowed — a caller that
/// wants to send less on rejection asks again with a smaller amount).
///
/// Internally tracks *milli-tokens* (`tokens × 1000`) rather than
/// tokens directly, so `refill`'s core arithmetic —
/// `elapsed_millis × refill_rate` — is exact integer math with no
/// per-tick rounding drift, instead of needing `f64` and accepting
/// the accumulated-error risk that comes with it over a long-running
/// bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenBucket {
    capacity_milli_tokens: u64,
    refill_rate_per_sec: u64,
    milli_tokens: u64,
    last_refill_millis: u64,
}

impl TokenBucket {
    /// Starts full (`capacity` tokens available immediately) — the
    /// conventional token-bucket default, since a bucket that started
    /// empty would make the very first burst §53 exists to allow
    /// impossible until a full `capacity / refill_rate` seconds had
    /// passed.
    pub fn new(capacity: u64, refill_rate_per_sec: u64, now_millis: u64) -> Self {
        let capacity_milli_tokens = capacity.saturating_mul(1000);
        Self {
            capacity_milli_tokens,
            refill_rate_per_sec,
            milli_tokens: capacity_milli_tokens,
            last_refill_millis: now_millis,
        }
    }

    fn refill(&mut self, now_millis: u64) {
        // A clock that goes backwards must not rewind the refill mark:
        // doing so would make the *next* forward reading look like a
        // long elapsed interval and hand out a burst of free tokens.
        if now_millis <= self.last_refill_millis {
            return;
        }
        let elapsed = now_millis - self.last_refill_millis;
        let added = elapsed.saturating_mul(self.refill_rate_per_sec);
        self.milli_tokens = self
            .milli_tokens
            .saturating_add(added)
            .min(self.capacity_milli_tokens);
        self.last_refill_millis = now_millis;
    }

    /// Refills up to `now_millis`, then attempts to withdraw `amount`
    /// tokens. Returns `true` and deducts on success; returns `false`
    /// and leaves the bucket untouched otherwise (`now_millis` still
    /// advances the refill clock even on rejection — a caller
    /// hammering a starved bucket doesn't get to keep resetting its
    /// own refill baseline just by trying).
    pub fn try_consume(&mut self, amount: u64, now_millis: u64) -> bool {
        self.refill(now_millis);
        let needed = amount.saturating_mul(1000);
        if self.milli_tokens >= needed {
            self.milli_tokens -= needed;
            true
        } else {
            false
        }
    }

    /// Tokens currently available, after refilling as of `now_millis`
    /// — read-only, for a caller that wants to check before deciding
    /// how much to request rather than probing with `try_consume`.
    pub fn available(&mut self, now_millis: u64) -> u64 {
        self.refill(now_millis);
        self.milli_tokens / 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backwards_clock_does_not_grant_a_free_burst() {
        let mut b = TokenBucket::new(100, 10, 10_000);
        assert!(b.try_consume(100, 10_000)); // drain
        assert!(!b.try_consume(1, 5_000)); // clock jumps back: no refill
                                           // Forward again to just 1s after the real last mark: only ~10 tokens.
        assert_eq!(b.available(11_000), 10);
    }

    #[test]
    fn starts_full_and_allows_an_immediate_burst_up_to_capacity() {
        let mut bucket = TokenBucket::new(1000, 100, 0);
        assert!(bucket.try_consume(1000, 0));
    }

    #[test]
    fn rejects_consumption_beyond_capacity_even_with_zero_elapsed_time() {
        let mut bucket = TokenBucket::new(1000, 100, 0);
        assert!(!bucket.try_consume(1001, 0));
        // Rejection must not have deducted anything.
        assert_eq!(bucket.available(0), 1000);
    }

    #[test]
    fn refills_exactly_by_elapsed_millis_times_rate_with_no_rounding_drift() {
        let mut bucket = TokenBucket::new(1000, 100, 0);
        assert!(bucket.try_consume(1000, 0)); // drain to empty
        assert_eq!(bucket.available(0), 0);

        // 1000ms at 100 tokens/sec = exactly 100 tokens, checked to
        // the integer, not just "some positive amount."
        assert_eq!(bucket.available(1_000), 100);
    }

    #[test]
    fn refill_never_exceeds_capacity_however_long_the_elapsed_time() {
        let mut bucket = TokenBucket::new(1000, 100, 0);
        bucket.try_consume(1000, 0);
        // Far more time than needed to fully refill.
        assert_eq!(bucket.available(10_000_000), 1000);
    }

    #[test]
    fn rejected_consumption_still_advances_the_refill_clock() {
        let mut bucket = TokenBucket::new(1000, 100, 0);
        bucket.try_consume(1000, 0);
        assert!(!bucket.try_consume(50, 100)); // only 10 tokens back at t=100ms
                                               // Refill baseline moved to t=100 despite the rejection — from
                                               // t=100, another 400ms at 100/sec adds exactly 40 more,
                                               // landing at 10 + 40 = 50 available, not 100 (which a reset
                                               // baseline would incorrectly allow).
        assert_eq!(bucket.available(500), 50);
    }

    #[test]
    fn partial_consumption_is_never_allowed_only_all_or_nothing() {
        let mut bucket = TokenBucket::new(100, 10, 0);
        assert!(!bucket.try_consume(101, 0));
        // Full balance still intact — nothing was partially withdrawn.
        assert_eq!(bucket.available(0), 100);
    }

    #[test]
    fn typical_per_peer_bytes_per_second_usage_shape() {
        // §53's own named use case: per-peer bytes/sec.
        let mut per_peer = TokenBucket::new(64 * 1024, 16 * 1024, 0); // 64 KiB burst, 16 KiB/s
        assert!(per_peer.try_consume(64 * 1024, 0)); // use the initial burst
        assert!(!per_peer.try_consume(1, 0)); // immediately exhausted
        assert!(per_peer.try_consume(16 * 1024, 1_000)); // one second later, refilled
    }
}
