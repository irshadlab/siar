//! `04-offline-event-log-architecture.md` §40 "Compaction" — "retention
//! is domain-specific" — §92 Phase 7.
//!
//! [`RetentionClass`] is the spec's own five-variant enum, verbatim.
//! The crate supplies the *mechanism*; the owning domain supplies the
//! *policy* by registering each of its event types in a
//! [`RetentionTable`] (same pattern as [`crate::replication::ScopeTable`]).
//!
//! | class | compaction |
//! |---|---|
//! | `Permanent` | never |
//! | `SecurityAudit` | never (the spec's "device revocation → long retention") |
//! | `UserControlled` | never automatically — only the user's own deletion ([`crate::deletion`]) |
//! | `OperationalShortTerm` | payload emptied after `operational_max_age_ms` ("completed transfer operational noise → compactable") |
//! | `Ephemeral` | payload emptied after `ephemeral_max_age_ms`; the spec's "typing status → never journal" is the real fix, this is the backstop for one that got journaled anyway |
//!
//! ## Fail safe: unregistered means keep
//!
//! An event type nobody classified is `Permanent`. Where
//! [`crate::replication`] fails *closed* (don't send), retention fails
//! *toward keeping*: wrongly retaining costs disk, wrongly compacting
//! destroys history that cannot be recovered.
//!
//! ## "Compaction" here means payload redaction
//!
//! See [`crate::redaction`] for why the envelope stays: versions,
//! offsets, idempotency ids and the shape of history are what other
//! guarantees rest on. Compaction reclaims the payload bytes.
//!
//! ## Protecting what pending work still needs
//!
//! [`plan_compaction`] takes a `protect` predicate. Anything it returns
//! `true` for is never a candidate. The case that matters: an event
//! whose outbox operation is still open — [`crate::outbox_reconcile`]
//! (§48) rebuilds work from payloads, so emptying one early would make
//! a lost outbox unrecoverable. [`open_outbox_sources`] builds the right
//! predicate input.
//!
//! The planner is a pure read. Planning and applying are separate so a
//! caller can review (§81 diagnostics) or throttle; [`apply_compaction`]
//! is a thin wrapper over the redaction primitive.

use std::collections::{HashMap, HashSet};

use crate::command_flow::EventRef;
use crate::ids::{EventId, EventTypeId, LocalLogOffset, Timestamp};
use crate::outbox::{Outbox, OutboxError};
use crate::redaction::{RedactableStore, RedactionReport};
use crate::store::EventStoreError;

/// §40, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetentionClass {
    Permanent,
    SecurityAudit,
    UserControlled,
    OperationalShortTerm,
    Ephemeral,
}

#[derive(Debug, Clone, Default)]
pub struct RetentionTable {
    classes: HashMap<EventTypeId, RetentionClass>,
}

impl RetentionTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, event_type: EventTypeId, class: RetentionClass) -> Self {
        self.classes.insert(event_type, class);
        self
    }

    pub fn set(&mut self, event_type: EventTypeId, class: RetentionClass) {
        self.classes.insert(event_type, class);
    }

    /// Unregistered types are `Permanent` — see the module doc.
    pub fn class_of(&self, event_type: EventTypeId) -> RetentionClass {
        self.classes
            .get(&event_type)
            .copied()
            .unwrap_or(RetentionClass::Permanent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub operational_max_age_ms: u64,
    pub ephemeral_max_age_ms: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            operational_max_age_ms: 7 * 24 * 3_600_000,
            ephemeral_max_age_ms: 3_600_000,
        }
    }
}

