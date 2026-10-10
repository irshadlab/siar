//! `04-offline-event-log-architecture.md` §55 "Event Size Limits" —
//! the per-type half the original uniform ceiling left open.
//!
//! §55: "every event type must have a maximum size," and nothing huge
//! may ride in a journal event: no "huge file", no "huge recursive
//! object", no "unbounded metadata". Until now the stores enforced one
//! flat [`DEFAULT_MAX_EVENT_PAYLOAD_BYTES`] for everything — correct as
//! a backstop, but it can't say "a typing-indicator event must be a few
//! dozen bytes" or "a message edit may be larger than a read receipt".
//! [`EventLimits`] adds that: a default plus per-type overrides, both
//! bounded by an absolute [`HARD_MAX_EVENT_PAYLOAD_BYTES`] that no
//! configuration can exceed — a misconfigured override cannot quietly
//! reopen the door to a file-sized journal entry.
//!
//! Both real stores take their limits through `with_limits` and apply
//! them exactly where the flat ceiling was applied before: **before any
//! write**, rejecting the whole batch (§11 atomicity) — including the
//! batch half of
//! [`crate::outbox::TransactionalOutbox::append_with_outbox`].
//!
//! ## Tightening is always safe; loosening is a decision
//!
//! A per-type limit *below* the default is a pure restriction. One
//! *above* it (a type that legitimately carries more) is allowed up to
//! the hard ceiling, but the domain that asks for it should first ask
//! whether the bytes belong in a blob (§43) with the event carrying only
//! a `BlobId` — which is what the ceiling exists to push people toward.

use std::collections::HashMap;

use thiserror::Error;

use crate::ids::EventTypeId;
use crate::store::{EventStoreError, DEFAULT_MAX_EVENT_PAYLOAD_BYTES};

/// No configuration may allow a single journal event larger than this.
pub const HARD_MAX_EVENT_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LimitConfigError {
    #[error("limit {requested} exceeds the hard ceiling of {HARD_MAX_EVENT_PAYLOAD_BYTES} bytes")]
    AboveHardCeiling { requested: usize },
    #[error("a limit of 0 would make the event type unwritable")]
    Zero,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLimits {
    default_max: usize,
    per_type: HashMap<EventTypeId, usize>,
}

impl Default for EventLimits {
    fn default() -> Self {
        Self {
            default_max: DEFAULT_MAX_EVENT_PAYLOAD_BYTES,
            per_type: HashMap::new(),
        }
    }
}

fn check(limit: usize) -> Result<usize, LimitConfigError> {
    match limit {
        0 => Err(LimitConfigError::Zero),
        n if n > HARD_MAX_EVENT_PAYLOAD_BYTES => {
            Err(LimitConfigError::AboveHardCeiling { requested: n })
        }
        n => Ok(n),
    }
}

impl EventLimits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default(mut self, max: usize) -> Result<Self, LimitConfigError> {
        self.default_max = check(max)?;
        Ok(self)
    }

    pub fn with_type(
        mut self,
        event_type: EventTypeId,
        max: usize,
    ) -> Result<Self, LimitConfigError> {
        self.per_type.insert(event_type, check(max)?);
        Ok(self)
    }

    pub fn limit_for(&self, event_type: EventTypeId) -> usize {
        self.per_type
            .get(&event_type)
            .copied()
            .unwrap_or(self.default_max)
    }

    pub fn validate(
        &self,
        event_type: EventTypeId,
        payload_len: usize,
    ) -> Result<(), EventStoreError> {
        let limit = self.limit_for(event_type);
        if payload_len > limit {
            return Err(EventStoreError::PayloadTooLarge {
                event_type,
                size: payload_len,
                limit,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, StreamId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::TransactionalOutbox;
    use crate::stoolap_store::StoolapEventStore;
    use crate::store::{AppendRequest, EventStore, NewEvent};

    const TYPING: EventTypeId = EventTypeId(1);
    const EDIT: EventTypeId = EventTypeId(2);
    const OTHER: EventTypeId = EventTypeId(3);

    fn limits() -> EventLimits {
        EventLimits::new()
            .with_type(TYPING, 32)
            .unwrap()
            .with_type(EDIT, 1_000_000)
            .unwrap()
    }

    fn ev(ty: EventTypeId, n: usize) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(0),
            origin: EventOrigin::System,
            correlation_id: None,
            causation_id: None,
            payload: vec![0; n],
        }
    }

    fn req(events: Vec<NewEvent>) -> AppendRequest {
        AppendRequest {
            stream_id: StreamId::from_name("s"),
            expected_version: 0,
            events,
        }
    }

    #[test]
    fn per_type_limits_override_the_default() {
        let l = limits();
        assert_eq!(l.limit_for(TYPING), 32);
        assert_eq!(l.limit_for(EDIT), 1_000_000);
        assert_eq!(l.limit_for(OTHER), DEFAULT_MAX_EVENT_PAYLOAD_BYTES);
        assert!(l.validate(TYPING, 32).is_ok());
        assert_eq!(
            l.validate(TYPING, 33).unwrap_err(),
            EventStoreError::PayloadTooLarge {
                event_type: TYPING,
                size: 33,
                limit: 32
            }
        );
        assert!(l.validate(EDIT, 999_999).is_ok());
    }

    #[test]
    fn no_configuration_can_exceed_the_hard_ceiling_or_be_zero() {
        assert_eq!(
            EventLimits::new()
                .with_default(HARD_MAX_EVENT_PAYLOAD_BYTES + 1)
                .unwrap_err(),
            LimitConfigError::AboveHardCeiling {
                requested: HARD_MAX_EVENT_PAYLOAD_BYTES + 1
            }
        );
        assert!(EventLimits::new()
            .with_type(EDIT, HARD_MAX_EVENT_PAYLOAD_BYTES)
            .is_ok());
        assert_eq!(
            EventLimits::new().with_type(EDIT, 0).unwrap_err(),
            LimitConfigError::Zero
        );
    }

    #[tokio::test]
    async fn the_in_memory_store_applies_per_type_limits_to_the_whole_batch() {
        let store = InMemoryEventStore::new().with_limits(limits());
        // One oversized TYPING event poisons the batch: nothing lands.
        let err = store
            .append(req(vec![ev(OTHER, 10), ev(TYPING, 100)]))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            EventStoreError::PayloadTooLarge { limit: 32, .. }
        ));
        assert_eq!(
            store.stream_head(StreamId::from_name("s")).await.unwrap(),
            0
        );

        store
            .append(req(vec![ev(TYPING, 32), ev(EDIT, 500_000)]))
            .await
            .unwrap();
        // The relaxed type admits what the old flat ceiling refused.
        const _: () = assert!(500_000 > DEFAULT_MAX_EVENT_PAYLOAD_BYTES);
    }

    #[tokio::test]
    async fn the_stoolap_store_applies_them_too_including_the_outbox_path() {
        let store = StoolapEventStore::open_in_memory()
            .unwrap()
            .with_limits(limits());
        assert!(matches!(
            store.append(req(vec![ev(TYPING, 33)])).await,
            Err(EventStoreError::PayloadTooLarge { .. })
        ));
        assert!(matches!(
            store
                .append_with_outbox(req(vec![ev(TYPING, 33)]), vec![])
                .await,
            Err(EventStoreError::PayloadTooLarge { .. })
        ));
        store.append(req(vec![ev(EDIT, 300_000)])).await.unwrap();
    }
}
