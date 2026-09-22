//! `04-offline-event-log-architecture.md` §37 "Emergency Events",
//! §80 "Emergency Integration", §92 Phase 3's fifth and last domain
//! (after identity, messaging, files, DTN — see those crates' own
//! `events.rs`/`audit_log.rs` for the other four; same shape here).
//!
//! `EventTypeId` constants this crate owns, a typed payload enum
//! (postcard-serialized, §9 schema-version-checked on decode from the
//! start), an `into_new_event` constructor, a `decode` counterpart,
//! and the same "construct only, never append" discipline every
//! sibling domain module already states — this crate has no
//! `siar-storage`/transport dependency (see `lib.rs`'s own scope
//! notes), so there is nowhere in this crate for a real `append`
//! caller to live yet either way.
//!
//! ## Streams
//!
//! One stream per report ([`report_stream_id`]), keyed by the new
//! [`crate::ids::ReportId`] this module needed and
//! [`crate::report::EmergencyReport`] didn't have — see that type's
//! own doc comment for why.
//!
//! ## What's covered, and what §80 explicitly does NOT ask for
//!
//! §80's own text: "SOS is persisted before transmission, even with no
//! network" — the life-safety-critical half of this domain — plus
//! trust reclassification, acknowledgment, and resolution as the
//! report's own lifecycle. Deliberately NOT covered: discovery-mode
//! transitions (`crate::mode::DiscoveryMode`) — that's radio/power
//! management, not an emergency *report's* own history, the same
//! "keep the mechanics out, keep the meaningful facts in" reasoning
//! `siar_dtn_bundle::events`'s own doc comment already gives for
//! excluding per-hop peer telemetry from DTN events.
//!
//! ## No verification logic here, only its OUTCOME
//!
//! [`EmergencyEvent::TrustReclassified`] records that
//! [`crate::trust::AlertTrust`] changed for a report — it does not,
//! and per this crate's own top-level scope notes CANNOT, perform the
//! signature verification that decides what the new value should be
//! (real `siar-crypto` work this crate deliberately has no dependency
//! for). Same "decide vs record" split
//! `siar_blob_manifest::events`'s own doc comment already names for
//! its relationship to `crate::transfer_state`.
//!
//! ## Numeric range
//!
//! 400-405 — the fifth block in the informal per-domain convention
//! (identity 1-8, messaging 100-108, files 200-208, DTN 300-308), same
//! real, unenforced gap every sibling module's own doc comment already
//! names.

use serde::{Deserialize, Serialize};
use siar_domain::AccountId;
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{CorrelationId, EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;
use thiserror::Error;

use crate::ids::ReportId;
use crate::kind::EmergencyMessageKind;
use crate::report::LocationSharing;
use crate::trust::AlertTrust;

/// One stream per report — see this module's own doc comment.
pub fn report_stream_id(report_id: ReportId) -> StreamId {
    StreamId::from_name(&format!("emergency-report:{report_id}"))
}

pub const EVENT_TYPE_REPORT_CREATED: EventTypeId = EventTypeId(400);
pub const EVENT_TYPE_TRUST_RECLASSIFIED: EventTypeId = EventTypeId(401);
pub const EVENT_TYPE_REPORT_ACKNOWLEDGED: EventTypeId = EventTypeId(402);
pub const EVENT_TYPE_REPORT_RESOLVED: EventTypeId = EventTypeId(403);
pub const EVENT_TYPE_REPORT_CANCELLED: EventTypeId = EventTypeId(404);
pub const EVENT_TYPE_REPORT_EXPIRED: EventTypeId = EventTypeId(405);

/// §37's named lifecycle. No per-event timestamp field — the
/// envelope's own `created_at` (set by [`Self::into_new_event`])
/// already carries that, same choice every sibling domain module
/// already makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EmergencyEvent {
    /// §80: "persisted before transmission, even with no network" —
    /// this is that persistence, for whichever caller sends it. Not
    /// `EmergencyReport` itself reused wholesale (§9: never permanently
    /// serialize a current domain struct as a durable schema) — the
    /// fields that matter for durable history, named explicitly.
    ReportCreated {
        report_id: ReportId,
        kind: EmergencyMessageKind,
        sender: AccountId,
        location: LocationSharing,
        people: Option<u16>,
        note: Option<String>,
    },
    /// See this module's own doc comment on why this records an
    /// outcome, not a verification.
    TrustReclassified {
        report_id: ReportId,
        new_trust: AlertTrust,
    },
    ReportAcknowledged {
        report_id: ReportId,
        acknowledging_account: AccountId,
    },
    /// The situation this report described is over — a `Safe`
    /// follow-up, a rescue completed, or any other real resolution.
    ReportResolved { report_id: ReportId },
    /// A false alarm or an explicit retraction — kept distinct from
    /// [`Self::ReportResolved`] (§34/§35's own sibling modules draw
    /// the same "cancelled vs completed are different facts, not the
    /// same outcome with different framing" distinction for transfers
    /// and identity operations).
    ReportCancelled { report_id: ReportId },
    /// A time-boxed report (e.g. `LocationSharing::Live`'s own
    /// `expires_at`) reached its own end without being resolved or
    /// cancelled by anyone.
    ReportExpired { report_id: ReportId },
}

