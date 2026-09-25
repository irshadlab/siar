//! Closes this crate's own standing gap ("no §22 audit-trail event
//! log" — per [[resilient-mesh]] project memory) by turning this
//! crate's own real operations — linking a device, revoking one,
//! verifying a revocation — into `siar-event-log` [`NewEvent`]s a
//! caller can actually append.
//!
//! This module deliberately only *constructs* events; it never calls
//! `EventStore::append` itself. Every "decide" module elsewhere in
//! this workspace keeps the same split (`siar_dtn_bundle::forwarding::
//! decide_forwarding` decides a forwarding action but never dials a
//! transport; `siar_routing_policy::plan` picks a route but never
//! opens a connection) — this crate stays a policy layer, not an I/O
//! layer, and pulling in `async-trait`/an executor just to call
//! `append` here would break that pattern for no benefit, since the
//! caller already has to own the `EventStore` instance regardless.

use crate::directory::DeviceStatus;
use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;

/// One stream per account's identity history — matches
/// `siar_dtn_bundle`/`siar_blob_manifest`'s own established pattern of
/// deriving a `StreamId` from a stable name via
/// [`StreamId::from_name`] rather than a random id, so every caller
/// who knows the account arrives at the same stream without needing a
/// separate lookup table.
pub fn identity_stream_id(account: AccountId) -> StreamId {
    StreamId::from_name(&format!("identity:{account}"))
}

/// Plain numeric tags, caller/module assigns constants — the same
/// choice `siar_dtn_bundle::types::PayloadTypeId` and
/// `siar_event_log`'s own doc comments already document the reasoning
/// for (a stable, versionable wire tag beats a string that can typo).
pub const EVENT_TYPE_DEVICE_LINKED: EventTypeId = EventTypeId(1);
pub const EVENT_TYPE_DEVICE_REVOKED: EventTypeId = EventTypeId(2);
pub const EVENT_TYPE_REVOCATION_VERIFIED: EventTypeId = EventTypeId(3);
/// §111's own remaining four named event types this round's new
/// operations (`suspend_device`, `rotate_device_key`,
/// `rotate_root_key`, `add_device_via_recovery`, the fork-detection
/// path in `trust_store`) had no audit constructor for until now —
/// added in the same tag-numbering scheme, new tags appended rather
/// than renumbering the first three.
pub const EVENT_TYPE_DEVICE_SUSPENDED: EventTypeId = EventTypeId(4);
pub const EVENT_TYPE_DEVICE_ROTATED: EventTypeId = EventTypeId(5);
pub const EVENT_TYPE_ROOT_ROTATED: EventTypeId = EventTypeId(6);
pub const EVENT_TYPE_RECOVERY_USED: EventTypeId = EventTypeId(7);
pub const EVENT_TYPE_FORK_DETECTED: EventTypeId = EventTypeId(8);

/// The typed payload behind each of the three event types above —
/// postcard-serialized into [`NewEvent::payload`], mirroring
/// `siar_dtn_bundle::payload::PayloadReference`'s own "typed enum, not
/// raw bytes with a convention" choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityAuditPayload {
    DeviceLinked {
        device_id: DeviceId,
        generation: u64,
    },
    DeviceRevoked {
        device_id: DeviceId,
        generation: u64,
    },
    RevocationVerified {
        device_id: DeviceId,
        generation: u64,
    },
    DeviceSuspended {
        device_id: DeviceId,
        generation: u64,
    },
    DeviceRotated {
        device_id: DeviceId,
        generation: u64,
    },
    RootRotated {
        generation: u64,
    },
    RecoveryUsed {
        device_id: DeviceId,
        generation: u64,
    },
    /// §57's fork detection, given a real audit trail entry — the
    /// account has no root-key `device_id` to attach this to, so it's
    /// the one variant here without one.
    ForkDetected {
        generation: u64,
    },
}

