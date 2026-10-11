//! `04-offline-event-log-architecture.md` §72 "Analytics Separation".
//!
//! "Never upload the raw event journal as analytics. Analytics must use
//! a privacy-filtered projection."
//!
//! The way to make "never" hold is to make the *wrong thing impossible
//! to write*, not merely discouraged. So the export path is split by
//! type:
//!
//! - [`build_analytics`] / [`analytics_from_store`] read events and
//!   produce [`AnalyticsRecord`]s.
//! - [`AnalyticsRecord`] has exactly three fields — an event-type
//!   number, a coarse time bucket, and a count. It has **no field that
//!   can hold an event id, stream id, device id, payload, timestamp
//!   finer than the bucket, or any text.** It cannot be made to carry
//!   content because there is nowhere to put it.
//! - [`encode_for_upload`] — the only thing meant to feed an uploader —
//!   accepts `&[AnalyticsRecord]`, not events.
//!
//! There is no function in this module that serializes a
//! [`StoredEvent`]; an uploader that wants raw events has to go write
//! its own, in review, against this module's stated rule.
//!
//! ## The filters, in order
//!
//! 1. **Allow-list of event types.** Only types the domain explicitly
//!    marks as analytics-safe are counted at all; everything else is
//!    invisible, including types added later (same fail-closed stance as
//!    [`crate::replication::ScopeTable`]).
//! 2. **Aggregation.** Individual events vanish into `(type, bucket) →
//!    count`. Bucket width is policy (a day by default); a finer bucket
//!    leaks more timing.
//! 3. **Small-count suppression.** A bucket with fewer than
//!    `min_count` events is dropped: "one reaction at 03:00 on Tuesday"
//!    is nearly an individual event. (This is the simple k-threshold
//!    form of k-anonymity; it is not differential privacy and does not
//!    claim to be.)
//!
//! What this does **not** do: decide that a given *type* is safe to
//! count. "A message was created" is fine; counting events of a type
//! that exists only for a sensitive feature may itself disclose that the
//! feature is used. That judgement is the allow-list's author's.

use std::collections::{BTreeMap, HashSet};

use crate::ids::{EventTypeId, LocalLogOffset, Timestamp};
use crate::store::{EventStore, EventStoreError, StoredEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsPolicy {
    allowed: HashSet<EventTypeId>,
    pub bucket_ms: u64,
    pub min_count: u32,
}

impl AnalyticsPolicy {
    /// One-day buckets, suppress buckets under 5.
    pub fn new(allowed: impl IntoIterator<Item = EventTypeId>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
            bucket_ms: 24 * 3_600_000,
            min_count: 5,
        }
    }

    pub fn with_bucket_ms(mut self, bucket_ms: u64) -> Self {
        self.bucket_ms = bucket_ms.max(1);
        self
    }

    pub fn with_min_count(mut self, min_count: u32) -> Self {
        self.min_count = min_count;
        self
    }
}

/// The *entire* shape of what may leave the device. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AnalyticsRecord {
    pub event_type: u32,
    pub bucket_start_ms: u64,
    pub count: u32,
}

/// Encoded length of one record.
pub const ANALYTICS_RECORD_LEN: usize = 4 + 8 + 4;

/// Accumulates counts across batches; call [`Self::finish`] once.
#[derive(Debug)]
pub struct AnalyticsAccumulator<'p> {
    policy: &'p AnalyticsPolicy,
    counts: BTreeMap<(u32, u64), u32>,
}

