//! `04-offline-event-log-architecture.md` §62 "Unknown Events".
//!
//! "Unknown event handling must be explicit. Optional unknown →
//! store/ignore safely. Required semantic unknown → block stream until
//! upgrade. Never deserialize into arbitrary current types."
//!
//! Replication (§49-53) and restore (§70) both bring in events written
//! by *other builds*. A node on an older build will meet event types it
//! has never heard of; what it does next must be a decision made in
//! advance, not an accident of whichever `match` arm a decoder falls
//! into. [`UnknownEventPolicy`] makes the decision per event type:
//!
//! | event type | [`Disposition`] |
//! |---|---|
//! | registered as known | `Apply` |
//! | unknown, inside a declared *optional* range | `StoreAndIgnore` — it stays in the log (so it replicates onward and a later build can use it) but this build's projections skip it |
//! | unknown, anywhere else | `Block` — fail safe |
//!
//! "Anywhere else blocks" is the deliberate default. An unknown event
//! might be a revocation, a membership change, or a security fact;
//! ignoring one would let this node keep acting on state the rest of
//! the account has already moved past. Optional ranges are opt-in, so a
//! domain declares "extension events live in 9000-9999 and are safe to
//! skip" once, in review, rather than every decoder guessing.
//!
//! ## What "block" means here
//!
//! [`triage`] stops at the **first** blocking event in a batch: it
//! returns the events before it (safe to apply), the log offset up to
//! which a projection checkpoint may advance
//! ([`Triage::safe_through`]), and a [`BlockedAt`] naming the stream,
//! version and type. Nothing after the blocker is applied — not even
//! from other streams. That is stricter than the spec's "block stream",
//! on purpose: projection checkpoints in this crate are per *log offset*
//! ([`crate::projection`]), so letting later events through would mean
//! the checkpoint either skips the blocker (losing it) or never moves
//! again. Per-stream checkpoints would allow finer blocking; they don't
//! exist yet. Blocking is not an error: the events stay stored,
//! and the projection resumes from the same checkpoint the moment a
//! build that knows the type runs.
//!
//! The decoder side of "never deserialize into arbitrary current types"
//! is each domain's own (`decode_messaging_event` returns an error for
//! an unknown tag rather than coercing); this module is the *policy*
//! deciding what happens when it does.

use std::collections::HashSet;
use std::ops::RangeInclusive;

use crate::ids::{EventTypeId, LocalLogOffset, StreamId};
use crate::store::StoredEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Apply,
    StoreAndIgnore,
    Block,
}

#[derive(Debug, Clone, Default)]
pub struct UnknownEventPolicy {
    known: HashSet<EventTypeId>,
    optional: Vec<RangeInclusive<u32>>,
}

