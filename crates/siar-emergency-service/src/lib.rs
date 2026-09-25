#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §37's own named gap,
//! closed — the fourth and last of the domains §95 itself lists
//! (files, identity, DTN, emergency) as not yet having a real
//! `EventStore::append` caller. `siar_emergency::events::EmergencyEvent`
//! (§37's catalog) has existed since it was first built, with the same
//! "construct only, never append" note every sibling domain module
//! already carries.
//!
//! ## One real difference from the other three rounds
//!
//! Files (`TransferState`) and DTN (`BundleState`) already had a
//! decide layer to wire a record step onto. Identity didn't need one
//! at all — its decisions already lived in real, tested functions
//! elsewhere in `siar-identity-multidevice`. Emergency had NEITHER: no
//! decide layer, and no existing decision function anywhere in this
//! workspace that decided a report's lifecycle. Recording could still have
//! gone ahead with nothing gating it — but that would mean nothing
//! stops a caller from recording `ReportResolved` for a report that
//! was already `Cancelled`, which for a life-safety domain is a worse
//! gap to leave open than the other three were. So this round also
//! added `siar_emergency::report_status` (`ReportStatus`/
//! `ReportEvent`/`InvalidReportTransition`) — a real gap, named as one
//! in that module's own doc comment, not folded quietly into "just
//! another service crate."
//!
//! ## What [`EmergencyReportService::apply`] gates, and what it doesn't
//!
//! Only `EmergencyEvent::ReportResolved`/`ReportCancelled`/
//! `ReportExpired` go through [`ReportStatus::transition`] — see
//! `report_status`'s own doc comment for exactly why
//! `TrustReclassified`/`ReportAcknowledged` deliberately don't: both
//! can arrive at any point in a report's life, including after it's
//! already closed, over a delayed mesh path. [`Self::
//! record_trust_reclassified`]/[`Self::record_report_acknowledged`]
//! record those two directly, gated only on the report being known to
//! this service at all.
//!
//! ## What this closes, and what it doesn't
//!
//! Closes §37/§95's "no real caller" gap for emergency reports — the
//! fourth and last of the domains §95 names. Same caveats as the
//! other three rounds: this crate's own `statuses` map is
//! process-memory only (no independent durability of its own), and
//! nothing in this workspace's uploaded `apps/*` calls
//! [`EmergencyReportService`] yet — only this round's own tests do.
//! With this round done, every domain §95 originally called out by
//! name now has a real, tested decide→record path; wiring an actual
//! caller in `apps/*` for any of the four is still fully open.