impl<'p> AnalyticsAccumulator<'p> {
    pub fn new(policy: &'p AnalyticsPolicy) -> Self {
        Self {
            policy,
            counts: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, event: &StoredEvent) {
        let e = &event.envelope;
        if !self.policy.allowed.contains(&e.event_type) {
            return;
        }
        let width = self.policy.bucket_ms.max(1);
        let bucket = e.created_at.0 / width * width;
        let slot = self.counts.entry((e.event_type.0, bucket)).or_insert(0);
        *slot = slot.saturating_add(1);
    }

    /// Applies small-count suppression and returns records sorted by
    /// `(type, bucket)`.
    pub fn finish(self) -> Vec<AnalyticsRecord> {
        let min = self.policy.min_count;
        self.counts
            .into_iter()
            .filter(|(_, c)| *c >= min)
            .map(|((event_type, bucket_start_ms), count)| AnalyticsRecord {
                event_type,
                bucket_start_ms,
                count,
            })
            .collect()
    }
}

/// Convenience over an in-memory slice.
pub fn build_analytics(policy: &AnalyticsPolicy, events: &[StoredEvent]) -> Vec<AnalyticsRecord> {
    let mut acc = AnalyticsAccumulator::new(policy);
    for e in events {
        acc.add(e);
    }
    acc.finish()
}

/// Streams the whole log in bounded pages (§59) — memory is
/// proportional to the number of *buckets*, not events.
pub async fn analytics_from_store<S: EventStore + ?Sized>(
    store: &S,
    policy: &AnalyticsPolicy,
    page: usize,
) -> Result<Vec<AnalyticsRecord>, EventStoreError> {
    let mut acc = AnalyticsAccumulator::new(policy);
    let mut from = LocalLogOffset(0);
    loop {
        let batch = store.read_log(from, page.max(1)).await?;
        let Some(last) = batch.last() else { break };
        from = last.local_offset;
        for e in &batch {
            acc.add(e);
        }
    }
    Ok(acc.finish())
}

/// Fixed-width big-endian encoding — the only serialization of analytics
/// this module offers.
pub fn encode_for_upload(records: &[AnalyticsRecord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len() * ANALYTICS_RECORD_LEN);
    for r in records {
        out.extend_from_slice(&r.event_type.to_be_bytes());
        out.extend_from_slice(&r.bucket_start_ms.to_be_bytes());
        out.extend_from_slice(&r.count.to_be_bytes());
    }
    out
}

/// Decodes [`encode_for_upload`] output; `None` if the length is not a
/// whole number of records.
pub fn decode_upload(bytes: &[u8]) -> Option<Vec<AnalyticsRecord>> {
    if !bytes.len().is_multiple_of(ANALYTICS_RECORD_LEN) {
        return None;
    }
    bytes
        .chunks_exact(ANALYTICS_RECORD_LEN)
        .map(|c| {
            Some(AnalyticsRecord {
                event_type: u32::from_be_bytes(c[..4].try_into().ok()?),
                bucket_start_ms: u64::from_be_bytes(c[4..12].try_into().ok()?),
                count: u32::from_be_bytes(c[12..].try_into().ok()?),
            })
        })
        .collect()
}

/// Wall-clock helper kept here so callers don't hand-roll the bucket
/// arithmetic that decides how much timing leaks.
pub fn bucket_of(policy: &AnalyticsPolicy, t: Timestamp) -> u64 {
    let width = policy.bucket_ms.max(1);
    t.0 / width * width
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, StreamId};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::{AppendRequest, NewEvent};
    use siar_domain::DeviceId;

    const MSG: EventTypeId = EventTypeId(1);
    const SENSITIVE: EventTypeId = EventTypeId(2);
    const DAY: u64 = 24 * 3_600_000;

