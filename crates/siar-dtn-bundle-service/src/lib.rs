#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §36's own named gap, closed:
//! `siar_dtn_bundle::events::DtnEvent` (§36's catalog, 9 events,
//! `EventTypeId` 300-308) and `siar_dtn_bundle::state::BundleState`
//! (§18's state machine) have both existed since they were first
//! built — that module's own top doc comment says outright "construct
//! only, never append," and `ROADMAP.md`'s own §36/§95 rows confirm
//! nothing in this workspace ever did either. This crate is that
//! caller — same shape [`siar-file-transfer-service`] already is for
//! §34: DECIDE (`BundleState::transition`) then, only on success,
//! RECORD (`DtnEvent` → `append_with_retry`).
//!
//! ## Two transitions this service deliberately records nothing for
//!
//! `BundleEvent::BecomeEligible` and `BundleEvent::Reject` both
//! succeed as real `BundleState` transitions but map to no
//! `DtnEvent` — not an oversight, but following `siar_dtn_bundle::
//! events`'s own doc comment exactly:
//!
//! - `Eligible` is "scheduling-internal" — §2 "Do Not Event-Source
//!   Everything" applied to a scheduler's own moment-to-moment
//!   decision, not a durable lifecycle fact.
//! - `Rejected` happens before a bundle is ever durably `Stored`, so
//!   there is "no meaningful 'this bundle existed and then was
//!   rejected' history to record distinct from simply never having
//!   created one" — that module's own words. Concretely: this means a
//!   real caller should only call [`DtnBundleService::create_bundle`]
//!   for bundles that already passed whatever validation would
//!   otherwise reject them; this service has no "create, then maybe
//!   reject" flow, matching that the events module itself says this
//!   is "a decision that caller makes, not this module."
//!
//! Both are still real, checked transitions in [`DtnBundleService::
//! apply`] — `BundleState::transition` still validates and rejects an
//! illegal move for either event exactly like any other; they simply
//! produce `Ok` with nothing recorded, rather than an error.
//!
//! ## What this closes, and what it doesn't
//!
//! Closes §36/§95's "no real caller" gap for DTN bundles, the same
//! half-closed shape as the files/identity rounds: a real, tested
//! decide→record path exists; nothing in this workspace's uploaded
//! `apps/*`, or in `siar-dtn-bundle`/`siar-dtn` themselves, calls
//! [`DtnBundleService`] yet, and [`DtnBundleService`]'s own `states`
//! map is process-memory only (same caveat
//! `siar-file-transfer-service`'s own doc comment already states, for
//! the same reason). §37 (Emergency) has the identical gap under a
//! fifth domain, still open after this.

