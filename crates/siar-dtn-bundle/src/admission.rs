//! §55 "Storage Quotas", §56 "Critical Reserve", §57 "Per-Peer Quota",
//! §58 "Admission Control".

use serde::{Deserialize, Serialize};

use crate::types::StorageClass;

/// §55, field-for-field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageQuotas {
    pub max_dtn_bytes: u64,
    pub max_bundles: u32,
    pub max_per_peer_bytes: u64,
    pub max_critical_reserve_bytes: u64,
}

/// §56: "Reserve storage for SOS, authority alerts, delivery ACKs, so
/// bulk content cannot consume all relay capacity." This crate's own
/// [`StorageClass`] already has exactly two variants that map onto the
/// section's three named items — [`StorageClass::CriticalEmergency`]
/// covers both SOS and authority alerts (an authority alert IS a
/// critical-emergency-class artifact, not a third category needing its
/// own variant), and [`StorageClass::DeliveryReceipt`] covers the
/// third. No new type needed — this function is the one place that
/// mapping is actually enforced, rather than left as a convention two
/// call sites could disagree about.
pub fn is_reserved_class(class: StorageClass) -> bool {
    matches!(
        class,
        StorageClass::CriticalEmergency | StorageClass::DeliveryReceipt
    )
}

/// §57's own three named tiers. "Authorization and quota are
/// separate" — deliberately not reusing
/// [`crate::priority_authorization::PriorityClaim`]'s
/// authorized-for-critical bool: a peer can be a fully verified
/// contact (moderate quota) while still being completely unauthorized
/// to claim SOS priority, and vice versa in principle. Conflating the
/// two into one type would silently violate the section's own explicit
/// instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerQuotaTier {
    Unknown,
    VerifiedContact,
    TrustedAuthority,
}

/// §57's own worked example, "policy-defined" for the top tier taken
/// literally: `trusted_authority_bytes` is a real field a caller sets,
/// not a hardcoded "unlimited," since even a trusted authority sharing
/// one relay with everyone else needs SOME bound to keep the section's
/// own "prevent one peer from filling store" guarantee meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerQuotaPolicy {
    pub unknown_bytes: u64,
    pub verified_contact_bytes: u64,
    pub trusted_authority_bytes: u64,
}

impl PeerQuotaPolicy {
    pub fn quota_for(&self, tier: PeerQuotaTier) -> u64 {
        match tier {
            PeerQuotaTier::Unknown => self.unknown_bytes,
            PeerQuotaTier::VerifiedContact => self.verified_contact_bytes,
            PeerQuotaTier::TrustedAuthority => self.trusted_authority_bytes,
        }
    }
}

/// §58's own seven-item checklist, field-for-field. `verify_envelope`
/// stands for §59's "signature/integrity" check
/// ([`crate::bundle::BundleIntegrity::verify_payload_hash`]) plus the
/// rest of §59's own validation (see [`crate::validation::validate_bundle`])
/// — kept as one caller-supplied bool here rather than re-running that
/// whole check inline, since a caller building this struct has almost
/// certainly already run it as its own, earlier step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionChecks {
    pub size_ok: bool,
    pub envelope_verified: bool,
    pub not_expired: bool,
    pub replication_budget_positive: bool,
    pub quota_available: bool,
    pub forwarding_policy_allows: bool,
    pub not_duplicate: bool,
}

/// Named per the specific check that failed, in §58's own listed
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionDenialReason {
    OversizedBundle,
    EnvelopeInvalid,
    Expired,
    ReplicationBudgetExhausted,
    QuotaExceeded,
    ForwardingPolicyDenied,
    DuplicateBundle,
}

/// §58: "Before accepting a relay bundle: [the seven checks]. Only
/// then persist." Checked in the spec's own listed order; the first
/// failing check is the one reported, and — matching the
/// `authorize_transfer`-style gate `siar-blob-manifest`'s own
/// `transfer_authorization` module already uses for the analogous
/// question in spec 05 — an all-false input is refused, not
/// vacuously admitted.
pub fn admit_bundle(checks: AdmissionChecks) -> Result<(), AdmissionDenialReason> {
    if !checks.size_ok {
        return Err(AdmissionDenialReason::OversizedBundle);
    }
    if !checks.envelope_verified {
        return Err(AdmissionDenialReason::EnvelopeInvalid);
    }
    if !checks.not_expired {
        return Err(AdmissionDenialReason::Expired);
    }
    if !checks.replication_budget_positive {
        return Err(AdmissionDenialReason::ReplicationBudgetExhausted);
    }
    if !checks.quota_available {
        return Err(AdmissionDenialReason::QuotaExceeded);
    }
    if !checks.forwarding_policy_allows {
        return Err(AdmissionDenialReason::ForwardingPolicyDenied);
    }
    if !checks.not_duplicate {
        return Err(AdmissionDenialReason::DuplicateBundle);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_critical_emergency_and_delivery_receipt_are_reserved_classes() {
        assert!(is_reserved_class(StorageClass::CriticalEmergency));
        assert!(is_reserved_class(StorageClass::DeliveryReceipt));
        assert!(!is_reserved_class(StorageClass::LocalOrigin));
        assert!(!is_reserved_class(StorageClass::Relay));
    }

    #[test]
    fn per_peer_quota_scales_with_trust_tier() {
        let policy = PeerQuotaPolicy {
            unknown_bytes: 1_000,
            verified_contact_bytes: 50_000,
            trusted_authority_bytes: 1_000_000,
        };
        assert_eq!(policy.quota_for(PeerQuotaTier::Unknown), 1_000);
        assert_eq!(policy.quota_for(PeerQuotaTier::VerifiedContact), 50_000);
        assert_eq!(policy.quota_for(PeerQuotaTier::TrustedAuthority), 1_000_000);
    }

    fn all_pass() -> AdmissionChecks {
        AdmissionChecks {
            size_ok: true,
            envelope_verified: true,
            not_expired: true,
            replication_budget_positive: true,
            quota_available: true,
            forwarding_policy_allows: true,
            not_duplicate: true,
        }
    }

    #[test]
    fn every_check_passing_admits_the_bundle() {
        assert_eq!(admit_bundle(all_pass()), Ok(()));
    }

    #[test]
    fn an_all_false_input_is_refused_not_vacuously_admitted() {
        let checks = AdmissionChecks {
            size_ok: false,
            envelope_verified: false,
            not_expired: false,
            replication_budget_positive: false,
            quota_available: false,
            forwarding_policy_allows: false,
            not_duplicate: false,
        };
        assert_eq!(
            admit_bundle(checks),
            Err(AdmissionDenialReason::OversizedBundle)
        );
    }

    #[test]
    fn the_first_failing_check_in_spec_order_is_reported() {
        let checks = AdmissionChecks {
            quota_available: false,
            forwarding_policy_allows: false, // would also fail, but quota is checked first
            ..all_pass()
        };
        assert_eq!(
            admit_bundle(checks),
            Err(AdmissionDenialReason::QuotaExceeded)
        );
    }

    #[test]
    fn a_duplicate_is_the_last_check_and_only_reported_when_everything_else_passes() {
        let checks = AdmissionChecks {
            not_duplicate: false,
            ..all_pass()
        };
        assert_eq!(
            admit_bundle(checks),
            Err(AdmissionDenialReason::DuplicateBundle)
        );
    }
}
