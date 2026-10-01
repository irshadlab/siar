//! §59 "Bundle Validation", §60 "Oversized Bundle", §61 "Malformed
//! Metadata".
//!
//! §61 gets no code of its own: "malformed bundle should not affect
//! other stored bundles; parsing must be isolated and bounded" is
//! resolved by construction, not by a check — `serde`/`postcard`
//! deserializes one bundle's own bytes into one `DtnBundle` value with
//! no shared mutable parse state across calls, so a malformed input
//! can only ever fail (or succeed) on its own, never corrupt a
//! previously-parsed bundle sitting in
//! [`crate::store::InMemoryBundleStore`]. Naming this rather than
//! silently implying it needed a type.
//!
//! §59's own checklist has two named items this crate genuinely has no
//! field for yet — `version` (no bundle-format version field exists
//! anywhere in [`crate::bundle::DtnBundle`]) and `header size` (there's
//! no separate header/payload split at the type level to measure) —
//! flagged rather than faked with a check that would always trivially
//! pass.

use serde::{Deserialize, Serialize};

use crate::bundle::DtnBundle;
use crate::payload::InlinePayloadTooLarge;
use crate::types::{DtnDestination, RouteToken};

/// §60's own bound — the actual serialized size of the bundle as it
/// arrived, not any field inside it a remote peer could lie about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleValidationLimits {
    pub max_wire_size_bytes: usize,
}

/// Not `Serialize`/`Deserialize` — this is a local validation
/// outcome, never sent over the wire, and one of its variants wraps
/// [`InlinePayloadTooLarge`], which itself has no need to serialize.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BundleValidationError {
    /// §60: "reject before allocation/storage reservation. Never trust
    /// remote size fields" — this is checked against the bundle's own
    /// real serialized byte length, not a size field inside it.
    #[error("bundle is {actual} bytes, over the {max}-byte wire limit")]
    OversizedBundle { actual: usize, max: usize },
    #[error(transparent)]
    InlinePayloadTooLarge(#[from] InlinePayloadTooLarge),
    #[error("hop limit is zero — cannot ever be forwarded")]
    ZeroHopLimit,
    #[error("bundle is already expired")]
    AlreadyExpired,
    #[error("destination route token is empty")]
    EmptyDestination,
    #[error("payload hash does not match the supplied payload bytes")]
    IntegrityMismatch,
}

/// §59's checklist, in order, for everything this crate has a real
/// field to check (see this module's own top doc comment for the two
/// items it honestly doesn't). `payload_bytes` is `Option` because
/// integrity can only be checked when the actual bytes are on hand —
/// always true for [`crate::payload::PayloadReference::Inline`],
/// often not yet true for `Blob`/`Chunk` references at the point a
/// bundle envelope is first validated; `None` skips that one check
/// rather than failing it, since "not yet checkable" and "checked and
/// failed" are different things.
pub fn validate_bundle(
    bundle: &DtnBundle,
    payload_bytes: Option<&[u8]>,
    now_millis: u64,
    limits: &BundleValidationLimits,
) -> Result<(), BundleValidationError> {
    let wire_bytes = postcard::to_allocvec(bundle)
        .expect("postcard serialization of DtnBundle is infallible for this shape")
        .len();
    if wire_bytes > limits.max_wire_size_bytes {
        return Err(BundleValidationError::OversizedBundle {
            actual: wire_bytes,
            max: limits.max_wire_size_bytes,
        });
    }
    bundle.payload_ref.validate()?;
    if bundle.hop_limit == 0 {
        return Err(BundleValidationError::ZeroHopLimit);
    }
    if bundle.is_expired(now_millis) {
        return Err(BundleValidationError::AlreadyExpired);
    }
    if destination_route_token(&bundle.destination).is_some_and(|token| token.0.is_empty()) {
        return Err(BundleValidationError::EmptyDestination);
    }
    if let Some(payload_bytes) = payload_bytes {
        if !bundle.integrity.verify_payload_hash(payload_bytes) {
            return Err(BundleValidationError::IntegrityMismatch);
        }
    }
    Ok(())
}

