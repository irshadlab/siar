//! §6 "Bundle Structure", §8 "Bundle Security Principle", §13 "Bundle
//! Immutability", §20 "Expiry", §21 "Hop Limit", §22 "Replication
//! Budget".

use serde::{Deserialize, Serialize};

use crate::payload::PayloadReference;
use crate::types::{
    BundleId, DtnDestination, DtnPriority, DtnSource, ForwardingClass, PayloadTypeId,
};

/// §8: relays carry ciphertext, never plaintext. This crate has no
/// decrypt path and no reason for one — the same stance `siar-dtn`'s
/// existing `MeshBundle` already documents for its own `ciphertext`
/// field ("intermediates forward ciphertext, never see plaintext").
/// A payload hash for integrity checking without needing to understand
/// the payload's own structure (§5: "the relay does not need to
/// understand the application payload").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleIntegrity {
    pub payload_hash: [u8; 32],
    /// §9: "trusted to carry" vs "trusted to read" — an origin
    /// signature lets a relay/destination verify who actually created
    /// this bundle without needing to decrypt anything, so a relay can
    /// still validate provenance while remaining unable to read
    /// content. The signature bytes themselves aren't modeled here
    /// (that's a `siar-crypto`/`siar-identity-multidevice` integration
    /// — see this crate's own top doc comment); this field is a
    /// placeholder slot for wherever that signature ultimately lives,
    /// deliberately left optional so a real signer can fill it in
    /// later without changing this struct's shape. `Vec<u8>`, not
    /// `[u8; 64]` — the same serde derive limitation (arrays past 32
    /// elements need an extra crate)
    /// `siar_identity_multidevice::DeviceCertificate::signature`'s own
    /// doc comment already documents, hit again here for real via a
    /// compile error, not guessed preemptively.
    pub origin_signature: Option<Vec<u8>>,
}

impl BundleIntegrity {
    /// §59 "Bundle Validation"'s own "signature/integrity" item, the
    /// half this crate can actually check without `siar-crypto`: a
    /// relay never needs to decrypt a payload to confirm it's the same
    /// bytes the origin hashed (§5: "the relay does not need to
    /// understand the application payload"). Does NOT verify
    /// `origin_signature` — that needs the real signer/verifier this
    /// crate deliberately doesn't have; a caller with `siar-crypto`
    /// available layers that check on top, it isn't skipped silently
    /// here.
    pub fn verify_payload_hash(&self, payload_bytes: &[u8]) -> bool {
        // `blake3::Hash`'s `PartialEq` is constant-time; comparing raw
        // byte arrays is not.
        blake3::hash(payload_bytes) == blake3::Hash::from_bytes(self.payload_hash)
    }
}

/// §6, field-for-field, plus [`BundleIntegrity`] (§8) folded in as
/// `integrity` matches the spec's own struct.
///
/// §13: "routing-critical bundle fields should be immutable except
/// hop-local metadata." This struct doesn't enforce that at the type
/// level (Rust has no field-level immutability short of splitting into
/// two structs) — [`DtnBundle::forwarded`]/[`DtnBundle::consume_replication`]
/// are the only sanctioned mutation paths this crate provides, both
/// hop-local per §13's own list.
///
/// §14 "Bundle Envelope vs Local Record": every field here IS the wire
/// form — [`crate::store::WireBundle`] is a type alias making that
/// explicit for anyone reading against the spec's own naming. The
/// local-only half of §14's split (storage path, retry count, peer
/// history, custody state) now lives on
/// [`crate::store::StoredBundle`], not on this struct — see that
/// type's own doc comment for the full accounting of which field
/// satisfies which of §14's four named local-record items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DtnBundle {
    pub bundle_id: BundleId,
    pub source: DtnSource,
    pub destination: DtnDestination,
    pub payload_type: PayloadTypeId,
    pub created_at_millis: u64,
    pub expires_at_millis: u64,
    pub priority: DtnPriority,
    pub hop_limit: u8,
    pub replication_budget: u8,
    pub forwarding_class: ForwardingClass,
    pub payload_ref: PayloadReference,
    pub integrity: BundleIntegrity,
}

