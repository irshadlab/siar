//! §180 "Device Link Rate Limits", §181 "Recovery Rate Limits".
//!
//! §181's second half needs no new type: "offline recovery should use
//! strong local cryptographic proof rather than relying solely on
//! rate limits" is already [`crate::recovery::add_device_via_recovery`]'s
//! own design — a quorum/policy check over real evidence, with no rate
//! limiter anywhere in that path. [`RecoveryRateLimiter`] below exists
//! only for §181's first half, "where infrastructure participates,"
//! and is never a substitute for that cryptographic check — the two
//! compose (infrastructure rate-limits the attempt; the evidence check
//! decides whether it succeeds), neither one implies the other.

use std::collections::HashMap;

use siar_domain::AccountId;
use siar_event_log::ids::Timestamp;

/// §180's own three named counters, tracked per account. Deliberately
/// three separate counters rather than one combined "attempts" count
/// — "active invites" is a standing total, not a rate; "attempts per
/// time window" and "failed verification attempts" are two different
/// rates that should be tunable independently (a device that's simply
/// slow to complete linking shouldn't count against the SAME budget as
/// one presenting wrong verification codes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRateLimits {
    pub max_active_invites: u32,
    pub max_attempts_per_window: u32,
    pub window: std::time::Duration,
    pub max_failed_verifications: u32,
}