impl IdentityAuditPayload {
    /// §56 "Durability Classes" — see `siar_event_log::durability`'s
    /// own doc comment for the rule, and its own worked example:
    /// `DeviceRevoked` → `Critical` is this crate's own event, named
    /// in the spec itself. The same reasoning extends to every
    /// sibling security-state variant here — `RevocationVerified`/
    /// `DeviceSuspended`/`DeviceRotated`/`RootRotated`/`RecoveryUsed`/
    /// `ForkDetected` all describe a change to WHO is trusted; losing
    /// any of them risks the same kind of unsafe inversion
    /// `DeviceRevoked` does (an old key, or a kicked-off device,
    /// silently still trusted) — all `Critical`. Only `DeviceLinked`
    /// is `Durable`: losing it fails CLOSED (a real device looks
    /// unrecognized until it re-links), not open, so it doesn't meet
    /// this rule's own bar for `Critical`.
    pub fn durability_class(&self) -> siar_event_log::DurabilityClass {
        use siar_event_log::DurabilityClass as D;
        match self {
            Self::DeviceLinked { .. } => D::Durable,
            Self::DeviceRevoked { .. } => D::Critical,
            Self::RevocationVerified { .. } => D::Critical,
            Self::DeviceSuspended { .. } => D::Critical,
            Self::DeviceRotated { .. } => D::Critical,
            Self::RootRotated { .. } => D::Critical,
            Self::RecoveryUsed { .. } => D::Critical,
            Self::ForkDetected { .. } => D::Critical,
        }
    }

    fn event_type(&self) -> EventTypeId {
        match self {
            Self::DeviceLinked { .. } => EVENT_TYPE_DEVICE_LINKED,
            Self::DeviceRevoked { .. } => EVENT_TYPE_DEVICE_REVOKED,
            Self::RevocationVerified { .. } => EVENT_TYPE_REVOCATION_VERIFIED,
            Self::DeviceSuspended { .. } => EVENT_TYPE_DEVICE_SUSPENDED,
            Self::DeviceRotated { .. } => EVENT_TYPE_DEVICE_ROTATED,
            Self::RootRotated { .. } => EVENT_TYPE_ROOT_ROTATED,
            Self::RecoveryUsed { .. } => EVENT_TYPE_RECOVERY_USED,
            Self::ForkDetected { .. } => EVENT_TYPE_FORK_DETECTED,
        }
    }

    fn into_new_event(self) -> NewEvent {
        let event_type = self.event_type();
        let origin = self.origin();
        let payload =
            postcard::to_allocvec(&self).expect("IdentityAuditPayload always postcard-serializes");
        NewEvent {
            event_id: EventId::new(),
            event_type,
            schema_version: CURRENT_IDENTITY_AUDIT_SCHEMA_VERSION,
            created_at: Timestamp::now(),
            origin,
            correlation_id: None,
            causation_id: None,
            payload,
        }
    }

    /// `RootRotated`/`ForkDetected` have no single device to attribute
    /// origin to (a root rotation is an account-level act; a fork is
    /// discovered, not performed by any one device) — `EventOrigin::System`
    /// is the correct fit for both, not a fabricated device id.
    fn origin(&self) -> EventOrigin {
        match self {
            Self::DeviceLinked { device_id, .. }
            | Self::DeviceRevoked { device_id, .. }
            | Self::RevocationVerified { device_id, .. }
            | Self::DeviceSuspended { device_id, .. }
            | Self::DeviceRotated { device_id, .. }
            | Self::RecoveryUsed { device_id, .. } => EventOrigin::LocalDevice(*device_id),
            Self::RootRotated { .. } | Self::ForkDetected { .. } => EventOrigin::System,
        }
    }
}

/// A device successfully joined the account's [`crate::directory::DeviceDirectory`]
/// (§16-17/19-20's linking flow, already real elsewhere in this
/// crate) — call after that flow succeeds, with the directory's new
/// generation.
pub fn device_linked_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::DeviceLinked {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::revocation::revoke_device`] succeeded — call with its
/// returned [`crate::directory::DeviceDirectory`]'s new generation.
pub fn device_revoked_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::DeviceRevoked {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::revocation::verify_revocation`] succeeded on a remote
/// peer's directory — a distinct event from `DeviceRevoked` because
/// it can happen on a *different* device than the one that issued the
/// revocation (§25-27's whole point: every device independently
/// verifies a revocation it receives, it doesn't just trust that the
/// issuer did it correctly).
pub fn revocation_verified_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::RevocationVerified {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::device_state::suspend_device`] succeeded.
pub fn device_suspended_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::DeviceSuspended {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::rotation::rotate_device_key`] succeeded.
pub fn device_rotated_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::DeviceRotated {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::root_rotation::rotate_root_key`] succeeded and its
/// [`crate::root_rotation::RootRotation`] verified.
pub fn root_rotated_event(new_generation: u64) -> NewEvent {
    IdentityAuditPayload::RootRotated {
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::recovery::add_device_via_recovery`] succeeded.
pub fn recovery_used_event(device_id: DeviceId, new_generation: u64) -> NewEvent {
    IdentityAuditPayload::RecoveryUsed {
        device_id,
        generation: new_generation,
    }
    .into_new_event()
}

