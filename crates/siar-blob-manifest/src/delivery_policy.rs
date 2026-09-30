//! §69 "Emergency Files", §70 "Multi-Device File Delivery", §71
//! "Own-Device Blob Sync", §72 "Peer Discovery of Blob Availability".

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};

use crate::ids::BlobId;

/// §69's own six named content kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmergencyFileKind {
    SosText,
    Location,
    Thumbnail,
    Voice,
    FullImage,
    Video,
}

/// §69's own four priority tiers (its example collapses "video" to
/// "low/bulk," read here as one tier — this crate has no separate
/// concept of "bulk" elsewhere to justify a fifth variant). Declared
/// in descending urgency, `Ord`-derived so `Highest < High` reads
/// correctly as "more urgent sorts first" — the same
/// declaration-order-is-rank pattern
/// [`crate::storage_lifecycle::CacheEvictionTier`]/
/// `siar_dtn_bundle::eviction::EvictionTier` already use elsewhere in
/// this workspace, applied here to urgency instead of eviction order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EmergencyFilePriority {
    Highest,
    High,
    Medium,
    LowBulk,
}

/// §69: "Blob engine supplies objects; Part 17 decides priority." This
/// function is this crate's own suggested classification, not an
/// authoritative decision — `siar-emergency` (Part 17) owns the real
/// enforcement, and may reasonably choose differently. Offered because
/// the spec's own worked example is unambiguous enough to be worth
/// naming as a default rather than leaving every caller to
/// re-transcribe the same six-way match themselves.
pub fn suggested_emergency_priority(kind: EmergencyFileKind) -> EmergencyFilePriority {
    use EmergencyFileKind as K;
    use EmergencyFilePriority as P;
    match kind {
        K::SosText | K::Location => P::Highest,
        K::Thumbnail | K::Voice => P::High,
        K::FullImage => P::Medium,
        K::Video => P::LowBulk,
    }
}

/// §70's own three named destinations. "Part 02 identity resolves
/// target devices" — this crate doesn't resolve `Account`/`AllDevices`
/// into a concrete device list itself; that resolution is
/// `siar-identity-multidevice`'s own job (Part 02), consumed as an
/// input by whatever layer turns a `FileDeliveryTarget` into real
/// per-device transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileDeliveryTarget {
    Account(AccountId),
    SpecificDevice(DeviceId),
    AllDevices,
}

/// §71's own four named policies. `FullFileToSelectedDevices` takes a
/// real `Vec<DeviceId>` rather than reusing
/// [`FileDeliveryTarget::SpecificDevice`] (singular) — §71's own
/// example implies more than one selected device is a normal case
/// ("full file to selected devices," plural), and collapsing that to
/// repeated single-device policies would lose the fact that it's one
/// sync decision covering a named set, not several independent ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnDeviceSyncPolicy {
    MetadataEverywhere,
    ThumbnailEverywhere,
    FullFileOnDemand,
    FullFileToSelectedDevices(Vec<DeviceId>),
}

/// §72, field-for-field: an advertisement of possession, never of
/// content — nothing here carries a [`crate::descriptor::FileMetadata`]
/// or anything else that would describe what the blob actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobAvailabilityAdvertisement {
    pub blob_id: BlobId,
    pub advertised_to: DeviceId,
}

/// §72's own two preconditions, both required: "only where privacy
/// policy permits" AND "should occur inside authorized sessions." A
/// caller can't satisfy this with just one — an authorized session
/// with a privacy policy that forbids advertisement is still a no, and
/// vice versa.
pub fn may_advertise_availability(privacy_policy_allows: bool, session_authorized: bool) -> bool {
    privacy_policy_allows && session_authorized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_s_own_worked_example_maps_exactly() {
        use EmergencyFileKind as K;
        use EmergencyFilePriority as P;
        assert_eq!(suggested_emergency_priority(K::SosText), P::Highest);
        assert_eq!(suggested_emergency_priority(K::Location), P::Highest);
        assert_eq!(suggested_emergency_priority(K::Thumbnail), P::High);
        assert_eq!(suggested_emergency_priority(K::Voice), P::High);
        assert_eq!(suggested_emergency_priority(K::FullImage), P::Medium);
        assert_eq!(suggested_emergency_priority(K::Video), P::LowBulk);
    }

    #[test]
    fn priority_tiers_rank_most_urgent_first() {
        assert!(EmergencyFilePriority::Highest < EmergencyFilePriority::High);
        assert!(EmergencyFilePriority::High < EmergencyFilePriority::Medium);
        assert!(EmergencyFilePriority::Medium < EmergencyFilePriority::LowBulk);
    }

    #[test]
    fn advertising_requires_both_preconditions_at_once() {
        assert!(!may_advertise_availability(false, true));
        assert!(!may_advertise_availability(true, false));
        assert!(!may_advertise_availability(false, false));
        assert!(may_advertise_availability(true, true));
    }
}