impl DtnBundle {
    /// §20: "Expired bundles are never forwarded."
    pub fn is_expired(&self, now_millis: u64) -> bool {
        now_millis >= self.expires_at_millis
    }

    /// §21: "Every forward decrements local remaining hop budget. At
    /// zero: do not forward further." Same `Option`-returning shape as
    /// `siar_dtn::bundle::MeshBundle::forwarded` (this workspace's
    /// existing, differently-modeled DTN crate) for the same reason:
    /// `None` means "this was the last hop, treat exactly like drop."
    pub fn forwarded(mut self) -> Option<Self> {
        if self.hop_limit == 0 {
            return None;
        }
        self.hop_limit -= 1;
        Some(self)
    }

    /// §22: bounds duplicate copies independent of hop count. Returns
    /// `true` (and consumes one unit) if a copy may still be handed to
    /// a new carrier, `false` if the budget is already exhausted.
    pub fn consume_replication(&mut self) -> bool {
        if self.replication_budget == 0 {
            return false;
        }
        self.replication_budget -= 1;
        true
    }
}

/// Shared minimal bundle for sibling-module unit tests.
#[cfg(test)]
pub(crate) fn test_bundle() -> DtnBundle {
    use crate::types::RouteToken;
    DtnBundle {
        bundle_id: BundleId::new(),
        source: DtnSource(RouteToken(vec![1])),
        destination: DtnDestination::DeviceOpaque(RouteToken(vec![2])),
        payload_type: PayloadTypeId(1),
        created_at_millis: 0,
        expires_at_millis: 1_000_000,
        priority: DtnPriority::Normal,
        hop_limit: 8,
        replication_budget: 4,
        forwarding_class: ForwardingClass::SprayAndWait,
        payload_ref: PayloadReference::Inline(vec![1, 2, 3]),
        integrity: BundleIntegrity {
            payload_hash: [0u8; 32],
            origin_signature: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RouteToken;

    fn bundle(hop_limit: u8, replication_budget: u8) -> DtnBundle {
        DtnBundle {
            bundle_id: BundleId::new(),
            source: DtnSource(RouteToken(vec![1])),
            destination: DtnDestination::DeviceOpaque(RouteToken(vec![2])),
            payload_type: PayloadTypeId(1),
            created_at_millis: 0,
            expires_at_millis: 1_000,
            priority: DtnPriority::Normal,
            hop_limit,
            replication_budget,
            forwarding_class: ForwardingClass::SprayAndWait,
            payload_ref: PayloadReference::Inline(vec![9, 9, 9]),
            integrity: BundleIntegrity {
                payload_hash: [0u8; 32],
                origin_signature: None,
            },
        }
    }

    #[test]
    fn is_expired_true_once_now_reaches_expires_at() {
        let b = bundle(4, 2);
        assert!(!b.is_expired(999));
        assert!(b.is_expired(1_000));
    }

    #[test]
    fn forwarded_decrements_hop_limit_until_it_hits_zero() {
        let b = bundle(1, 2)
            .forwarded()
            .expect("hop_limit 1 -> 0 should still forward");
        assert_eq!(b.hop_limit, 0);
        assert!(
            b.forwarded().is_none(),
            "hop_limit already 0 must not forward further"
        );
    }

    #[test]
    fn consume_replication_stops_at_zero() {
        let mut b = bundle(4, 1);
        assert!(b.consume_replication());
        assert_eq!(b.replication_budget, 0);
        assert!(!b.consume_replication());
    }

    #[test]
    fn verify_payload_hash_accepts_the_matching_bytes_and_rejects_everything_else() {
        let payload = b"real payload bytes";
        let integrity = BundleIntegrity {
            payload_hash: *blake3::hash(payload).as_bytes(),
            origin_signature: None,
        };
        assert!(integrity.verify_payload_hash(payload));
        assert!(!integrity.verify_payload_hash(b"tampered payload bytes"));
    }
}
