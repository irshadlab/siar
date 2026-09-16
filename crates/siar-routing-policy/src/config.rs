//! §117 "Route Policy Configuration".
//!
//! §117's own code block names seven fields; five already had a real,
//! more specific type elsewhere in this crate by the time this round
//! started — [`RoutingConfig`] reuses every one of them rather than
//! re-declaring narrower duplicates:
//! `relay_policy`/`bluetooth_policy`/`dtn_policy` are booleans, the
//! same shape [`crate::requirements::DeliveryRequirements`]'s own
//! `allow_relay`/`allow_bluetooth`/`allow_dtn` already use for an
//! identical on/off transport gate — this struct's versions are the
//! *startup-wide* default, not a duplicate concept, matching how
//! [`crate::decision::PolicyLayers`]'s `application` layer
//! already sits above per-operation `DeliveryRequirements` fields in
//! the same way. `multipath_policy` is likewise a bool, mirroring
//! [`crate::requirements::DeliveryRequirements::allow_multipath`]. `retry_policy` is
//! [`crate::retry::RetryPolicy`] unchanged; `hysteresis` is
//! [`crate::policy::HysteresisPolicy`] unchanged. `direct_preference`
//! has no existing typed home — [`crate::privacy::PrivacyPolicy::prefer_direct`]
//! is the *per-call* version of the same idea, so this field is that
//! setting's startup-wide default, plain `bool`.
//!
//! "Validate at startup" ([`RoutingConfig::validate`]) checks the one
//! genuinely inconsistent state this shape can be in:
//! `retry_policy.initial_backoff_millis` bigger than its own
//! `max_backoff_millis` (nothing later in the crate would catch this —
//! [`crate::retry::RetryPolicy::base_backoff_millis`] would just
//! silently clamp every attempt to the same wrong number forever).
//! Every other field is a plain `bool`/already-validated-at-construction
//! type ([`crate::metrics::Ratio`], reached through `retry_policy`,
//! already clamps to `[0.0, 1.0]` on construction — nothing to check
//! twice) — there is no combination of booleans this struct can hold
//! that's inherently wrong the way an inverted backoff range is, so
//! this function doesn't invent a check for one.

use serde::{Deserialize, Serialize};

use crate::policy::HysteresisPolicy;
use crate::retry::RetryPolicy;

/// §117, transcribed field-for-field (see this module's own doc
/// comment for which five of the seven reuse an existing type
/// outright). `Serialize`/`Deserialize` added for §179 "Route Policy
/// Persistence" — see [`crate::privacy::PrivacyPolicy`]'s own doc
/// comment for the fuller reasoning; every field type here
/// (`RetryPolicy`, `HysteresisPolicy`, plain `bool`) already supports
/// it as of this round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    pub direct_preference: bool,
    pub relay_policy: bool,
    pub bluetooth_policy: bool,
    pub dtn_policy: bool,
    pub multipath_policy: bool,
    pub retry_policy: RetryPolicy,
    pub hysteresis: HysteresisPolicy,
}

/// The one thing [`RoutingConfig::validate`] actually checks — see
/// this module's own doc comment for why nothing else needs a
/// variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("retry policy's initial backoff ({initial}ms) exceeds its own max backoff ({max}ms)")]
    RetryBackoffOrderingInverted { initial: u64, max: u64 },
}

impl RoutingConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.retry_policy.initial_backoff_millis > self.retry_policy.max_backoff_millis {
            return Err(ConfigError::RetryBackoffOrderingInverted {
                initial: self.retry_policy.initial_backoff_millis,
                max: self.retry_policy.max_backoff_millis,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Ratio;
    use crate::scoring::RouteScoreDelta;

    fn valid_config() -> RoutingConfig {
        RoutingConfig {
            direct_preference: true,
            relay_policy: true,
            bluetooth_policy: true,
            dtn_policy: true,
            multipath_policy: false,
            retry_policy: RetryPolicy::durable_message(),
            hysteresis: HysteresisPolicy {
                switch_threshold: RouteScoreDelta(0.3),
                minimum_hold_millis: 10_000,
                degraded_override: true,
            },
        }
    }

    #[test]
    fn a_well_formed_config_validates() {
        assert!(valid_config().validate().is_ok());
    }

    #[test]
    fn an_inverted_backoff_range_fails_validation() {
        let mut config = valid_config();
        config.retry_policy = RetryPolicy {
            max_attempts: Some(5),
            initial_backoff_millis: 60_000,
            max_backoff_millis: 1_000, // smaller than initial — inverted
            jitter: Ratio::new(0.2),
            retry_on_network_change: true,
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::RetryBackoffOrderingInverted {
                initial: 60_000,
                max: 1_000,
            })
        );
    }

    #[test]
    fn no_retrys_zeroed_backoff_range_is_not_inverted() {
        // §38's own "typing indicators should not retry" constructor —
        // initial == max == 0, not initial > max, so this must pass.
        let mut config = valid_config();
        config.retry_policy = RetryPolicy::no_retry();
        assert!(config.validate().is_ok());
    }

    /// §179 "Route Policy Persistence": "persist user/application
    /// settings." This crate has no persistence layer of its own to
    /// exercise (see this module's own doc comment), so the strongest
    /// honest proof available here is a compile-time one: a generic
    /// function that only accepts `Serialize + for<'de> Deserialize<'de>`
    /// types, called with every setting type §179 names. If a future
    /// edit ever dropped one of these derives, this test would stop
    /// compiling, not silently pass.
    #[test]
    fn spec_179_every_persistable_setting_type_actually_implements_serde() {
        fn assert_persistable<T: serde::Serialize + for<'de> serde::Deserialize<'de>>(_: &T) {}

        assert_persistable(&valid_config());
        assert_persistable(&crate::privacy::PrivacyPolicy::default());
        assert_persistable(&crate::decision::SystemPolicy {
            max_operation_bytes: crate::descriptor::ByteCount(0),
        });
        assert_persistable(&crate::decision::ApplicationPolicy::default());
        assert_persistable(&crate::policy::RoutingPolicyProfile::Balanced);
    }
}