impl UnknownEventPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_known(mut self, types: impl IntoIterator<Item = EventTypeId>) -> Self {
        self.known.extend(types);
        self
    }

    /// Declares `range` of type ids as skippable when unknown.
    pub fn with_optional_range(mut self, range: RangeInclusive<u32>) -> Self {
        self.optional.push(range);
        self
    }

    pub fn disposition(&self, event_type: EventTypeId) -> Disposition {
        if self.known.contains(&event_type) {
            Disposition::Apply
        } else if self.optional.iter().any(|r| r.contains(&event_type.0)) {
            Disposition::StoreAndIgnore
        } else {
            Disposition::Block
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockedAt {
    pub stream: StreamId,
    pub stream_version: u64,
    pub event_type: EventTypeId,
    pub local_offset: LocalLogOffset,
}

#[derive(Debug)]
pub struct Triage<'a> {
    /// In order, ready for `Projection::apply`.
    pub apply: Vec<&'a StoredEvent>,
    /// Unknown-but-optional events skipped (still stored).
    pub ignored: u32,
    /// Highest offset a checkpoint may advance to: everything up to it
    /// was either applied or safely ignored. `None` if nothing at all
    /// could be consumed.
    pub safe_through: Option<LocalLogOffset>,
    pub blocked: Option<BlockedAt>,
}

/// See the module doc.
pub fn triage<'a>(policy: &UnknownEventPolicy, events: &'a [StoredEvent]) -> Triage<'a> {
    let mut out = Triage {
        apply: Vec::new(),
        ignored: 0,
        safe_through: None,
        blocked: None,
    };
    for e in events {
        match policy.disposition(e.envelope.event_type) {
            Disposition::Apply => out.apply.push(e),
            Disposition::StoreAndIgnore => out.ignored += 1,
            Disposition::Block => {
                out.blocked = Some(BlockedAt {
                    stream: e.envelope.stream_id,
                    stream_version: e.envelope.stream_version,
                    event_type: e.envelope.event_type,
                    local_offset: e.local_offset,
                });
                return out;
            }
        }
        out.safe_through = Some(e.local_offset);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::{AppendRequest, EventStore, NewEvent};

    const KNOWN_A: EventTypeId = EventTypeId(100);
    const KNOWN_B: EventTypeId = EventTypeId(101);
    const EXTENSION: EventTypeId = EventTypeId(9_500);
    const FUTURE_CORE: EventTypeId = EventTypeId(102);

    fn policy() -> UnknownEventPolicy {
        UnknownEventPolicy::new()
            .with_known([KNOWN_A, KNOWN_B])
            .with_optional_range(9_000..=9_999)
    }

    async fn log(types: &[EventTypeId]) -> Vec<StoredEvent> {
        let store = InMemoryEventStore::new();
        store
            .append(AppendRequest {
                stream_id: StreamId::from_name("s"),
                expected_version: 0,
                events: types
                    .iter()
                    .map(|t| NewEvent {
                        event_id: EventId::new(),
                        event_type: *t,
                        schema_version: 1,
                        created_at: Timestamp(0),
                        origin: EventOrigin::System,
                        correlation_id: None,
                        causation_id: None,
                        payload: vec![],
                    })
                    .collect(),
            })
            .await
            .unwrap();
        store.read_log(LocalLogOffset(0), 100).await.unwrap()
    }

    #[test]
    fn the_three_dispositions() {
        let p = policy();
        assert_eq!(p.disposition(KNOWN_A), Disposition::Apply);
        assert_eq!(p.disposition(EXTENSION), Disposition::StoreAndIgnore);
        assert_eq!(p.disposition(FUTURE_CORE), Disposition::Block);
        assert_eq!(
            p.disposition(EventTypeId(9_000)),
            Disposition::StoreAndIgnore
        );
        assert_eq!(p.disposition(EventTypeId(10_000)), Disposition::Block);
        // A type that is both known and inside an optional range is known.
        let both = UnknownEventPolicy::new()
            .with_known([EXTENSION])
            .with_optional_range(9_000..=9_999);
        assert_eq!(both.disposition(EXTENSION), Disposition::Apply);
    }

    #[test]
    fn an_empty_policy_blocks_everything() {
        assert_eq!(
            UnknownEventPolicy::new().disposition(KNOWN_A),
            Disposition::Block
        );
    }

    #[tokio::test]
    async fn optional_unknowns_are_skipped_but_the_checkpoint_still_advances() {
        let events = log(&[KNOWN_A, EXTENSION, KNOWN_B, EXTENSION]).await;
        let t = triage(&policy(), &events);
        assert_eq!(t.apply.len(), 2);
        assert_eq!(t.ignored, 2);
        assert!(t.blocked.is_none());
        assert_eq!(t.safe_through, Some(LocalLogOffset(4)));
    }

    #[tokio::test]
    async fn a_required_unknown_stops_everything_after_it() {
        let events = log(&[KNOWN_A, KNOWN_B, FUTURE_CORE, KNOWN_A, EXTENSION]).await;
        let t = triage(&policy(), &events);
        assert_eq!(t.apply.len(), 2, "events after the blocker are NOT applied");
        assert_eq!(
            t.safe_through,
            Some(LocalLogOffset(2)),
            "checkpoint stops short of it"
        );
        let b = t.blocked.unwrap();
        assert_eq!(
            (b.event_type, b.stream_version, b.local_offset),
            (FUTURE_CORE, 3, LocalLogOffset(3))
        );
    }

    #[tokio::test]
    async fn a_blocker_first_leaves_nothing_consumable() {
        let events = log(&[FUTURE_CORE, KNOWN_A]).await;
        let t = triage(&policy(), &events);
        assert!(t.apply.is_empty());
        assert_eq!(t.safe_through, None);
        assert!(t.blocked.is_some());
    }

    #[tokio::test]
    async fn after_an_upgrade_the_same_log_applies_in_full() {
        let events = log(&[KNOWN_A, FUTURE_CORE, KNOWN_B]).await;
        assert!(triage(&policy(), &events).blocked.is_some());
        let upgraded = policy().with_known([FUTURE_CORE]);
        let t = triage(&upgraded, &events);
        assert_eq!(t.apply.len(), 3);
        assert!(t.blocked.is_none());
    }
}