impl EmergencyEvent {
    pub fn event_type(&self) -> EventTypeId {
        match self {
            Self::ReportCreated { .. } => EVENT_TYPE_REPORT_CREATED,
            Self::TrustReclassified { .. } => EVENT_TYPE_TRUST_RECLASSIFIED,
            Self::ReportAcknowledged { .. } => EVENT_TYPE_REPORT_ACKNOWLEDGED,
            Self::ReportResolved { .. } => EVENT_TYPE_REPORT_RESOLVED,
            Self::ReportCancelled { .. } => EVENT_TYPE_REPORT_CANCELLED,
            Self::ReportExpired { .. } => EVENT_TYPE_REPORT_EXPIRED,
        }
    }

    pub fn report_id(&self) -> ReportId {
        match self {
            Self::ReportCreated { report_id, .. }
            | Self::TrustReclassified { report_id, .. }
            | Self::ReportAcknowledged { report_id, .. }
            | Self::ReportResolved { report_id, .. }
            | Self::ReportCancelled { report_id, .. }
            | Self::ReportExpired { report_id, .. } => *report_id,
        }
    }

    /// Thin wrapper over [`report_stream_id`] — same reason every
    /// sibling domain module's own `stream_id` method exists.
    pub fn stream_id(&self) -> StreamId {
        report_stream_id(self.report_id())
    }

    /// Caller-supplied `event_id`/`origin`/`correlation_id`/
    /// `causation_id` — same full §8 shape
    /// `siar_dtn_bundle::events::DtnEvent::into_new_event` and
    /// `siar_messaging::events::MessagingEvent::into_new_event` already
    /// use, for the same reason: no real caller exists yet either way,
    /// so building against the more complete precedent costs nothing
    /// and saves a second signature change later.
    pub fn into_new_event(
        self,
        event_id: EventId,
        origin: EventOrigin,
        correlation_id: Option<CorrelationId>,
        causation_id: Option<EventId>,
    ) -> NewEvent {
        let event_type = self.event_type();
        let payload =
            postcard::to_allocvec(&self).expect("EmergencyEvent always postcard-serializes");
        NewEvent {
            event_id,
            event_type,
            schema_version: CURRENT_EMERGENCY_EVENT_SCHEMA_VERSION,
            created_at: Timestamp::now(),
            origin,
            correlation_id,
            causation_id,
            payload,
        }
    }
}

/// §9: bump this, and add a real `V2` decode branch to
/// [`decode_emergency_event`] below, the day any of this module's
/// variants' fields actually change shape.
pub const CURRENT_EMERGENCY_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum EmergencyEventDecodeError {
    #[error(
        "unsupported emergency event schema version {0} (highest known: {CURRENT_EMERGENCY_EVENT_SCHEMA_VERSION})"
    )]
    UnsupportedVersion(u16),
    #[error("payload did not decode as a valid emergency event: {0}")]
    Malformed(#[from] postcard::Error),
}

