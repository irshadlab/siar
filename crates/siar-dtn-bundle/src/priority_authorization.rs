//! §91 "Authority Alerts", §93 "Priority Authorization", §94 "Priority
//! Downgrade".
//!
//! §92 "Spam Resistance" is named here but not given a type of its own:
//! its six named mechanisms (rate limits, bundle size caps, per-origin
//! quotas, proof-of-trust, issuer policy, critical-class restrictions)
//! are, respectively, `siar-resource-limits`'s own domain (rate/size/
//! quota enforcement — Part 08, already the highest cross-referenced
//! spec in this whole workspace; see `ROADMAP.md`'s own cross-spec
//! analysis), an identity/authority concern
//! (`siar-identity-multidevice`), and exactly what
//! [`authorize_priority`] below already does for the sixth
//! ("critical-class restrictions"). Building a seventh, redundant type
//! here that just re-names those other crates' own mechanisms would be
//! the opposite of this module's own job.
//!
//! [`DtnPriority::Sos`] is this crate's own name for what §36's spec
//! text (and §93's own worked example) calls `AuthorityCritical`/
//! `Critical` — see [`crate::types::DtnPriority`]'s own doc comment for
//! why the variant set diverged from the spec's suggested one. No new
//! variant is added here: §93's "unknown peer cannot claim
//! AuthorityCritical" is read as "unknown peer cannot claim `Sos`."

use serde::{Deserialize, Serialize};

use crate::types::{BroadcastScope, DtnPriority, RouteToken};

/// §91, field-for-field. `origin_signature`/`trusted_issuer` deliberately
/// mirror [`crate::bundle::BundleIntegrity::origin_signature`]'s own
/// `Vec<u8>`/opaque-token treatment — the real signature-verification
/// logic is `siar-crypto`/`siar-identity-multidevice`'s job (this
/// crate has no decrypt or verify path — see this crate's own top doc
/// comment), so this struct is the data shape an authority alert
/// carries, not a verifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorityAlert {
    pub origin_signature: Vec<u8>,
    pub trusted_issuer: RouteToken,
    pub expiry_millis: u64,
    pub scope: BroadcastScope,
}

impl AuthorityAlert {
    pub fn is_expired(&self, now_millis: u64) -> bool {
        now_millis >= self.expiry_millis
    }
}

/// §93's own worked example, generalized: whether the peer presenting
/// this claim has independently-verified standing to request
/// [`DtnPriority::Sos`] — verified upstream by
/// `siar-identity-multidevice`/an [`AuthorityAlert`]'s own trust chain,
/// never by this crate. `false` is the correct default for literally
/// every peer this crate has never been told otherwise about — §93's
/// own "map requested priority through local policy," not "trust
/// whatever a peer sends."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityClaim {
    pub requested_priority: DtnPriority,
    pub peer_authorized_for_critical: bool,
}

/// §94's own fork, made an explicit, caller-supplied choice rather than
/// a hardcoded pick — the spec itself says "downgrade, or reject,"
/// not which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnauthorizedPriorityPolicy {
    DowngradeTo(DtnPriority),
    Reject,
}

/// §93/§94 together: "unknown peer cannot claim `AuthorityCritical`...
/// if sender requests disallowed priority: downgrade, or reject. Do
/// not trust remote enum blindly." A claim below [`DtnPriority::Sos`]
/// always passes through unchanged — this policy exists specifically
/// to gate the one priority tier that implies special handling
/// (reserved capacity, broadcast reach), not to second-guess every
/// claim a peer makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityDecision {
    Accept(DtnPriority),
    Downgraded(DtnPriority),
    Rejected,
}

pub fn authorize_priority(
    claim: PriorityClaim,
    unauthorized_policy: UnauthorizedPriorityPolicy,
) -> PriorityDecision {
    if claim.requested_priority == DtnPriority::Sos && !claim.peer_authorized_for_critical {
        return match unauthorized_policy {
            UnauthorizedPriorityPolicy::DowngradeTo(priority) => {
                PriorityDecision::Downgraded(priority)
            }
            UnauthorizedPriorityPolicy::Reject => PriorityDecision::Rejected,
        };
    }
    PriorityDecision::Accept(claim.requested_priority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_authority_alert_expires_at_and_after_its_own_deadline() {
        let alert = AuthorityAlert {
            origin_signature: vec![1, 2, 3],
            trusted_issuer: RouteToken(vec![9]),
            expiry_millis: 1_000,
            scope: BroadcastScope { radius_hops: 3 },
        };
        assert!(!alert.is_expired(999));
        assert!(alert.is_expired(1_000));
    }

    #[test]
    fn a_non_critical_claim_always_passes_through_regardless_of_authorization() {
        let claim = PriorityClaim {
            requested_priority: DtnPriority::Important,
            peer_authorized_for_critical: false,
        };
        assert_eq!(
            authorize_priority(claim, UnauthorizedPriorityPolicy::Reject),
            PriorityDecision::Accept(DtnPriority::Important)
        );
    }

    #[test]
    fn an_authorized_peer_s_sos_claim_is_accepted() {
        let claim = PriorityClaim {
            requested_priority: DtnPriority::Sos,
            peer_authorized_for_critical: true,
        };
        assert_eq!(
            authorize_priority(claim, UnauthorizedPriorityPolicy::Reject),
            PriorityDecision::Accept(DtnPriority::Sos)
        );
    }

    #[test]
    fn an_unauthorized_peer_s_sos_claim_is_downgraded_when_policy_says_so() {
        let claim = PriorityClaim {
            requested_priority: DtnPriority::Sos,
            peer_authorized_for_critical: false,
        };
        assert_eq!(
            authorize_priority(
                claim,
                UnauthorizedPriorityPolicy::DowngradeTo(DtnPriority::Important)
            ),
            PriorityDecision::Downgraded(DtnPriority::Important)
        );
    }

    #[test]
    fn an_unauthorized_peer_s_sos_claim_is_rejected_when_policy_says_so() {
        let claim = PriorityClaim {
            requested_priority: DtnPriority::Sos,
            peer_authorized_for_critical: false,
        };
        assert_eq!(
            authorize_priority(claim, UnauthorizedPriorityPolicy::Reject),
            PriorityDecision::Rejected
        );
    }

    #[test]
    fn an_unknown_peer_can_never_claim_critical_by_merely_asserting_it() {
        // The whole point of §93: presence of the claim alone (with no
        // independent authorization) must never itself grant Sos.
        let claim = PriorityClaim {
            requested_priority: DtnPriority::Sos,
            peer_authorized_for_critical: false,
        };
        let decision = authorize_priority(
            claim,
            UnauthorizedPriorityPolicy::DowngradeTo(DtnPriority::Normal),
        );
        assert_ne!(decision, PriorityDecision::Accept(DtnPriority::Sos));
    }
}
