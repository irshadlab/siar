//! §91 "Monotonic vs Wall Time", §92 "Retry Timers Recovery", §93
//! "Backoff Recovery".
//!
//! Two clocks, two roles. Across a restart only persisted *wall* time
//! is meaningful, so age/expiry decisions carry an explicit skew
//! tolerance and can answer "uncertain". Inside a process a
//! [`MonotonicDeadline`] is used — deliberately not `Serialize`, so
//! persisting one is a compile error (§91: "do not persist monotonic
//! timestamps directly"). To persist a deadline, convert it with
//! [`persistable_wall_deadline`].

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------
// §91
// ---------------------------------------------------------------------

/// Persisted wall-clock time, milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WallMillis(pub u64);

/// In-process deadline on the monotonic clock. Intentionally neither
/// `Serialize` nor convertible to a bare integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonotonicDeadline(Instant);

impl MonotonicDeadline {
    pub fn after(d: Duration) -> Self {
        Self(Instant::now() + d)
    }

    pub fn is_reached(&self) -> bool {
        Instant::now() >= self.0
    }

    pub fn remaining(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

/// Converts the time left on an in-process deadline into a wall-time
/// value that is safe to persist.
pub fn persistable_wall_deadline(remaining: Duration, now: WallMillis) -> WallMillis {
    WallMillis(
        now.0
            .saturating_add(remaining.as_millis().min(u64::MAX as u128) as u64),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSanity {
    Normal,
    /// Wall time is earlier than a time we persisted by more than the
    /// tolerance: clock reset, manual change, or restore.
    WentBackward {
        by_millis: u64,
    },
}

pub const fn check_clock(
    last_persisted: WallMillis,
    now: WallMillis,
    tolerance: u64,
) -> ClockSanity {
    if now.0 >= last_persisted.0 {
        ClockSanity::Normal
    } else {
        let by = last_persisted.0 - now.0;
        if by <= tolerance {
            ClockSanity::Normal
        } else {
            ClockSanity::WentBackward { by_millis: by }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeEstimate {
    Elapsed(u64),
    /// `now` is before `start` by more than the tolerance; the age is
    /// not knowable.
    Unreliable,
}

/// Age across a restart. Small backward skew within `tolerance`
/// counts as zero elapsed rather than negative.
pub const fn age_millis(start: WallMillis, now: WallMillis, tolerance: u64) -> AgeEstimate {
    if now.0 >= start.0 {
        AgeEstimate::Elapsed(now.0 - start.0)
    } else if start.0 - now.0 <= tolerance {
        AgeEstimate::Elapsed(0)
    } else {
        AgeEstimate::Unreliable
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryState {
    Live,
    Expired,
    /// Within ±tolerance of the expiry instant: callers choose the
    /// conservative reading for their domain (security-sensitive
    /// tokens treat it as expired, user-visible retries as live).
    Uncertain,
}

pub const fn expiry_state(expiry: WallMillis, now: WallMillis, tolerance: u64) -> ExpiryState {
    if now.0.saturating_add(tolerance) < expiry.0 {
        ExpiryState::Live
    } else if now.0 >= expiry.0.saturating_add(tolerance) {
        ExpiryState::Expired
    } else {
        ExpiryState::Uncertain
    }
}

// ---------------------------------------------------------------------
// §92
// ---------------------------------------------------------------------

/// §92's three persisted fields for long-lived durable work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryState {
    pub next_attempt_at: WallMillis,
    pub attempt_count: u32,
    pub expiry: WallMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    EligibleNow,
    WaitUntil(WallMillis),
    Expired,
}

/// After restart: expired work is dropped; if `next_attempt_at` has
/// passed it is eligible *now*; otherwise it keeps its time.
pub const fn eligibility(s: &RetryState, now: WallMillis) -> Eligibility {
    if now.0 >= s.expiry.0 {
        Eligibility::Expired
    } else if now.0 >= s.next_attempt_at.0 {
        Eligibility::EligibleNow
    } else {
        Eligibility::WaitUntil(s.next_attempt_at)
    }
}

// ---------------------------------------------------------------------
// §93
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub base_millis: u64,
    pub max_millis: u64,
}

/// `base * 2^(attempt-1)`, capped. Attempt 0 and 1 both give `base`.
pub fn backoff_delay(attempt_count: u32, p: BackoffPolicy) -> u64 {
    let shift = attempt_count.saturating_sub(1).min(40);
    p.base_millis
        .saturating_mul(1u64 << shift)
        .min(p.max_millis)
}

/// §93's "coarse retry state": a level (≈ log₂ of attempts) is enough
/// to avoid restarting from zero, and is cheap to persist.
pub const fn coarse_level(attempt_count: u32) -> u8 {
    (32 - attempt_count.leading_zeros()) as u8
}

/// Delay to use for the first retry *after* a restart. The attempt
/// count survives the crash, so the backoff continues rather than
/// resetting to `base` for everything at once.
pub fn first_delay_after_restart(s: &RetryState, p: BackoffPolicy) -> u64 {
    backoff_delay(s.attempt_count.saturating_add(1), p)
}

/// When to try next after a restart: never *earlier* than the
/// persisted `next_attempt_at`.
pub const fn next_attempt_after_restart(s: &RetryState, now: WallMillis) -> WallMillis {
    if now.0 > s.next_attempt_at.0 {
        now
    } else {
        s.next_attempt_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_deadline_tracks_in_process_time() {
        let d = MonotonicDeadline::after(Duration::from_secs(3600));
        assert!(!d.is_reached());
        assert!(d.remaining() > Duration::from_secs(3500));
        assert!(MonotonicDeadline::after(Duration::ZERO).is_reached());
    }

    #[test]
    fn deadlines_are_persisted_as_wall_time() {
        let w = persistable_wall_deadline(Duration::from_secs(10), WallMillis(1_000));
        assert_eq!(w, WallMillis(11_000));
        assert_eq!(
            persistable_wall_deadline(Duration::MAX, WallMillis(5)),
            WallMillis(u64::MAX)
        );
    }

    #[test]
    fn small_backward_skew_is_tolerated_large_is_flagged() {
        assert_eq!(
            check_clock(WallMillis(1_000), WallMillis(990), 50),
            ClockSanity::Normal
        );
        assert_eq!(
            check_clock(WallMillis(1_000), WallMillis(500), 50),
            ClockSanity::WentBackward { by_millis: 500 }
        );
        assert_eq!(
            check_clock(WallMillis(1_000), WallMillis(2_000), 50),
            ClockSanity::Normal
        );
    }

    #[test]
    fn age_uses_wall_time_with_tolerance() {
        assert_eq!(
            age_millis(WallMillis(100), WallMillis(350), 10),
            AgeEstimate::Elapsed(250)
        );
        assert_eq!(
            age_millis(WallMillis(100), WallMillis(95), 10),
            AgeEstimate::Elapsed(0)
        );
        assert_eq!(
            age_millis(WallMillis(100), WallMillis(50), 10),
            AgeEstimate::Unreliable
        );
    }

    #[test]
    fn expiry_has_a_tolerance_band_of_uncertainty() {
        let e = WallMillis(1_000);
        assert_eq!(expiry_state(e, WallMillis(800), 100), ExpiryState::Live);
        assert_eq!(
            expiry_state(e, WallMillis(950), 100),
            ExpiryState::Uncertain
        );
        assert_eq!(
            expiry_state(e, WallMillis(1_050), 100),
            ExpiryState::Uncertain
        );
        assert_eq!(
            expiry_state(e, WallMillis(1_100), 100),
            ExpiryState::Expired
        );
        // zero tolerance degenerates to a plain comparison
        assert_eq!(expiry_state(e, WallMillis(999), 0), ExpiryState::Live);
        assert_eq!(expiry_state(e, WallMillis(1_000), 0), ExpiryState::Expired);
    }

    fn st(next: u64, attempts: u32, exp: u64) -> RetryState {
        RetryState {
            next_attempt_at: WallMillis(next),
            attempt_count: attempts,
            expiry: WallMillis(exp),
        }
    }

    #[test]
    fn restart_eligibility_follows_next_attempt_and_expiry() {
        let s = st(500, 3, 5_000);
        assert_eq!(
            eligibility(&s, WallMillis(100)),
            Eligibility::WaitUntil(WallMillis(500))
        );
        assert_eq!(eligibility(&s, WallMillis(500)), Eligibility::EligibleNow);
        assert_eq!(eligibility(&s, WallMillis(4_999)), Eligibility::EligibleNow);
        assert_eq!(eligibility(&s, WallMillis(5_000)), Eligibility::Expired);
    }

    #[test]
    fn expiry_outranks_eligibility() {
        let s = st(0, 1, 10);
        assert_eq!(eligibility(&s, WallMillis(10)), Eligibility::Expired);
    }

    const P: BackoffPolicy = BackoffPolicy {
        base_millis: 100,
        max_millis: 10_000,
    };

    #[test]
    fn backoff_doubles_and_caps() {
        let v: Vec<_> = (0..9).map(|a| backoff_delay(a, P)).collect();
        assert_eq!(
            v,
            vec![100, 100, 200, 400, 800, 1_600, 3_200, 6_400, 10_000]
        );
        assert_eq!(backoff_delay(u32::MAX, P), 10_000);
    }

    #[test]
    fn backoff_is_not_reset_to_zero_by_a_crash() {
        let s = st(0, 8, 99_999);
        assert!(first_delay_after_restart(&s, P) > P.base_millis);
        assert_eq!(first_delay_after_restart(&s, P), 10_000);
    }

    #[test]
    fn coarse_level_grows_logarithmically() {
        assert_eq!(coarse_level(0), 0);
        assert_eq!(coarse_level(1), 1);
        assert_eq!(coarse_level(3), 2);
        assert_eq!(coarse_level(1_000), 10);
        assert_eq!(coarse_level(u32::MAX), 32);
    }

    #[test]
    fn next_attempt_is_never_pulled_earlier_by_a_restart() {
        let s = st(5_000, 2, 99_999);
        assert_eq!(
            next_attempt_after_restart(&s, WallMillis(100)),
            WallMillis(5_000)
        );
        assert_eq!(
            next_attempt_after_restart(&s, WallMillis(9_000)),
            WallMillis(9_000)
        );
    }
}
