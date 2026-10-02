//! §71-72: expiry without trustworthy wall clocks.
//!
//! Each node records a monotonic first-observation instant and counts
//! age from it. The wall clock is only trusted when the bundle's claimed
//! creation time is plausible relative to the local clock; when it is
//! not (future-dated by more than the tolerance), the bundle gets its
//! *full* relative lifetime from first sight rather than a wall deadline
//! an attacker (or a wrong clock) chose. Lifetime is capped so a hostile
//! far-future expiry can't pin relay storage.

/// Hard ceiling on any bundle's lifetime, whatever it claims.
pub const MAX_BUNDLE_LIFETIME_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;
pub const DEFAULT_CLOCK_TOLERANCE_MILLIS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifetimeRecord {
    first_seen_mono_millis: u64,
    /// Remaining life at first observation, already capped.
    remaining_at_first_seen_millis: u64,
    /// Present only when the wall clock was judged trustworthy.
    wall_expiry_millis: Option<u64>,
}

impl LifetimeRecord {
    /// `created_at`/`expires_at` are the bundle's own (untrusted) wall
    /// timestamps; `now_wall`/`now_mono` are this node's clocks.
    pub fn observe(
        created_at_millis: u64,
        expires_at_millis: u64,
        now_wall_millis: u64,
        now_mono_millis: u64,
        tolerance_millis: u64,
    ) -> Self {
        let lifetime = expires_at_millis
            .saturating_sub(created_at_millis)
            .min(MAX_BUNDLE_LIFETIME_MILLIS);
        let wall_plausible = created_at_millis <= now_wall_millis.saturating_add(tolerance_millis);
        let (remaining, wall_expiry) = if wall_plausible {
            let by_wall = expires_at_millis.saturating_sub(now_wall_millis);
            let r = by_wall.min(lifetime);
            (r, Some(now_wall_millis.saturating_add(r)))
        } else {
            (lifetime, None)
        };
        Self {
            first_seen_mono_millis: now_mono_millis,
            remaining_at_first_seen_millis: remaining,
            wall_expiry_millis: wall_expiry,
        }
    }

    /// Expired if *either* the monotonic age or (when trusted) the wall
    /// deadline has run out. Using the earlier of the two means a wall
    /// clock that is later set backwards cannot resurrect a bundle.
    pub fn is_expired(&self, now_wall_millis: u64, now_mono_millis: u64) -> bool {
        let age = now_mono_millis.saturating_sub(self.first_seen_mono_millis);
        if age >= self.remaining_at_first_seen_millis {
            return true;
        }
        self.wall_expiry_millis
            .is_some_and(|w| now_wall_millis >= w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: u64 = DEFAULT_CLOCK_TOLERANCE_MILLIS;

    #[test]
    fn plausible_clock_expires_at_the_wall_deadline() {
        let r = LifetimeRecord::observe(1_000, 11_000, 2_000, 0, TOL);
        assert!(!r.is_expired(10_999, 1));
        assert!(r.is_expired(11_000, 1));
    }

    #[test]
    fn wall_clock_set_backwards_cannot_resurrect_a_bundle() {
        let r = LifetimeRecord::observe(0, 10_000, 0, 0, TOL);
        assert!(r.is_expired(0, 10_000), "monotonic age alone expires it");
    }

    #[test]
    fn future_dated_bundle_gets_relative_lifetime_not_a_wall_deadline() {
        let far_future = 10_000_000_000_000;
        let r = LifetimeRecord::observe(far_future, far_future + 60_000, 1_000, 500, TOL);
        assert!(!r.is_expired(1_000_000_000, 500 + 59_999));
        assert!(r.is_expired(0, 500 + 60_000));
    }

    #[test]
    fn hostile_lifetime_is_capped() {
        let r = LifetimeRecord::observe(0, u64::MAX, 0, 0, TOL);
        assert!(r.is_expired(0, MAX_BUNDLE_LIFETIME_MILLIS));
    }

    #[test]
    fn inverted_timestamps_expire_immediately() {
        let r = LifetimeRecord::observe(5_000, 1_000, 6_000, 0, TOL);
        assert!(r.is_expired(6_000, 0));
    }
}