/// [`crate::trust_store::TrustedAccountStore::accept`] returned
/// [`crate::error::IdentityError::IdentityForkDetected`] (§57) — an
/// account-level event, not attributable to one device (see this
/// module's own note on `EventOrigin::System`).
pub fn fork_detected_event(generation: u64) -> NewEvent {
    IdentityAuditPayload::ForkDetected { generation }.into_new_event()
}

/// §9 "Versioned Event Schemas": the schema version [`NewEvent`]
/// carries alongside the payload bytes, not a magic number repeated at
/// every call site — same convention
/// `siar_messaging::events::CURRENT_MESSAGING_EVENT_SCHEMA_VERSION`/
/// `siar_blob_manifest::events::CURRENT_FILE_EVENT_SCHEMA_VERSION`
/// already established for the other two domain event catalogs. Bump
/// this, and add a real `V2` decode branch to [`decode_audit_payload`]
/// below, the day any of this module's variants' fields actually
/// change shape.
const CURRENT_IDENTITY_AUDIT_SCHEMA_VERSION: u16 = 1;

/// Same real bug `siar_messaging::events::MessagingEventDecodeError`'s
/// own doc comment describes (found there first, then found here too,
/// then in `siar_blob_manifest::events`, on the same audit pass): this
/// function used to run `postcard::from_bytes` straight against the
/// payload with no regard for the stored `schema_version` sitting
/// right next to it — §9's own named anti-pattern.
#[derive(Debug, thiserror::Error)]
pub enum AuditPayloadDecodeError {
    #[error(
        "unsupported identity audit event schema version {0} (highest known: {CURRENT_IDENTITY_AUDIT_SCHEMA_VERSION})"
    )]
    UnsupportedVersion(u16),
    #[error("payload did not decode as a valid identity audit event: {0}")]
    Malformed(#[from] postcard::Error),
}

/// Reconstructs the audit payload from a [`siar_event_log::store::StoredEvent`]'s
/// raw bytes — the read-side counterpart to the three constructors
/// above, so a caller building an actual audit-trail view doesn't have
/// to know the postcard encoding itself. `schema_version` should come
/// from the same
/// [`siar_event_log::envelope::EventEnvelope::schema_version`] the
/// payload itself was read alongside — see
/// [`AuditPayloadDecodeError`]'s own doc comment for why this isn't
/// just `decode_audit_payload(payload)` anymore.
pub fn decode_audit_payload(
    schema_version: u16,
    payload: &[u8],
) -> Result<IdentityAuditPayload, AuditPayloadDecodeError> {
    match schema_version {
        1 => Ok(postcard::from_bytes(payload)?),
        other => Err(AuditPayloadDecodeError::UnsupportedVersion(other)),
    }
}

