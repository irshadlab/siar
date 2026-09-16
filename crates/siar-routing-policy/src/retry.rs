//! §38 "Retry Policy", §39 "Retry on Connectivity Change".

use serde::{Deserialize, Serialize};

use crate::metrics::Ratio;
use crate::requirements::DeliveryRequirements;

/// §38. `Serialize`/`Deserialize` added for §179 "Route Policy
/// Persistence" — see [`crate::privacy::PrivacyPolicy`]'s own doc
/// comment for the fuller reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: Option<u32>,
    pub initial_backoff_millis: u64,
    pub max_backoff_millis: u64,
    pub jitter: Ratio,
    pub retry_on_network_change: bool,
}

impl RetryPolicy {
    /// §38: "Durable messages may retry indefinitely until expiry."
    pub fn durable_message() -> Self {
        Self {
            max_attempts: None,
            initial_backoff_millis: 1_000,
            max_backoff_millis: 60_000,
            jitter: Ratio::new(0.2),
            retry_on_network_change: true,
        }
    }

    /// §38: "Typing indicators should not retry."
    pub fn no_retry() -> Self {
        Self {
            max_attempts: Some(0),
            initial_backoff_millis: 0,
            max_backoff_millis: 0,
            jitter: Ratio::new(0.0),
            retry_on_network_change: false,
        }
    }

    /// Whether attempt number `attempt` (1-based: the value passed for
    /// the *next* attempt about to be made) is still permitted.
    pub fn allows_attempt(&self, attempt: u32) -> bool {
        match self.max_attempts {
            Some(max) => attempt <= max,
            None => true,
        }
    }

    /// The base backoff delay before `attempt` (1-based), before jitter
    /// is applied — plain doubling, capped at `max_backoff_millis`.
    /// Deterministic on purpose (§123's "Deterministic Scoring" ethos
    /// extended to retry timing): jitter is a separate, explicit step
    /// ([`RetryPolicy::apply_jitter`]) that takes an externally-supplied
    /// random value rather than this crate depending on `rand` itself,
    /// so backoff timing stays testable without mocking an RNG.
    pub fn base_backoff_millis(&self, attempt: u32) -> u64 {
        let attempt = attempt.max(1);
        let shift = attempt.saturating_sub(1).min(32);
        let scaled = self
            .initial_backoff_millis
            .saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX));
        scaled.min(self.max_backoff_millis)
    }

    /// §38's `jitter` field applied to a base backoff value.
    /// `random_unit` must be in `[0.0, 1.0)`, supplied by the caller —
    /// see [`RetryPolicy::base_backoff_millis`]'s own doc comment for
    /// why this crate doesn't generate it itself. The result is
    /// `base ± (base * jitter * random_unit)`, keeping the jittered
    /// value within `[base * (1 - jitter), base * (1 + jitter)]`.
    pub fn apply_jitter(base_millis: u64, jitter: Ratio, random_unit: f64) -> u64 {
        let random_unit = random_unit.clamp(0.0, 1.0);
        let spread = base_millis as f64 * jitter.get();
        let offset = spread * (2.0 * random_unit - 1.0);
        (base_millis as f64 + offset).max(0.0).round() as u64
    }

    /// §62 "Expiry-Aware Routing": "\[Routing\] should stop retrying
    /// after expiry" — [`Self::allows_attempt`] alone only ever knew
    /// about attempt *count*; an operation can just as easily run out
    /// of *time* first (§62's own example: "SOS expires after 24h,
    /// typing after 5s" — neither is phrased as a count). This is the
    /// combined check: both the count-based and expiry-based limits
    /// have to allow the attempt, via [`DeliveryRequirements::has_expired`].
    pub fn allows_attempt_at(
        &self,
        attempt: u32,
        req: &DeliveryRequirements,
        created_at_millis: u64,
        now_millis: u64,
    ) -> bool {
        self.allows_attempt(attempt) && !req.has_expired(created_at_millis, now_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_each_attempt_until_the_cap() {
        let policy = RetryPolicy::durable_message();
        assert_eq!(policy.base_backoff_millis(1), 1_000);
        assert_eq!(policy.base_backoff_millis(2), 2_000);
        assert_eq!(policy.base_backoff_millis(3), 4_000);
        assert_eq!(policy.base_backoff_millis(10), 60_000); // capped
    }

    #[test]
    fn durable_messages_allow_unlimited_attempts() {
        let policy = RetryPolicy::durable_message();
        assert!(policy.allows_attempt(1));
        assert!(policy.allows_attempt(1_000_000));
    }

    #[test]
    fn no_retry_policy_disallows_every_attempt() {
        let policy = RetryPolicy::no_retry();
        assert!(!policy.allows_attempt(1));
    }

    #[test]
    fn jitter_stays_within_the_declared_spread() {
        let base = 10_000u64;
        let jitter = Ratio::new(0.2);
        let low = RetryPolicy::apply_jitter(base, jitter, 0.0);
        let mid = RetryPolicy::apply_jitter(base, jitter, 0.5);
        let high = RetryPolicy::apply_jitter(base, jitter, 1.0);
        assert_eq!(low, 8_000);
        assert_eq!(mid, 10_000);
        assert_eq!(high, 12_000);
    }

    #[test]
    fn an_expired_operation_is_not_allowed_to_retry_even_under_its_attempt_cap() {
        use crate::requirements::DeliveryRequirements;
        let policy = RetryPolicy::durable_message(); // unlimited attempts
        let req = DeliveryRequirements::typing_indicator(); // 5_000ms expiry
        assert!(policy.allows_attempt_at(1, &req, 0, 1_000));
        assert!(!policy.allows_attempt_at(1, &req, 0, 6_000));
    }

    #[test]
    fn attempt_cap_still_applies_even_when_not_yet_expired() {
        use crate::requirements::DeliveryRequirements;
        let policy = RetryPolicy::no_retry();
        let req = DeliveryRequirements::interactive_message(); // no expiry
        assert!(!policy.allows_attempt_at(1, &req, 0, 0));
    }
}
