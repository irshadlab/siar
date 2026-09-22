//! `04-offline-event-log-architecture.md` §36 "DTN Events", §79 "DTN
//! Integration" ("Part 06 should persist: bundle creation, store,
//! forward, delivery, expiry, while keeping peer-encounter telemetry
//! mostly operational"), §92 Phase 3's fourth domain (after identity,
//! messaging, files — see those crates' own `events.rs`/`audit_log.rs`
//! for the other three; same shape here on purpose).
//!
//! `EventTypeId` constants this crate owns, a typed payload enum
//! (postcard-serialized, schema-version-checked on decode — §9, same
//! discipline `siar_messaging::events`/`siar_blob_manifest::events`
//! both already apply, built in from the start here rather than fixed
//! after the fact), an `into_new_event` constructor per event, a
//! `decode` counterpart, and the same "construct only, never append"
//! discipline every sibling domain module already states outright —
//! this crate's own `lib.rs` already says as much for the crate as a
//! whole (no `siar-storage`/transport dependency — see that module's
//! own scope section).
//!
//! ## Streams
//!
//! One stream per bundle ([`bundle_stream_id`]) — the natural
//! aggregate boundary, same reasoning `siar_blob_manifest::events`'s
//! own doc comment gives for "one stream per transfer": every caller
//! who knows the `BundleId` arrives at the same stream with no lookup
//! table.
//!
//! ## Which of `BundleState`'s 11 states got an event, and which didn't
//!
//! §79's own text: "keeping peer-encounter telemetry mostly
//! operational" — meaning most of the hop-by-hop mechanics stay OUT of
//! durable history, only the meaningful lifecycle facts go in. Nine of
//! [`crate::state::BundleState`]'s eleven states get an event here;
//! `Eligible` does not — it is scheduling-internal (§2 "Do Not
//! Event-Source Everything": becoming eligible to forward is a
//! decision the forwarding scheduler makes moment to moment, not a
//! durable fact worth an audit trail entry on its own), and per
//! [`crate::state::BundleState`]'s own doc comment, repeated
//! `Forward` transitions ("ForwardedAgain") collapse into the same
//! `BundleState::Forwarded` state rather than a distinct one — this
//! module follows that same collapse: one [`DtnEvent::BundleForwarded`]
//! variant covers every hop, deliberately with no per-hop peer/relay
//! detail in its payload (that IS the "peer-encounter telemetry" §79
//! says to keep out).
//!
//! ## No `siar_domain` dependency, by design
//!
//! This crate has no dependency on `siar_domain` (see this crate's own
//! `Cargo.toml` comment) — [`DtnEvent::into_new_event`] therefore takes
//! `origin: EventOrigin` purely as an opaque type re-exported from
//! `siar_event_log` and never constructs one itself; the actual
//! `siar_domain::DeviceId` a real `LocalDevice`/`RemoteDevice` origin
//! needs comes from whichever external caller has one. This module's
//! own tests use `EventOrigin::System`/`Imported` for exactly this
//! reason — sidestepping the dependency rather than adding one just
//! for test convenience.
//!
//! ## Numeric range
//!
//! 300-308 — the fourth block in the informal per-domain convention
//! (identity 1-8, messaging 100-108, files 200-208), same real,
//! unenforced gap named in every one of those modules' own doc
//! comments.

use serde::{Deserialize, Serialize};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;
use thiserror::Error;

use crate::types::{BundleId, DtnDestination, DtnPriority, PayloadTypeId};

/// One stream per bundle — see this module's own doc comment.
pub fn bundle_stream_id(bundle_id: BundleId) -> StreamId {
    StreamId::from_name(&format!("dtn-bundle:{bundle_id}"))
}

pub const EVENT_TYPE_BUNDLE_CREATED: EventTypeId = EventTypeId(300);
pub const EVENT_TYPE_BUNDLE_STORED: EventTypeId = EventTypeId(301);
pub const EVENT_TYPE_BUNDLE_FORWARDED: EventTypeId = EventTypeId(302);
pub const EVENT_TYPE_BUNDLE_DESTINATION_REACHED: EventTypeId = EventTypeId(303);
pub const EVENT_TYPE_BUNDLE_ACKNOWLEDGED: EventTypeId = EventTypeId(304);
pub const EVENT_TYPE_BUNDLE_COMPLETED: EventTypeId = EventTypeId(305);
pub const EVENT_TYPE_BUNDLE_EXPIRED: EventTypeId = EventTypeId(306);
pub const EVENT_TYPE_BUNDLE_EVICTED: EventTypeId = EventTypeId(307);
pub const EVENT_TYPE_BUNDLE_CANCELLED: EventTypeId = EventTypeId(308);