impl Default for LinkRateLimits {
    fn default() -> Self {
        Self {
            max_active_invites: 3,
            max_attempts_per_window: 5,
            window: std::time::Duration::from_secs(600),
            max_failed_verifications: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRateLimitViolation {
    TooManyActiveInvites { actual: u32, limit: u32 },
    TooManyAttemptsInWindow { actual: u32, limit: u32 },
    TooManyFailedVerifications { actual: u32, limit: u32 },
}

#[derive(Debug, Clone, Default)]
struct AccountLinkActivity {
    active_invites: u32,
    attempt_timestamps: Vec<Timestamp>,
    failed_verifications: u32,
}

/// Per-account counters checked against [`LinkRateLimits`]. "Require
/// user confirmation" (§180's fourth requirement) is deliberately NOT
/// modeled here — that's a UI action this crate has no business
/// performing; this type only ever answers "is this account currently
/// over a rate limit," the input a confirmation prompt would act on.
#[derive(Debug, Clone, Default)]
pub struct LinkRateLimiter {
    limits: LinkRateLimits,
    activity: HashMap<AccountId, AccountLinkActivity>,
}

impl LinkRateLimiter {
    pub fn new(limits: LinkRateLimits) -> Self {
        Self {
            limits,
            activity: HashMap::new(),
        }
    }

    pub fn record_invite_created(&mut self, account_id: AccountId) {
        let a = self.activity.entry(account_id).or_default();
        a.active_invites = a.active_invites.saturating_add(1);
    }

    pub fn record_invite_consumed_or_expired(&mut self, account_id: AccountId) {
        if let Some(activity) = self.activity.get_mut(&account_id) {
            activity.active_invites = activity.active_invites.saturating_sub(1);
        }
    }

    pub fn record_attempt(&mut self, account_id: AccountId, now: Timestamp) {
        let activity = self.activity.entry(account_id).or_default();
        activity.attempt_timestamps.push(now);
        let window_start = now.0.saturating_sub(self.limits.window.as_millis() as u64);
        activity.attempt_timestamps.retain(|t| t.0 >= window_start);
    }

    pub fn record_failed_verification(&mut self, account_id: AccountId) {
        self.activity
            .entry(account_id)
            .or_default()
            .failed_verifications += 1;
    }

    pub fn check(&self, account_id: AccountId) -> Vec<LinkRateLimitViolation> {
        let Some(activity) = self.activity.get(&account_id) else {
            return Vec::new();
        };
        let mut violations = Vec::new();
        if activity.active_invites > self.limits.max_active_invites {
            violations.push(LinkRateLimitViolation::TooManyActiveInvites {
                actual: activity.active_invites,
                limit: self.limits.max_active_invites,
            });
        }
        let attempts_in_window = activity.attempt_timestamps.len() as u32;
        if attempts_in_window > self.limits.max_attempts_per_window {
            violations.push(LinkRateLimitViolation::TooManyAttemptsInWindow {
                actual: attempts_in_window,
                limit: self.limits.max_attempts_per_window,
            });
        }
        if activity.failed_verifications > self.limits.max_failed_verifications {
            violations.push(LinkRateLimitViolation::TooManyFailedVerifications {
                actual: activity.failed_verifications,
                limit: self.limits.max_failed_verifications,
            });
        }
        violations
    }
}

/// §181's infrastructure-side counter — structurally simpler than
/// [`LinkRateLimiter`] since spec names no sub-categories for
/// recovery attempts the way §180 names three for linking.
#[derive(Debug, Clone, Default)]
pub struct RecoveryRateLimiter {
    attempts: HashMap<AccountId, u32>,
    max_attempts: u32,
}

impl RecoveryRateLimiter {
    pub fn new(max_attempts: u32) -> Self {
        Self {
            attempts: HashMap::new(),
            max_attempts,
        }
    }

    pub fn record_attempt(&mut self, account_id: AccountId) {
        let n = self.attempts.entry(account_id).or_insert(0);
        *n = n.saturating_add(1);
    }

    pub fn is_over_limit(&self, account_id: AccountId) -> bool {
        self.attempts.get(&account_id).copied().unwrap_or(0) > self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_invite_count_is_tracked_independent_of_attempts_or_failures() {
        let mut limiter = LinkRateLimiter::new(LinkRateLimits {
            max_active_invites: 1,
            ..LinkRateLimits::default()
        });
        let account = AccountId::new();
        limiter.record_invite_created(account);
        limiter.record_invite_created(account);

        let violations = limiter.check(account);
        assert!(
            violations.contains(&LinkRateLimitViolation::TooManyActiveInvites {
                actual: 2,
                limit: 1
            })
        );
    }

    #[test]
    fn consuming_an_invite_frees_up_the_active_invite_budget() {
        let mut limiter = LinkRateLimiter::new(LinkRateLimits {
            max_active_invites: 1,
            ..LinkRateLimits::default()
        });
        let account = AccountId::new();
        limiter.record_invite_created(account);
        limiter.record_invite_created(account);
        limiter.record_invite_consumed_or_expired(account);

        assert!(!limiter
            .check(account)
            .iter()
            .any(|v| matches!(v, LinkRateLimitViolation::TooManyActiveInvites { .. })));
    }

    #[test]
    fn failed_verifications_are_tracked_separately_from_attempts() {
        let mut limiter = LinkRateLimiter::new(LinkRateLimits {
            max_failed_verifications: 0,
            ..LinkRateLimits::default()
        });
        let account = AccountId::new();
        limiter.record_failed_verification(account);

        let violations = limiter.check(account);
        assert!(violations
            .iter()
            .any(|v| matches!(v, LinkRateLimitViolation::TooManyFailedVerifications { .. })));
    }

    #[test]
    fn attempt_window_only_counts_attempts_still_within_the_window() {
        let mut limiter = LinkRateLimiter::new(LinkRateLimits {
            max_attempts_per_window: 1,
            window: std::time::Duration::from_millis(1000),
            ..LinkRateLimits::default()
        });
        let account = AccountId::new();
        limiter.record_attempt(account, Timestamp(0));
        // Well outside the window — the earlier attempt should have
        // aged out by the time this one is recorded.
        limiter.record_attempt(account, Timestamp(10_000));

        assert!(!limiter
            .check(account)
            .iter()
            .any(|v| matches!(v, LinkRateLimitViolation::TooManyAttemptsInWindow { .. })));
    }

    #[test]
    fn recovery_rate_limiter_flags_only_once_the_limit_is_exceeded() {
        let mut limiter = RecoveryRateLimiter::new(2);
        let account = AccountId::new();
        limiter.record_attempt(account);
        limiter.record_attempt(account);
        assert!(!limiter.is_over_limit(account));

        limiter.record_attempt(account);
        assert!(limiter.is_over_limit(account));
    }
}