/// True if `status` is the kind of status transition this module
/// bothers auditing at all — [`DeviceStatus::Expired`] has no
/// constructor function above because nothing in this crate currently
/// produces that transition (see this crate's own `lib.rs` gap list);
/// this function exists so a future caller adding that transition has
/// one obvious place to extend, rather than the omission being silent.
pub fn is_audited_status(status: DeviceStatus) -> bool {
    matches!(status, DeviceStatus::Active | DeviceStatus::Revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_revoked_is_critical_the_spec_own_worked_example_device_linked_is_not() {
        use siar_event_log::DurabilityClass;
        let device = DeviceId::new();
        assert_eq!(
            IdentityAuditPayload::DeviceRevoked {
                device_id: device,
                generation: 2,
            }
            .durability_class(),
            DurabilityClass::Critical
        );
        assert_eq!(
            IdentityAuditPayload::DeviceLinked {
                device_id: device,
                generation: 1,
            }
            .durability_class(),
            DurabilityClass::Durable
        );
        assert_eq!(
            IdentityAuditPayload::ForkDetected { generation: 3 }.durability_class(),
            DurabilityClass::Critical
        );
    }

    #[test]
    fn same_account_always_derives_the_same_stream_id() {
        let account = AccountId::new();
        assert_eq!(identity_stream_id(account), identity_stream_id(account));
    }

    #[test]
    fn different_accounts_derive_different_stream_ids() {
        assert_ne!(
            identity_stream_id(AccountId::new()),
            identity_stream_id(AccountId::new())
        );
    }

    #[test]
    fn device_linked_event_round_trips_through_postcard() {
        let device = DeviceId::new();
        let event = device_linked_event(device, 3);
        assert_eq!(event.event_type, EVENT_TYPE_DEVICE_LINKED);

        let decoded = decode_audit_payload(event.schema_version, &event.payload).unwrap();
        assert_eq!(
            decoded,
            IdentityAuditPayload::DeviceLinked {
                device_id: device,
                generation: 3
            }
        );
    }

    #[test]
    fn device_revoked_event_round_trips_through_postcard() {
        let device = DeviceId::new();
        let event = device_revoked_event(device, 5);
        assert_eq!(event.event_type, EVENT_TYPE_DEVICE_REVOKED);

        let decoded = decode_audit_payload(event.schema_version, &event.payload).unwrap();
        assert_eq!(
            decoded,
            IdentityAuditPayload::DeviceRevoked {
                device_id: device,
                generation: 5
            }
        );
    }

    #[test]
    fn revocation_verified_event_round_trips_through_postcard() {
        let device = DeviceId::new();
        let event = revocation_verified_event(device, 5);
        assert_eq!(event.event_type, EVENT_TYPE_REVOCATION_VERIFIED);

        let decoded = decode_audit_payload(event.schema_version, &event.payload).unwrap();
        assert_eq!(
            decoded,
            IdentityAuditPayload::RevocationVerified {
                device_id: device,
                generation: 5
            }
        );
    }

    #[test]
    fn each_event_type_gets_a_distinct_tag() {
        let tags = [
            EVENT_TYPE_DEVICE_LINKED,
            EVENT_TYPE_DEVICE_REVOKED,
            EVENT_TYPE_REVOCATION_VERIFIED,
            EVENT_TYPE_DEVICE_SUSPENDED,
            EVENT_TYPE_DEVICE_ROTATED,
            EVENT_TYPE_ROOT_ROTATED,
            EVENT_TYPE_RECOVERY_USED,
            EVENT_TYPE_FORK_DETECTED,
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn active_and_revoked_are_audited_but_expired_is_flagged_as_not_yet() {
        assert!(is_audited_status(DeviceStatus::Active));
        assert!(is_audited_status(DeviceStatus::Revoked));
        assert!(!is_audited_status(DeviceStatus::Expired));
    }

    #[test]
    fn spec_111_root_rotated_and_fork_detected_use_system_origin_not_a_device() {
        let event = root_rotated_event(5);
        assert_eq!(event.origin, EventOrigin::System);
        let event = fork_detected_event(3);
        assert_eq!(event.origin, EventOrigin::System);
    }

    #[test]
    fn decoding_rejects_an_unsupported_schema_version() {
        let event = device_linked_event(DeviceId::new(), 1);

        // Same proof `siar_messaging::events`/`siar_blob_manifest::
        // events`'s own equivalent tests make: the CURRENT
        // `IdentityAuditPayload` shape could decode this payload just
        // fine — this asserts the version gate rejects it anyway when
        // told it came from a schema version this module doesn't
        // recognize.
        let result = decode_audit_payload(99, &event.payload);
        assert!(matches!(
            result,
            Err(AuditPayloadDecodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn spec_111_the_five_new_event_constructors_round_trip_through_postcard() {
        let device = DeviceId::new();
        for event in [
            device_suspended_event(device, 1),
            device_rotated_event(device, 2),
            root_rotated_event(3),
            recovery_used_event(device, 4),
            fork_detected_event(5),
        ] {
            let decoded = decode_audit_payload(event.schema_version, &event.payload).unwrap();
            assert_eq!(decoded.event_type(), event.event_type);
        }
    }
}