/// §36/§79's named lifecycle, minus `Eligible` — see this module's own
/// doc comment for why. `Rejected` (from [`crate::state::BundleState`])
/// also has no event here: a rejection happens at creation time, before
/// a bundle is ever durably `Stored` (see the transition table's own
/// `Created -> Rejected` edge) — there is no meaningful "this bundle
/// existed and then was rejected" history to record distinct from
/// simply never having created one, so `BundleCreated` for a bundle
/// that gets immediately rejected is deliberately never appended by a
/// real caller in the first place (a decision that caller makes, not
/// this module).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DtnEvent {
    BundleCreated {
        bundle_id: BundleId,
        destination: DtnDestination,
        priority: DtnPriority,
        payload_type: PayloadTypeId,
    },
    BundleStored {
        bundle_id: BundleId,
    },
    /// Covers every hop — see this module's own doc comment on why
    /// there's no per-hop peer/relay field.
    BundleForwarded {
        bundle_id: BundleId,
    },
    BundleDestinationReached {
        bundle_id: BundleId,
    },
    BundleAcknowledged {
        bundle_id: BundleId,
    },
    BundleCompleted {
        bundle_id: BundleId,
    },
    BundleExpired {
        bundle_id: BundleId,
    },
    BundleEvicted {
        bundle_id: BundleId,
    },
    BundleCancelled {
        bundle_id: BundleId,
    },
}

impl DtnEvent {
    pub fn event_type(&self) -> EventTypeId {
        match self {
            Self::BundleCreated { .. } => EVENT_TYPE_BUNDLE_CREATED,
            Self::BundleStored { .. } => EVENT_TYPE_BUNDLE_STORED,
            Self::BundleForwarded { .. } => EVENT_TYPE_BUNDLE_FORWARDED,
            Self::BundleDestinationReached { .. } => EVENT_TYPE_BUNDLE_DESTINATION_REACHED,
            Self::BundleAcknowledged { .. } => EVENT_TYPE_BUNDLE_ACKNOWLEDGED,
            Self::BundleCompleted { .. } => EVENT_TYPE_BUNDLE_COMPLETED,
            Self::BundleExpired { .. } => EVENT_TYPE_BUNDLE_EXPIRED,
            Self::BundleEvicted { .. } => EVENT_TYPE_BUNDLE_EVICTED,
            Self::BundleCancelled { .. } => EVENT_TYPE_BUNDLE_CANCELLED,
        }
    }

    pub fn bundle_id(&self) -> BundleId {
        match self {
            Self::BundleCreated { bundle_id, .. }
            | Self::BundleStored { bundle_id, .. }
            | Self::BundleForwarded { bundle_id, .. }
            | Self::BundleDestinationReached { bundle_id, .. }
            | Self::BundleAcknowledged { bundle_id, .. }
            | Self::BundleCompleted { bundle_id, .. }
            | Self::BundleExpired { bundle_id, .. }
            | Self::BundleEvicted { bundle_id, .. }
            | Self::BundleCancelled { bundle_id, .. } => *bundle_id,
        }
    }

    /// Thin wrapper over [`bundle_stream_id`] — same reason every
    /// sibling domain module's own `stream_id` method exists.
    pub fn stream_id(&self) -> StreamId {
        bundle_stream_id(self.bundle_id())
    }

    /// Caller-supplied `event_id`/`origin`/`correlation_id`/
    /// `causation_id`, matching `siar_messaging::events::
    /// MessagingEvent::into_new_event`'s own signature (§8) — not
    /// `siar_blob_manifest`/`siar_identity_multidevice`'s simpler
    /// origin-only form, since this module has no real caller yet
    /// either way and the messaging shape is the more complete
    /// precedent to build against once one exists.
    pub fn into_new_event(
        self,
        event_id: EventId,
        origin: EventOrigin,
        correlation_id: Option<siar_event_log::ids::CorrelationId>,
        causation_id: Option<EventId>,
    ) -> NewEvent {
        let event_type = self.event_type();
        let payload = postcard::to_allocvec(&self).expect("DtnEvent always postcard-serializes");
        NewEvent {
            event_id,
            event_type,
            schema_version: CURRENT_DTN_EVENT_SCHEMA_VERSION,
            created_at: Timestamp::now(),
            origin,
            correlation_id,
            causation_id,
            payload,
        }
    }
}

/// §9: bump this, and add a real `V2` decode branch to
/// [`decode_dtn_event`] below, the day any of this module's variants'
/// fields actually change shape — same convention every sibling
/// domain module's own constant already establishes.
pub const CURRENT_DTN_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum DtnEventDecodeError {
    #[error(
        "unsupported DTN event schema version {0} (highest known: {CURRENT_DTN_EVENT_SCHEMA_VERSION})"
    )]
    UnsupportedVersion(u16),
    #[error("payload did not decode as a valid DTN event: {0}")]
    Malformed(#[from] postcard::Error),
}