/// The pure rule: may an event of `class` and `age_ms` be compacted?
pub fn is_compactable(class: RetentionClass, age_ms: u64, policy: &RetentionPolicy) -> bool {
    match class {
        RetentionClass::Permanent
        | RetentionClass::SecurityAudit
        | RetentionClass::UserControlled => false,
        RetentionClass::OperationalShortTerm => age_ms >= policy.operational_max_age_ms,
        RetentionClass::Ephemeral => age_ms >= policy.ephemeral_max_age_ms,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPlan {
    pub candidates: Vec<EventId>,
    pub scanned: u64,
    /// Pass this as `from_offset` to continue; equal to the input when
    /// the log had nothing further.
    pub next_offset: LocalLogOffset,
    pub protected: u32,
}

/// Scans up to `batch` events after `from_offset`. `age` is measured
/// from the event's own `created_at` (display time, §27 — fine for a
/// retention horizon, which is approximate by nature).
pub async fn plan_compaction<S, P>(
    store: &S,
    table: &RetentionTable,
    policy: &RetentionPolicy,
    now: Timestamp,
    from_offset: LocalLogOffset,
    batch: usize,
    protect: P,
) -> Result<CompactionPlan, EventStoreError>
where
    S: RedactableStore + ?Sized,
    P: Fn(&EventRef<'_>) -> bool,
{
    let page = store.read_log(from_offset, batch).await?;
    let mut plan = CompactionPlan {
        candidates: Vec::new(),
        scanned: page.len() as u64,
        next_offset: page.last().map_or(from_offset, |e| e.local_offset),
        protected: 0,
    };
    for stored in &page {
        let class = table.class_of(stored.envelope.event_type);
        let age = now.0.saturating_sub(stored.envelope.created_at.0);
        if !is_compactable(class, age, policy) {
            continue;
        }
        if protect(&EventRef::from_stored(stored)) {
            plan.protected += 1;
            continue;
        }
        if store.is_redacted(stored.envelope.event_id).await? {
            continue;
        }
        plan.candidates.push(stored.envelope.event_id);
    }
    Ok(plan)
}

pub async fn apply_compaction<S: RedactableStore + ?Sized>(
    store: &S,
    plan: &CompactionPlan,
) -> Result<RedactionReport, EventStoreError> {
    store.redact_payloads(&plan.candidates).await
}

/// Source event ids of every outbox operation still owed work: feed
/// this into the `protect` predicate so compaction never empties a
/// payload [`crate::outbox_reconcile`] might need to rebuild.
pub async fn open_outbox_sources<O: Outbox + ?Sized>(
    outbox: &O,
    limit: usize,
) -> Result<HashSet<EventId>, OutboxError> {
    Ok(outbox
        .list_open(limit)
        .await?
        .into_iter()
        .map(|e| e.source_event_id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::StreamId;
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::{EffectKind, OutboxEntry};
    use crate::store::{AppendRequest, EventStore, NewEvent};
    use siar_domain::DeviceId;

    const AUDIT: EventTypeId = EventTypeId(1);
    const NOISE: EventTypeId = EventTypeId(2);
    const TYPING: EventTypeId = EventTypeId(3);
    const MYSTERY: EventTypeId = EventTypeId(4);
    const DAY: u64 = 24 * 3_600_000;

    fn table() -> RetentionTable {
        RetentionTable::new()
            .with(AUDIT, RetentionClass::SecurityAudit)
            .with(NOISE, RetentionClass::OperationalShortTerm)
            .with(TYPING, RetentionClass::Ephemeral)
    }

    fn ev(ty: EventTypeId, created: u64) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(created),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: b"secret".to_vec(),
        }
    }

    #[test]
    fn the_rule_table() {
        let p = RetentionPolicy::default();
        for c in [
            RetentionClass::Permanent,
            RetentionClass::SecurityAudit,
            RetentionClass::UserControlled,
        ] {
            assert!(!is_compactable(c, u64::MAX, &p), "{c:?} never compacts");
        }
        let op = RetentionClass::OperationalShortTerm;
        assert!(!is_compactable(op, 7 * DAY - 1, &p));
        assert!(is_compactable(op, 7 * DAY, &p));
        assert!(!is_compactable(RetentionClass::Ephemeral, 3_599_999, &p));
        assert!(is_compactable(RetentionClass::Ephemeral, 3_600_000, &p));
    }

    #[test]
    fn unregistered_types_are_kept() {
        assert_eq!(table().class_of(MYSTERY), RetentionClass::Permanent);
    }

    #[tokio::test]
    async fn only_aged_operational_and_ephemeral_events_are_planned() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let old = ev(NOISE, 0);
        let fresh = ev(NOISE, 30 * DAY);
        let typing = ev(TYPING, 0);
        let audit = ev(AUDIT, 0);
        let mystery = ev(MYSTERY, 0);
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![
                    old.clone(),
                    fresh,
                    typing.clone(),
                    audit.clone(),
                    mystery.clone(),
                ],
            })
            .await
            .unwrap();

        let now = Timestamp(30 * DAY);
        let plan = plan_compaction(
            &store,
            &table(),
            &RetentionPolicy::default(),
            now,
            LocalLogOffset(0),
            100,
            |_| false,
        )
        .await
        .unwrap();
        assert_eq!(plan.scanned, 5);
        assert_eq!(plan.next_offset, LocalLogOffset(5));
        let mut want = vec![old.event_id, typing.event_id];
        want.sort_by_key(|i| i.as_uuid());
        let mut got = plan.candidates.clone();
        got.sort_by_key(|i| i.as_uuid());
        assert_eq!(got, want);

        let report = apply_compaction(&store, &plan).await.unwrap();
        assert_eq!(report.redacted, 2);

        // Tombstones: payload gone, slot, id and shape intact.
        let all = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(all.len(), 5, "no event removed");
        assert!(all[0].envelope.payload.is_empty());
        assert_eq!(all[0].envelope.event_id, old.event_id);
        assert_eq!(all[0].envelope.stream_version, 1);
        assert_eq!(all[3].envelope.payload, b"secret", "audit event untouched");
        assert_eq!(all[4].envelope.payload, b"secret", "unclassified kept");
        assert!(store.is_redacted(old.event_id).await.unwrap());
        assert!(!store.is_redacted(audit.event_id).await.unwrap());

        // Idempotent: a second plan finds nothing new.
        let again = plan_compaction(
            &store,
            &table(),
            &RetentionPolicy::default(),
            now,
            LocalLogOffset(0),
            100,
            |_| false,
        )
        .await
        .unwrap();
        assert!(again.candidates.is_empty());
        // A tombstoned id stays "seen": a late duplicate cannot
        // resurrect the removed payload (§24).
        let r = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 5,
                events: vec![old],
            })
            .await
            .unwrap();
        assert_eq!(r.local_offsets, vec![None]);
    }

    #[tokio::test]
    async fn events_with_open_outbox_work_are_protected() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let pending = ev(NOISE, 0);
        let done = ev(NOISE, 0);
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![pending.clone(), done.clone()],
            })
            .await
            .unwrap();
        let outbox = store.outbox();
        outbox
            .enqueue(
                OutboxEntry::new(
                    pending.event_id,
                    stream,
                    EffectKind(1),
                    0,
                    vec![],
                    Timestamp(0),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let open = open_outbox_sources(&outbox, 1000).await.unwrap();
        let plan = plan_compaction(
            &store,
            &table(),
            &RetentionPolicy::default(),
            Timestamp(30 * DAY),
            LocalLogOffset(0),
            100,
            |e| open.contains(&e.event_id),
        )
        .await
        .unwrap();
        assert_eq!(plan.protected, 1);
        assert_eq!(plan.candidates, vec![done.event_id]);
    }

    #[tokio::test]
    async fn planning_pages_through_the_log() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: (0..5).map(|_| ev(NOISE, 0)).collect(),
            })
            .await
            .unwrap();
        let mut from = LocalLogOffset(0);
        let mut total = 0;
        loop {
            let plan = plan_compaction(
                &store,
                &table(),
                &RetentionPolicy::default(),
                Timestamp(30 * DAY),
                from,
                2,
                |_| false,
            )
            .await
            .unwrap();
            if plan.scanned == 0 {
                break;
            }
            total += plan.candidates.len();
            from = plan.next_offset;
        }
        assert_eq!(total, 5);
    }
}