fn destination_route_token(destination: &DtnDestination) -> Option<&RouteToken> {
    match destination {
        DtnDestination::DeviceOpaque(token)
        | DtnDestination::AccountOpaque(token)
        | DtnDestination::GroupOpaque(token) => Some(token),
        DtnDestination::LocalBroadcast(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleIntegrity;
    use crate::payload::PayloadReference;
    use crate::types::{DtnPriority, DtnSource, ForwardingClass, PayloadTypeId};

    fn valid_bundle() -> DtnBundle {
        let payload = b"hello".to_vec();
        DtnBundle {
            bundle_id: crate::types::BundleId::new(),
            source: DtnSource(RouteToken(vec![1])),
            destination: DtnDestination::DeviceOpaque(RouteToken(vec![2])),
            payload_type: PayloadTypeId(1),
            created_at_millis: 0,
            expires_at_millis: 10_000,
            priority: DtnPriority::Normal,
            hop_limit: 4,
            replication_budget: 2,
            forwarding_class: ForwardingClass::SprayAndWait,
            integrity: BundleIntegrity {
                payload_hash: *blake3::hash(&payload).as_bytes(),
                origin_signature: None,
            },
            payload_ref: PayloadReference::Inline(payload),
        }
    }

    fn generous_limits() -> BundleValidationLimits {
        BundleValidationLimits {
            max_wire_size_bytes: 1_000_000,
        }
    }

    #[test]
    fn a_well_formed_bundle_with_matching_payload_bytes_passes_every_check() {
        let bundle = valid_bundle();
        let payload = b"hello";
        assert_eq!(
            validate_bundle(&bundle, Some(payload), 0, &generous_limits()),
            Ok(())
        );
    }

    #[test]
    fn integrity_is_not_checked_when_payload_bytes_are_unavailable() {
        let bundle = valid_bundle();
        assert_eq!(
            validate_bundle(&bundle, None, 0, &generous_limits()),
            Ok(())
        );
    }

    #[test]
    fn tampered_payload_bytes_fail_the_integrity_check() {
        let bundle = valid_bundle();
        let result = validate_bundle(&bundle, Some(b"tampered"), 0, &generous_limits());
        assert_eq!(result, Err(BundleValidationError::IntegrityMismatch));
    }

    #[test]
    fn a_bundle_over_the_real_wire_size_limit_is_rejected_before_anything_else() {
        let bundle = valid_bundle();
        let tiny_limits = BundleValidationLimits {
            max_wire_size_bytes: 4,
        };
        let result = validate_bundle(&bundle, Some(b"hello"), 0, &tiny_limits);
        assert!(matches!(
            result,
            Err(BundleValidationError::OversizedBundle { .. })
        ));
    }

    #[test]
    fn a_zero_hop_limit_is_rejected() {
        let mut bundle = valid_bundle();
        bundle.hop_limit = 0;
        assert_eq!(
            validate_bundle(&bundle, None, 0, &generous_limits()),
            Err(BundleValidationError::ZeroHopLimit)
        );
    }

    #[test]
    fn an_already_expired_bundle_is_rejected() {
        let bundle = valid_bundle();
        let result = validate_bundle(&bundle, None, 10_000, &generous_limits());
        assert_eq!(result, Err(BundleValidationError::AlreadyExpired));
    }

    #[test]
    fn an_empty_opaque_destination_is_rejected() {
        let mut bundle = valid_bundle();
        bundle.destination = DtnDestination::DeviceOpaque(RouteToken(vec![]));
        assert_eq!(
            validate_bundle(&bundle, None, 0, &generous_limits()),
            Err(BundleValidationError::EmptyDestination)
        );
    }

    #[test]
    fn a_local_broadcast_destination_has_no_route_token_to_validate() {
        let mut bundle = valid_bundle();
        bundle.destination =
            DtnDestination::LocalBroadcast(crate::types::BroadcastScope { radius_hops: 2 });
        assert_eq!(
            validate_bundle(&bundle, None, 0, &generous_limits()),
            Ok(())
        );
    }
}
