//! §69 "Cancel Bundle".
//!
//! The state-machine half of this section (which states may legally be
//! cancelled) lives on [`crate::state::BundleState::is_cancellable`] —
//! this module is the other half: the actual wire message an origin
//! sends. "Cancellation propagation is best effort" and "cannot erase
//! already-delivered plaintext" are both facts about the real world
//! (no encounter transport exists in this crate to propagate over; a
//! peer that already decrypted and displayed a bundle's payload cannot
//! be made to un-see it), not things code can enforce — named here
//! rather than silently implied as solved.

use serde::{Deserialize, Serialize};

use crate::types::BundleId;

/// §69, field-for-field. `issued_at_millis` isn't named in the spec's
/// own bare `BundleCancel` snippet but is the same "when was this
/// issued" field every other timestamped message in this crate
/// carries (e.g. [`crate::bundle::DtnBundle::created_at_millis`]) —
/// without it, a relay has no way to prefer a newer cancel over a
/// stale, re-delivered duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleCancel {
    pub bundle_id: BundleId,
    pub issued_at_millis: u64,
}

impl BundleCancel {
    pub fn new(bundle_id: BundleId, issued_at_millis: u64) -> Self {
        Self {
            bundle_id,
            issued_at_millis,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_cancel_round_trips_through_postcard() {
        let cancel = BundleCancel::new(BundleId::new(), 12_345);
        let bytes = postcard::to_allocvec(&cancel).unwrap();
        let decoded: BundleCancel = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, cancel);
    }
}