    fn ev(ty: EventTypeId, at: u64, payload: &[u8]) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(at),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: payload.to_vec(),
        }
    }

    async fn log(events: Vec<NewEvent>) -> InMemoryEventStore {
        let s = InMemoryEventStore::new();
        s.append(AppendRequest {
            stream_id: StreamId::from_name("s"),
            expected_version: 0,
            events,
        })
        .await
        .unwrap();
        s
    }

    #[tokio::test]
    async fn only_allow_listed_types_are_counted_into_coarse_buckets() {
        let mut events = Vec::new();
        for i in 0..6 {
            events.push(ev(MSG, DAY + i * 1000, b"hi")); // day 1
        }
        for i in 0..7 {
            events.push(ev(MSG, 3 * DAY + 5_000_000 + i, b"hi")); // day 3
        }
        for _ in 0..10 {
            events.push(ev(SENSITIVE, DAY, b"x")); // not allowed
        }
        let store = log(events).await;
        let policy = AnalyticsPolicy::new([MSG]);
        let recs = analytics_from_store(&store, &policy, 4).await.unwrap();
        assert_eq!(
            recs,
            vec![
                AnalyticsRecord {
                    event_type: 1,
                    bucket_start_ms: DAY,
                    count: 6
                },
                AnalyticsRecord {
                    event_type: 1,
                    bucket_start_ms: 3 * DAY,
                    count: 7
                },
            ]
        );
        assert!(
            recs.iter().all(|r| r.event_type != 2),
            "unlisted type is invisible"
        );
    }

    #[tokio::test]
    async fn small_buckets_are_suppressed_not_reported() {
        let events = (0..4).map(|i| ev(MSG, DAY + i, b"")).collect();
        let store = log(events).await;
        let recs = analytics_from_store(&store, &AnalyticsPolicy::new([MSG]), 10)
            .await
            .unwrap();
        assert!(recs.is_empty(), "4 < min_count 5");
        let lax = analytics_from_store(&store, &AnalyticsPolicy::new([MSG]).with_min_count(1), 10)
            .await
            .unwrap();
        assert_eq!(lax[0].count, 4);
    }

    #[tokio::test]
    async fn nothing_identifying_or_textual_can_appear_in_the_output() {
        let secret_payload = b"SECRET-PAYLOAD-TEXT";
        let events: Vec<NewEvent> = (0..8).map(|i| ev(MSG, DAY + i, secret_payload)).collect();
        let ids: Vec<[u8; 16]> = events
            .iter()
            .map(|e| *e.event_id.as_uuid().as_bytes())
            .collect();
        let store = log(events).await;
        let policy = AnalyticsPolicy::new([MSG]);
        let recs = analytics_from_store(&store, &policy, 3).await.unwrap();

        let wire = encode_for_upload(&recs);
        assert_eq!(wire.len(), recs.len() * ANALYTICS_RECORD_LEN);
        assert!(!wire
            .windows(secret_payload.len())
            .any(|w| w == secret_payload));
        for id in &ids {
            assert!(
                !wire.windows(16).any(|w| w == id),
                "no event id on the wire"
            );
        }
        let debug = format!("{recs:?}");
        assert!(!debug.contains("SECRET"));
        // And the timestamps are only bucket-coarse: every event was
        // created at DAY+i, the record says DAY.
        assert!(recs
            .iter()
            .all(|r| r.bucket_start_ms % policy.bucket_ms == 0));
        assert_eq!(decode_upload(&wire).unwrap(), recs);
        assert_eq!(decode_upload(&wire[..wire.len() - 1]), None);
    }

    #[tokio::test]
    async fn streaming_in_small_pages_equals_one_pass() {
        let events: Vec<NewEvent> = (0..40).map(|i| ev(MSG, DAY * (i % 3) + i, b"")).collect();
        let store = log(events).await;
        let policy = AnalyticsPolicy::new([MSG]).with_min_count(1);
        let a = analytics_from_store(&store, &policy, 1).await.unwrap();
        let b = analytics_from_store(&store, &policy, 1000).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a.iter().map(|r| r.count).sum::<u32>(), 40);
    }

    #[test]
    fn an_empty_allow_list_exports_nothing_at_all() {
        let policy = AnalyticsPolicy::new([]);
        assert!(build_analytics(&policy, &[]).is_empty());
    }

    #[test]
    fn bucket_arithmetic_floors_to_the_policy_width() {
        let p = AnalyticsPolicy::new([MSG]).with_bucket_ms(3_600_000);
        assert_eq!(
            bucket_of(&p, Timestamp(3_600_000 * 5 + 1234)),
            3_600_000 * 5
        );
        let zero = AnalyticsPolicy::new([MSG]).with_bucket_ms(0);
        assert_eq!(zero.bucket_ms, 1, "a zero width cannot divide by zero");
    }
}