/// Read-side counterpart to [`EmergencyEvent::into_new_event`].
/// `schema_version` should come from the same
/// [`siar_event_log::envelope::EventEnvelope::schema_version`] the
/// payload itself was read alongside.
pub fn decode_emergency_event(
    schema_version: u16,
    payload: &[u8],
) -> Result<EmergencyEvent, EmergencyEventDecodeError> {
    match schema_version {
        1 => Ok(postcard::from_bytes(payload)?),
        other => Err(EmergencyEventDecodeError::UnsupportedVersion(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::GeoPoint;

    fn sample_created(report_id: ReportId) -> EmergencyEvent {
        EmergencyEvent::ReportCreated {
            report_id,
            kind: EmergencyMessageKind::Sos,
            sender: AccountId::new(),
            location: LocationSharing::Approximate(GeoPoint {
                latitude: 12.34,
                longitude: 56.78,
            }),
            people: Some(2),
            note: Some("trapped, third floor".to_string()),
        }
    }

    #[test]
    fn same_report_always_derives_the_same_stream_id() {
        let report_id = ReportId::new();
        assert_eq!(report_stream_id(report_id), report_stream_id(report_id));
    }

    #[test]
    fn different_reports_derive_different_stream_ids() {
        assert_ne!(
            report_stream_id(ReportId::new()),
            report_stream_id(ReportId::new())
        );
    }

    #[test]
    fn report_created_round_trips_through_postcard_and_carries_its_own_stream() {
        let report_id = ReportId::new();
        let event = sample_created(report_id);
        assert_eq!(event.stream_id(), report_stream_id(report_id));

        let new_event =
            event
                .clone()
                .into_new_event(EventId::new(), EventOrigin::System, None, None);
        assert_eq!(new_event.event_type, EVENT_TYPE_REPORT_CREATED);

        let decoded = decode_emergency_event(new_event.schema_version, &new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn decoding_rejects_an_unsupported_schema_version() {
        let event = EmergencyEvent::ReportResolved {
            report_id: ReportId::new(),
        };
        let new_event = event.into_new_event(EventId::new(), EventOrigin::Imported, None, None);
        let result = decode_emergency_event(99, &new_event.payload);
        assert!(matches!(
            result,
            Err(EmergencyEventDecodeError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn each_event_type_gets_a_distinct_tag_and_does_not_collide_with_other_domains() {
        let tags = [
            EVENT_TYPE_REPORT_CREATED,
            EVENT_TYPE_TRUST_RECLASSIFIED,
            EVENT_TYPE_REPORT_ACKNOWLEDGED,
            EVENT_TYPE_REPORT_RESOLVED,
            EVENT_TYPE_REPORT_CANCELLED,
            EVENT_TYPE_REPORT_EXPIRED,
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
            // identity: 1-8, messaging: 100-108, files: 200-208,
            // DTN: 300-308 — see this module's own doc comment.
            assert!(a.0 >= 400);
        }
    }

    #[test]
    fn every_variant_round_trips_and_reports_its_own_report_id() {
        let report_id = ReportId::new();
        let events = vec![
            EmergencyEvent::TrustReclassified {
                report_id,
                new_trust: AlertTrust::KnownContact,
            },
            EmergencyEvent::ReportAcknowledged {
                report_id,
                acknowledging_account: AccountId::new(),
            },
            EmergencyEvent::ReportResolved { report_id },
            EmergencyEvent::ReportCancelled { report_id },
            EmergencyEvent::ReportExpired { report_id },
        ];
        for event in events {
            assert_eq!(event.report_id(), report_id);
            let new_event =
                event
                    .clone()
                    .into_new_event(EventId::new(), EventOrigin::System, None, None);
            let decoded =
                decode_emergency_event(new_event.schema_version, &new_event.payload).unwrap();
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn correlation_and_causation_round_trip_through_a_new_event() {
        let report_id = ReportId::new();
        let created_id = EventId::new();
        let correlation_id = CorrelationId::new();
        let created = sample_created(report_id).into_new_event(
            created_id,
            EventOrigin::System,
            Some(correlation_id),
            None,
        );
        assert_eq!(created.correlation_id, Some(correlation_id));
        assert_eq!(created.causation_id, None);

        let acknowledged = EmergencyEvent::ReportAcknowledged {
            report_id,
            acknowledging_account: AccountId::new(),
        }
        .into_new_event(
            EventId::new(),
            EventOrigin::System,
            Some(correlation_id),
            Some(created_id),
        );
        assert_eq!(acknowledged.correlation_id, Some(correlation_id));
        assert_eq!(acknowledged.causation_id, Some(created_id));
    }

    /// A `Live` location (the one variant carrying an `expires_at`
    /// tick, not just a bare `GeoPoint`) round-trips too, not just
    /// `Approximate`.
    #[test]
    fn a_live_location_variant_round_trips_too() {
        let report_id = ReportId::new();
        let event = EmergencyEvent::ReportCreated {
            report_id,
            kind: EmergencyMessageKind::LocationBeacon,
            sender: AccountId::new(),
            location: LocationSharing::Live {
                point: GeoPoint {
                    latitude: -12.0,
                    longitude: 100.5,
                },
                expires_at: 12_345,
            },
            people: None,
            note: None,
        };
        let new_event =
            event
                .clone()
                .into_new_event(EventId::new(), EventOrigin::System, None, None);
        let decoded = decode_emergency_event(new_event.schema_version, &new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }
}