use siar_domain::AccountId;
use siar_emergency::events::EmergencyEvent;
use siar_emergency::report::LocationSharing;
use siar_emergency::report_status::{InvalidReportTransition, ReportEvent, ReportStatus};
use siar_emergency::trust::AlertTrust;
use siar_emergency::{kind::EmergencyMessageKind, ReportId};
use siar_event_log::{append_with_retry, EventId, EventOrigin, EventStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Same bound every sibling real caller in this workspace already
/// uses — matched, not re-derived.
const EVENT_LOG_APPEND_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmergencyReportError {
    #[error("report {0} already exists")]
    DuplicateReport(ReportId),
    #[error("report {0} is not known to this service")]
    UnknownReport(ReportId),
    #[error(transparent)]
    InvalidTransition(#[from] InvalidReportTransition),
}

/// The real caller — see this module's own doc comment.
///
/// `event_log` optional, `statuses` process-memory-only: same shape
/// and same reasoning as `siar-file-transfer-service::
/// FileTransferService`/`siar-dtn-bundle-service::DtnBundleService` —
/// see either of those crates' own doc comments for the full
/// argument, not re-derived here.
pub struct EmergencyReportService {
    event_log: Option<Arc<dyn EventStore + Send + Sync>>,
    statuses: Mutex<HashMap<ReportId, ReportStatus>>,
}

impl Default for EmergencyReportService {
    fn default() -> Self {
        Self {
            event_log: None,
            statuses: Mutex::new(HashMap::new()),
        }
    }
}

impl EmergencyReportService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_log(mut self, event_log: Arc<dyn EventStore + Send + Sync>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    pub fn current_status(&self, report_id: ReportId) -> Option<ReportStatus> {
        self.statuses
            .lock()
            .expect("status lock")
            .get(&report_id)
            .copied()
    }

    /// §80's "persisted before transmission, even with no network" —
    /// `ReportStatus::Active` plus §37's `EmergencyEvent::ReportCreated`.
    /// The one transition this service doesn't get from
    /// `ReportStatus::transition` itself, since there is no "before"
    /// status to transition from.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_report(
        &self,
        report_id: ReportId,
        kind: EmergencyMessageKind,
        sender: AccountId,
        location: LocationSharing,
        people: Option<u16>,
        note: Option<String>,
        origin: EventOrigin,
    ) -> Result<ReportStatus, EmergencyReportError> {
        {
            let mut statuses = self.statuses.lock().expect("status lock");
            if statuses.contains_key(&report_id) {
                return Err(EmergencyReportError::DuplicateReport(report_id));
            }
            statuses.insert(report_id, ReportStatus::Active);
        }
        self.record(
            EmergencyEvent::ReportCreated {
                report_id,
                kind,
                sender,
                location,
                people,
                note,
            },
            origin,
        )
        .await;
        Ok(ReportStatus::Active)
    }

    /// `ReportStatus`'s own transition table, decided first — recorded
    /// (§37) only if the decision succeeds. Only `Resolve`/`Cancel`/
    /// `Expire` go through here — see this module's own doc comment
    /// for why `TrustReclassified`/`ReportAcknowledged` have their own
    /// methods below instead.
    pub async fn apply(
        &self,
        report_id: ReportId,
        event: ReportEvent,
        origin: EventOrigin,
    ) -> Result<ReportStatus, EmergencyReportError> {
        let next = {
            let mut statuses = self.statuses.lock().expect("status lock");
            let Some(&current) = statuses.get(&report_id) else {
                return Err(EmergencyReportError::UnknownReport(report_id));
            };
            let next = current.transition(event)?;
            statuses.insert(report_id, next);
            next
        };
        let emergency_event = match event {
            ReportEvent::Resolve => EmergencyEvent::ReportResolved { report_id },
            ReportEvent::Cancel => EmergencyEvent::ReportCancelled { report_id },
            ReportEvent::Expire => EmergencyEvent::ReportExpired { report_id },
        };
        self.record(emergency_event, origin).await;
        Ok(next)
    }

    /// See this module's own doc comment: not gated by `ReportStatus`
    /// at all, only by the report being known — a trust reclassification
    /// can legitimately arrive after a report is already closed.
    pub async fn record_trust_reclassified(
        &self,
        report_id: ReportId,
        new_trust: AlertTrust,
        origin: EventOrigin,
    ) -> Result<(), EmergencyReportError> {
        self.require_known(report_id)?;
        self.record(
            EmergencyEvent::TrustReclassified {
                report_id,
                new_trust,
            },
            origin,
        )
        .await;
        Ok(())
    }

    /// Same reasoning as [`Self::record_trust_reclassified`]: an
    /// acknowledgment over a delayed mesh path can arrive after the
    /// report is already `Resolved`/`Cancelled`/`Expired`, and that's
    /// still a real fact worth recording, not an error.
    pub async fn record_report_acknowledged(
        &self,
        report_id: ReportId,
        acknowledging_account: AccountId,
        origin: EventOrigin,
    ) -> Result<(), EmergencyReportError> {
        self.require_known(report_id)?;
        self.record(
            EmergencyEvent::ReportAcknowledged {
                report_id,
                acknowledging_account,
            },
            origin,
        )
        .await;
        Ok(())
    }

    fn require_known(&self, report_id: ReportId) -> Result<(), EmergencyReportError> {
        if self.statuses.lock().expect("status lock").contains_key(&report_id) {
            Ok(())
        } else {
            Err(EmergencyReportError::UnknownReport(report_id))
        }
    }

    /// Same no-op-without-a-log, log-not-propagate-on-failure pattern
    /// every sibling real caller in this workspace already uses.
    async fn record(&self, event: EmergencyEvent, origin: EventOrigin) {
        let Some(store) = self.event_log.as_deref() else {
            return;
        };
        let stream_id = event.stream_id();
        let new_event = event.into_new_event(EventId::new(), origin, None, None);
        if let Err(e) =
            append_with_retry(store, stream_id, new_event, EVENT_LOG_APPEND_MAX_ATTEMPTS).await
        {
            tracing::warn!(error = %e, "failed to record emergency report event to the event log");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_emergency::events::decode_emergency_event;
    use siar_emergency::report_stream_id;
    use siar_emergency::report::GeoPoint;
    use siar_event_log::InMemoryEventStore;

    fn sample_sos() -> (EmergencyMessageKind, LocationSharing, Option<u16>, Option<String>) {
        (
            EmergencyMessageKind::Sos,
            LocationSharing::Approximate(GeoPoint {
                latitude: 12.0,
                longitude: 34.0,
            }),
            Some(1),
            Some("trapped, need help".to_string()),
        )
    }

    #[tokio::test]
    async fn a_report_created_then_resolved_records_both_events() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = EmergencyReportService::new().with_event_log(store.clone());
        let report_id = ReportId::new();
        let sender = AccountId::new();
        let (kind, location, people, note) = sample_sos();

        let status = service
            .create_report(report_id, kind, sender, location, people, note, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(status, ReportStatus::Active);

        let status = service
            .apply(report_id, ReportEvent::Resolve, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(status, ReportStatus::ReportResolved);
        assert!(status.is_terminal());

        let stream = store
            .read_stream(report_stream_id(report_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream.len(), 2);
        assert!(matches!(
            decode_emergency_event(stream[0].envelope.schema_version, &stream[0].envelope.payload)
                .unwrap(),
            EmergencyEvent::ReportCreated { .. }
        ));
        assert!(matches!(
            decode_emergency_event(stream[1].envelope.schema_version, &stream[1].envelope.payload)
                .unwrap(),
            EmergencyEvent::ReportResolved { .. }
        ));
    }

    #[tokio::test]
    async fn a_terminal_report_rejects_a_second_lifecycle_event() {
        let service = EmergencyReportService::new();
        let report_id = ReportId::new();
        let sender = AccountId::new();
        let (kind, location, people, note) = sample_sos();
        service
            .create_report(report_id, kind, sender, location, people, note, EventOrigin::System)
            .await
            .unwrap();
        service
            .apply(report_id, ReportEvent::Cancel, EventOrigin::System)
            .await
            .unwrap();

        let result = service
            .apply(report_id, ReportEvent::Resolve, EventOrigin::System)
            .await;
        assert!(matches!(
            result,
            Err(EmergencyReportError::InvalidTransition(_))
        ));
        assert_eq!(
            service.current_status(report_id),
            Some(ReportStatus::ReportCancelled)
        );
    }

    #[tokio::test]
    async fn an_acknowledgment_can_arrive_after_the_report_is_already_resolved() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = EmergencyReportService::new().with_event_log(store.clone());
        let report_id = ReportId::new();
        let sender = AccountId::new();
        let (kind, location, people, note) = sample_sos();
        service
            .create_report(report_id, kind, sender, location, people, note, EventOrigin::System)
            .await
            .unwrap();
        service
            .apply(report_id, ReportEvent::Resolve, EventOrigin::System)
            .await
            .unwrap();

        // The report is already terminal — this still succeeds, per
        // this module's own doc comment.
        let acker = AccountId::new();
        service
            .record_report_acknowledged(report_id, acker, EventOrigin::System)
            .await
            .unwrap();
        service
            .record_trust_reclassified(report_id, AlertTrust::KnownContact, EventOrigin::System)
            .await
            .unwrap();

        let stream = store
            .read_stream(report_stream_id(report_id), 0, 100)
            .await
            .unwrap();
        // Created, Resolved, Acknowledged, TrustReclassified.
        assert_eq!(stream.len(), 4);
    }

    #[tokio::test]
    async fn an_unknown_report_is_rejected_for_every_entry_point() {
        let service = EmergencyReportService::new();
        let report_id = ReportId::new();
        assert!(matches!(
            service
                .apply(report_id, ReportEvent::Resolve, EventOrigin::System)
                .await,
            Err(EmergencyReportError::UnknownReport(_))
        ));
        assert!(matches!(
            service
                .record_report_acknowledged(report_id, AccountId::new(), EventOrigin::System)
                .await,
            Err(EmergencyReportError::UnknownReport(_))
        ));
        assert!(matches!(
            service
                .record_trust_reclassified(report_id, AlertTrust::Unverified, EventOrigin::System)
                .await,
            Err(EmergencyReportError::UnknownReport(_))
        ));
    }

    #[tokio::test]
    async fn creating_the_same_report_twice_is_rejected() {
        let service = EmergencyReportService::new();
        let report_id = ReportId::new();
        let sender = AccountId::new();
        let (kind, location, people, note) = sample_sos();
        service
            .create_report(
                report_id,
                kind,
                sender,
                location.clone(),
                people,
                note.clone(),
                EventOrigin::System,
            )
            .await
            .unwrap();
        let result = service
            .create_report(report_id, kind, sender, location, people, note, EventOrigin::System)
            .await;
        assert_eq!(
            result,
            Err(EmergencyReportError::DuplicateReport(report_id))
        );
    }
}