use siar_dtn_bundle::events::DtnEvent;
use siar_dtn_bundle::state::{BundleEvent, BundleState, InvalidBundleTransition};
use siar_dtn_bundle::types::{BundleId, DtnDestination, DtnPriority, PayloadTypeId};
use siar_event_log::{append_with_retry, EventId, EventOrigin, EventStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Same bound every sibling real caller in this workspace already
/// uses — matched, not re-derived.
const EVENT_LOG_APPEND_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DtnBundleError {
    #[error("bundle {0} already exists")]
    DuplicateBundle(BundleId),
    #[error("bundle {0} is not known to this service")]
    UnknownBundle(BundleId),
    #[error(transparent)]
    InvalidTransition(#[from] InvalidBundleTransition),
}

/// The real caller — see this module's own doc comment.
///
/// `event_log` optional, `states` process-memory-only: same shape and
/// same reasoning as `siar-file-transfer-service::FileTransferService`
/// — see that crate's own doc comment for the full argument, not
/// re-derived here.
pub struct DtnBundleService {
    event_log: Option<Arc<dyn EventStore + Send + Sync>>,
    states: Mutex<HashMap<BundleId, BundleState>>,
}

impl Default for DtnBundleService {
    fn default() -> Self {
        Self {
            event_log: None,
            states: Mutex::new(HashMap::new()),
        }
    }
}

impl DtnBundleService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_log(mut self, event_log: Arc<dyn EventStore + Send + Sync>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    pub fn current_state(&self, bundle_id: BundleId) -> Option<BundleState> {
        self.states
            .lock()
            .expect("state lock")
            .get(&bundle_id)
            .copied()
    }

    /// §18's initial state (`BundleState::Created`) plus §36's
    /// `DtnEvent::BundleCreated` — the one transition this service
    /// doesn't get from `BundleState::transition` itself. See this
    /// module's own doc comment: only call this for a bundle that has
    /// already passed whatever validation would otherwise reject it —
    /// this service has no separate "undo" for a bundle recorded here
    /// and rejected a moment later.
    pub async fn create_bundle(
        &self,
        bundle_id: BundleId,
        destination: DtnDestination,
        priority: DtnPriority,
        payload_type: PayloadTypeId,
        origin: EventOrigin,
    ) -> Result<BundleState, DtnBundleError> {
        {
            let mut states = self.states.lock().expect("state lock");
            if states.contains_key(&bundle_id) {
                return Err(DtnBundleError::DuplicateBundle(bundle_id));
            }
            states.insert(bundle_id, BundleState::Created);
        }
        self.record(
            DtnEvent::BundleCreated {
                bundle_id,
                destination,
                priority,
                payload_type,
            },
            origin,
        )
        .await;
        Ok(BundleState::Created)
    }

    /// §18's own transition table, decided first — recorded (§36) only
    /// for the seven `BundleEvent` variants that have a corresponding
    /// `DtnEvent`. As of §29 "Pure Decision Functions," the decide
    /// step is [`siar_dtn_bundle::decide`], not duplicated here — see
    /// that function's own doc comment. `BecomeEligible`/`Reject`
    /// still transition and still return `Ok`, just record nothing
    /// (an empty `Vec` from `decide`, not a special case this method
    /// has to know about anymore) — see this module's own doc comment
    /// for why that's deliberate rather than a silent bug.
    pub async fn apply(
        &self,
        bundle_id: BundleId,
        event: BundleEvent,
        origin: EventOrigin,
    ) -> Result<BundleState, DtnBundleError> {
        let current = {
            let states = self.states.lock().expect("state lock");
            let Some(&current) = states.get(&bundle_id) else {
                return Err(DtnBundleError::UnknownBundle(bundle_id));
            };
            current
        };
        let (next, events) = siar_dtn_bundle::decide(current, event, bundle_id)?;
        self.states
            .lock()
            .expect("state lock")
            .insert(bundle_id, next);
        for dtn_event in events {
            self.record(dtn_event, origin).await;
        }
        Ok(next)
    }

    /// Same no-op-without-a-log, log-not-propagate-on-failure pattern
    /// every sibling real caller in this workspace already uses — see
    /// `siar-file-transfer-service`'s own `record` for the full
    /// reasoning.
    async fn record(&self, event: DtnEvent, origin: EventOrigin) {
        let Some(store) = self.event_log.as_deref() else {
            return;
        };
        let stream_id = event.stream_id();
        let new_event = event.into_new_event(EventId::new(), origin, None, None);
        if let Err(e) =
            append_with_retry(store, stream_id, new_event, EVENT_LOG_APPEND_MAX_ATTEMPTS).await
        {
            tracing::warn!(error = %e, "failed to record DTN bundle event to the event log");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_dtn_bundle::events::{bundle_stream_id, decode_dtn_event};
    use siar_dtn_bundle::types::{BroadcastScope, RouteToken};
    use siar_event_log::InMemoryEventStore;

    fn sample_destination() -> DtnDestination {
        DtnDestination::LocalBroadcast(BroadcastScope { radius_hops: 3 })
    }

    #[tokio::test]
    async fn a_normal_bundle_walks_creation_to_completion_and_records_every_step() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = DtnBundleService::new().with_event_log(store.clone());
        let bundle_id = BundleId::new();

        let state = service
            .create_bundle(
                bundle_id,
                sample_destination(),
                DtnPriority::Normal,
                PayloadTypeId(7),
                EventOrigin::System,
            )
            .await
            .unwrap();
        assert_eq!(state, BundleState::Created);

        let state = service
            .apply(bundle_id, BundleEvent::PersistDurably, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(state, BundleState::Stored);

        // Scheduling-internal — no event, per this crate's own doc
        // comment, but still a real, tracked state change.
        let state = service
            .apply(bundle_id, BundleEvent::BecomeEligible, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(state, BundleState::Eligible);

        let state = service
            .apply(bundle_id, BundleEvent::Forward, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(state, BundleState::Forwarded);
        // A second hop stays in Forwarded — no distinct state, per
        // `BundleState`'s own doc comment — and records a SECOND
        // `BundleForwarded`, not a no-op.
        service
            .apply(bundle_id, BundleEvent::Forward, EventOrigin::System)
            .await
            .unwrap();

        let state = service
            .apply(
                bundle_id,
                BundleEvent::ReachDestination,
                EventOrigin::System,
            )
            .await
            .unwrap();
        assert!(state.is_delivered());

        service
            .apply(bundle_id, BundleEvent::Acknowledge, EventOrigin::System)
            .await
            .unwrap();
        let state = service
            .apply(bundle_id, BundleEvent::Complete, EventOrigin::System)
            .await
            .unwrap();
        assert!(state.is_terminal());

        // BundleCreated, BundleStored, (nothing for BecomeEligible),
        // BundleForwarded x2, BundleDestinationReached,
        // BundleAcknowledged, BundleCompleted = 7 recorded events.
        let stream = store
            .read_stream(bundle_stream_id(bundle_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream.len(), 7);
        let decoded: Vec<_> = stream
            .iter()
            .map(|e| decode_dtn_event(e.envelope.schema_version, &e.envelope.payload).unwrap())
            .collect();
        assert!(matches!(decoded[0], DtnEvent::BundleCreated { .. }));
        assert!(matches!(decoded[1], DtnEvent::BundleStored { .. }));
        assert!(matches!(decoded[2], DtnEvent::BundleForwarded { .. }));
        assert!(matches!(decoded[3], DtnEvent::BundleForwarded { .. }));
        assert!(matches!(
            decoded[4],
            DtnEvent::BundleDestinationReached { .. }
        ));
        assert!(matches!(decoded[5], DtnEvent::BundleAcknowledged { .. }));
        assert!(matches!(decoded[6], DtnEvent::BundleCompleted { .. }));
    }

    #[tokio::test]
    async fn reject_transitions_cleanly_but_records_nothing() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = DtnBundleService::new().with_event_log(store.clone());
        let bundle_id = BundleId::new();
        service
            .create_bundle(
                bundle_id,
                sample_destination(),
                DtnPriority::Low,
                PayloadTypeId(1),
                EventOrigin::System,
            )
            .await
            .unwrap();

        let state = service
            .apply(bundle_id, BundleEvent::Reject, EventOrigin::System)
            .await
            .unwrap();
        assert_eq!(state, BundleState::Rejected);

        // Only the earlier BundleCreated is in the log — Reject itself
        // recorded nothing, per this crate's own doc comment.
        let stream = store
            .read_stream(bundle_stream_id(bundle_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream.len(), 1);
        assert!(matches!(
            decode_dtn_event(
                stream[0].envelope.schema_version,
                &stream[0].envelope.payload
            )
            .unwrap(),
            DtnEvent::BundleCreated { .. }
        ));
    }

    #[tokio::test]
    async fn an_invalid_transition_is_rejected_and_records_nothing() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = DtnBundleService::new().with_event_log(store.clone());
        let bundle_id = BundleId::new();
        service
            .create_bundle(
                bundle_id,
                sample_destination(),
                DtnPriority::Normal,
                PayloadTypeId(1),
                EventOrigin::System,
            )
            .await
            .unwrap();

        // Created can't Forward without going through Stored/Eligible
        // first (see `BundleState`'s own transition table).
        let result = service
            .apply(bundle_id, BundleEvent::Forward, EventOrigin::System)
            .await;
        assert!(matches!(result, Err(DtnBundleError::InvalidTransition(_))));
        assert_eq!(service.current_state(bundle_id), Some(BundleState::Created));
        let stream = store
            .read_stream(bundle_stream_id(bundle_id), 0, 100)
            .await
            .unwrap();
        assert_eq!(stream.len(), 1);
    }

    #[tokio::test]
    async fn an_unknown_bundle_is_rejected() {
        let service = DtnBundleService::new();
        let result = service
            .apply(BundleId::new(), BundleEvent::Forward, EventOrigin::System)
            .await;
        assert!(matches!(result, Err(DtnBundleError::UnknownBundle(_))));
    }

    #[tokio::test]
    async fn creating_the_same_bundle_twice_is_rejected() {
        let service = DtnBundleService::new();
        let bundle_id = BundleId::new();
        service
            .create_bundle(
                bundle_id,
                sample_destination(),
                DtnPriority::Normal,
                PayloadTypeId(1),
                EventOrigin::System,
            )
            .await
            .unwrap();
        let result = service
            .create_bundle(
                bundle_id,
                sample_destination(),
                DtnPriority::Normal,
                PayloadTypeId(1),
                EventOrigin::System,
            )
            .await;
        assert_eq!(result, Err(DtnBundleError::DuplicateBundle(bundle_id)));
    }

    /// Confirms the sidestep this crate's own `Cargo.toml` comment
    /// promises actually holds — a device-scoped origin still works
    /// fine here via `RouteToken`, with no `siar_domain` dependency.
    #[tokio::test]
    async fn works_with_an_opaque_route_token_destination() {
        let service = DtnBundleService::new();
        let bundle_id = BundleId::new();
        let destination = DtnDestination::DeviceOpaque(RouteToken(vec![1, 2, 3, 4]));
        let state = service
            .create_bundle(
                bundle_id,
                destination,
                DtnPriority::Important,
                PayloadTypeId(42),
                EventOrigin::Imported,
            )
            .await
            .unwrap();
        assert_eq!(state, BundleState::Created);
    }
}