/// Read-side counterpart to [`DtnEvent::into_new_event`] — same role
/// every sibling domain module's own decode function plays.
/// `schema_version` should come from the same
/// [`siar_event_log::envelope::EventEnvelope::schema_version`] the
/// payload itself was read alongside.
pub fn decode_dtn_event(
    schema_version: u16,
    payload: &[u8],
) -> Result<DtnEvent, DtnEventDecodeError> {
    match schema_version {
        1 => Ok(postcard::from_bytes(payload)?),
        other => Err(DtnEventDecodeError::UnsupportedVersion(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BroadcastScope, RouteToken};

    fn sample_created(bundle_id: BundleId) -> DtnEvent {
        DtnEvent::BundleCreated {
            bundle_id,
            destination: DtnDestination::LocalBroadcast(BroadcastScope { radius_hops: 3 }),
            priority: DtnPriority::Normal,
            payload_type: PayloadTypeId(1),
        }
    }

    #[test]
    fn same_bundle_always_derives_the_same_stream_id() {
        let bundle_id = BundleId::new();
        assert_eq!(bundle_stream_id(bundle_id), bundle_stream_id(bundle_id));
    }

    #[test]
    fn different_bundles_derive_different_stream_ids() {
        assert_ne!(
            bundle_stream_id(BundleId::new()),
            bundle_stream_id(BundleId::new())
        );
    }

    #[test]
    fn bundle_created_round_trips_through_postcard_and_carries_its_own_stream() {
        let bundle_id = BundleId::new();
        let event = sample_created(bundle_id);
        assert_eq!(event.stream_id(), bundle_stream_id(bundle_id));

        let new_event =
            event
                .clone()
                .into_new_event(EventId::new(), EventOrigin::System, None, None);
        assert_eq!(new_event.event_type, EVENT_TYPE_BUNDLE_CREATED);
        assert_eq!(new_event.origin, EventOrigin::System);

        let decoded = decode_dtn_event(new_event.schema_version, &new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn decoding_rejects_an_unsupported_schema_version() {
        let event = DtnEvent::BundleStored {
            bundle_id: BundleId::new(),
        };
        let new_event = event.into_new_event(EventId::new(), EventOrigin::Imported, None, None);
        let result = decode_dtn_event(99, &new_event.payload);
        assert!(matches!(
            result,
            Err(DtnEventDecodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn each_event_type_gets_a_distinct_tag_and_does_not_collide_with_other_domains() {
        let tags = [
            EVENT_TYPE_BUNDLE_CREATED,
            EVENT_TYPE_BUNDLE_STORED,
            EVENT_TYPE_BUNDLE_FORWARDED,
            EVENT_TYPE_BUNDLE_DESTINATION_REACHED,
            EVENT_TYPE_BUNDLE_ACKNOWLEDGED,
            EVENT_TYPE_BUNDLE_COMPLETED,
            EVENT_TYPE_BUNDLE_EXPIRED,
            EVENT_TYPE_BUNDLE_EVICTED,
            EVENT_TYPE_BUNDLE_CANCELLED,
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
            // identity: 1-8, messaging: 100-108, files: 200-208 — see
            // this module's own doc comment.
            assert!(a.0 >= 300);
        }
    }

    #[test]
    fn every_variant_round_trips_and_reports_its_own_bundle_id() {
        let bundle_id = BundleId::new();
        let events = vec![
            DtnEvent::BundleStored { bundle_id },
            DtnEvent::BundleForwarded { bundle_id },
            DtnEvent::BundleDestinationReached { bundle_id },
            DtnEvent::BundleAcknowledged { bundle_id },
            DtnEvent::BundleCompleted { bundle_id },
            DtnEvent::BundleExpired { bundle_id },
            DtnEvent::BundleEvicted { bundle_id },
            DtnEvent::BundleCancelled { bundle_id },
        ];
        for event in events {
            assert_eq!(event.bundle_id(), bundle_id);
            let new_event =
                event
                    .clone()
                    .into_new_event(EventId::new(), EventOrigin::System, None, None);
            let decoded = decode_dtn_event(new_event.schema_version, &new_event.payload).unwrap();
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn correlation_and_causation_round_trip_through_a_new_event() {
        let bundle_id = BundleId::new();
        let created_id = EventId::new();
        let correlation_id = siar_event_log::ids::CorrelationId::new();
        let created = sample_created(bundle_id).into_new_event(
            created_id,
            EventOrigin::System,
            Some(correlation_id),
            None,
        );
        assert_eq!(created.correlation_id, Some(correlation_id));
        assert_eq!(created.causation_id, None);

        let stored = DtnEvent::BundleStored { bundle_id }.into_new_event(
            EventId::new(),
            EventOrigin::System,
            Some(correlation_id),
            Some(created_id),
        );
        assert_eq!(stored.correlation_id, Some(correlation_id));
        assert_eq!(stored.causation_id, Some(created_id));
    }

    /// Not otherwise exercised above — confirms `RouteToken` (used
    /// inside `DtnDestination`'s opaque variants) round-trips through
    /// this module's own postcard encoding too, not just the
    /// `LocalBroadcast` variant every other test happens to use.
    #[test]
    fn an_opaque_destination_variant_round_trips_too() {
        let bundle_id = BundleId::new();
        let event = DtnEvent::BundleCreated {
            bundle_id,
            destination: DtnDestination::DeviceOpaque(RouteToken(vec![1, 2, 3, 4])),
            priority: DtnPriority::Sos,
            payload_type: PayloadTypeId(7),
        };
        let new_event =
            event
                .clone()
                .into_new_event(EventId::new(), EventOrigin::System, None, None);
        let decoded = decode_dtn_event(new_event.schema_version, &new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }
}
