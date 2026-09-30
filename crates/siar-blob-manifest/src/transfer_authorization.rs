//! §46 "Transfer Authorization".
//!
//! §46's own five checks ("peer identity, authorization, size policy,
//! storage quota, content policy") collapse to four fields here:
//! identity verification and authorization are one crate's job
//! (`siar-identity-multidevice`) from this crate's point of view, so
//! [`AuthorizationChecks::peer_authorized`] represents both — this
//! crate has no way to independently verify a peer's identity and
//! shouldn't pretend it does by exposing two separate booleans a
//! caller would just set identically anyway. §46's own closing line —
//! "Connection alone does not imply permission" — is what
//! [`authorize_transfer`] enforces structurally: there is no code path
//! that returns `Ok` without every check passing.

use serde::{Deserialize, Serialize};

/// The facts a caller must already have gathered before asking this
/// module to authorize a transfer — this crate verifies none of these
/// itself (no identity, quota, or content-scanning logic lives here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizationChecks {
    pub peer_authorized: bool,
    pub size_within_policy: bool,
    pub storage_quota_available: bool,
    pub content_policy_passed: bool,
}

/// Named per the specific check that failed, in the same order §46
/// lists them — a caller can tell a sender *why* a transfer was
/// refused, not just that it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorizationDenialReason {
    PeerNotAuthorized,
    SizeExceedsPolicy,
    StorageQuotaExceeded,
    ContentPolicyViolation,
}

/// Checked in §46's own listed order; the first failing check is the
/// one reported — later checks are never evaluated once an earlier one
/// has already failed, since the transfer is refused either way and a
/// caller only needs one reason to act on.
pub fn authorize_transfer(checks: AuthorizationChecks) -> Result<(), AuthorizationDenialReason> {
    if !checks.peer_authorized {
        return Err(AuthorizationDenialReason::PeerNotAuthorized);
    }
    if !checks.size_within_policy {
        return Err(AuthorizationDenialReason::SizeExceedsPolicy);
    }
    if !checks.storage_quota_available {
        return Err(AuthorizationDenialReason::StorageQuotaExceeded);
    }
    if !checks.content_policy_passed {
        return Err(AuthorizationDenialReason::ContentPolicyViolation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_pass() -> AuthorizationChecks {
        AuthorizationChecks {
            peer_authorized: true,
            size_within_policy: true,
            storage_quota_available: true,
            content_policy_passed: true,
        }
    }

    #[test]
    fn every_check_passing_authorizes_the_transfer() {
        assert_eq!(authorize_transfer(all_pass()), Ok(()));
    }

    #[test]
    fn connection_alone_does_not_imply_permission() {
        // Every field false: nothing about "a connection exists" is
        // itself one of the checks, so an all-false input is refused,
        // not vacuously authorized.
        let checks = AuthorizationChecks {
            peer_authorized: false,
            size_within_policy: false,
            storage_quota_available: false,
            content_policy_passed: false,
        };
        assert_eq!(
            authorize_transfer(checks),
            Err(AuthorizationDenialReason::PeerNotAuthorized)
        );
    }

    #[test]
    fn the_first_failing_check_in_spec_order_is_the_one_reported() {
        let checks = AuthorizationChecks {
            peer_authorized: true,
            size_within_policy: false,
            storage_quota_available: false, // would also fail, but size is checked first
            content_policy_passed: true,
        };
        assert_eq!(
            authorize_transfer(checks),
            Err(AuthorizationDenialReason::SizeExceedsPolicy)
        );
    }

    #[test]
    fn each_check_is_independently_reportable() {
        assert_eq!(
            authorize_transfer(AuthorizationChecks {
                storage_quota_available: false,
                ..all_pass()
            }),
            Err(AuthorizationDenialReason::StorageQuotaExceeded)
        );
        assert_eq!(
            authorize_transfer(AuthorizationChecks {
                content_policy_passed: false,
                ..all_pass()
            }),
            Err(AuthorizationDenialReason::ContentPolicyViolation)
        );
    }
}
