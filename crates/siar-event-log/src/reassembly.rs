//! `04-offline-event-log-architecture.md` §25 "Out-of-Order Events",
//! §26 "Gap Detection" — the inbound half of §92 Phase 6 (outbound is
//! [`crate::replication`]).
//!
//! §25: "remote events can arrive out of order. Possible handling: hold
//! unresolved, request missing predecessor, apply only after dependency
//! satisfied. **Do not silently discard valid future events.**" §26: a
//! stream expecting version 44 that receives 46 is marked incomplete and
//! "reconciliation" is requested. [`StreamReassembler`] does exactly that
//! for stream-versioned replication:
//!
//! - version == head + 1 → appended now, then every held successor that
//!   has become contiguous is appended too;
//! - version > head + 1 → **held** (not applied, not dropped) and the
//!   [`StreamGap`] reported so the caller can ask the sender for the
//!   missing predecessors;
//! - version <= head → a duplicate (§24) *if it is the same event*;
//!   a different event at an already-occupied version is
//!   [`ReceiveError::Diverged`] — reported, never overwritten (§54).
//!
//! ## Held events are bounded, and refusal is loud
//!
//! §59's memory discipline rules out an unbounded hold buffer, but §25
//! rules out silent discard. The reconciliation: when a limit is hit,
//! [`StreamReassembler::receive`] returns [`ReceiveError::HoldFull`]
//! *without accepting the event*. Because the caller only acknowledges
//! what [`ReceiveOutcome`] says was accepted (and a cursor only advances
//! on acknowledgement — [`crate::sync_cursor`]), the sender still holds
//! the event and will resend it once the gap fills. Nothing is lost;
//! the cost of refusing is a retransmission.
//!
//! ## What to acknowledge
//!
//! Acknowledge `head` from [`ReceiveOutcome::Applied`]/
//! [`ReceiveOutcome::Duplicate`] — the highest *contiguous, durably
//! stored* version — never the version of a merely-[`Held`]
//! event. (§23: ACK only after persistence.) The held buffer is
//! volatile: after a crash it is gone, the acknowledged head is
//! unaffected, and the sender simply resends from there.
//!
//! ## Preconditions
//!
//! Events must already have passed the §23 pipeline
//! (`siar-remote-ingestion`: protocol, identity, authorization) — this
//! type orders and stores, it does not authenticate. An event held for a
//! while was authorized *when received*; a caller whose authorization
//! can be revoked in between should re-check at apply time, which this
//! layer cannot see.
//!
//! The stream head comes from [`EventStore::stream_head`] once per
//! stream and is then tracked locally. Another writer appending to the
//! same stream in the same process would make the cached head stale; the
//! resulting [`EventStoreError::ConcurrencyConflict`] evicts the cache
//! and is returned, so a retry re-reads it. (Single-writer streams —
//! the only kind cursor-based replication is defined for — never see it.)
//!
//! [`Held`]: ReceiveOutcome::Held

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::gap::{detect_gap, StreamGap};
use crate::ids::StreamId;
use crate::replication::ReplicationEventV1;
use crate::store::{AppendRequest, EventStore, EventStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReassemblyLimits {
    pub max_held_per_stream: usize,
    pub max_held_total: usize,
}

impl Default for ReassemblyLimits {
    fn default() -> Self {
        Self {
            max_held_per_stream: 256,
            max_held_total: 4096,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// Stored. `head` is the new contiguous head — acknowledge it.
    Applied {
        appended: u32,
        head: u64,
        still_held: u32,
    },
    /// Already stored (§24). `head` is the current contiguous head.
    Duplicate { head: u64 },
    /// Ahead of the head: kept, not applied. Request the missing range
    /// `gap.expected_version .. gap.received_version` from the sender.
    Held {
        gap: StreamGap,
        held_for_stream: u32,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReceiveError {
    #[error(transparent)]
    Store(#[from] EventStoreError),
    /// A *different* event already occupies this version of the stream:
    /// two writers, or a corrupted peer. Domain-specific (§54) — surfaced,
    /// never resolved here.
    #[error("stream {stream:?} version {version} holds a different event than the one received")]
    Diverged { stream: StreamId, version: u64 },
    /// A hold limit was reached; the event was **not** accepted and must
    /// not be acknowledged. See the module doc.
    #[error("hold buffer full while waiting for {gap:?}; event not accepted")]
    HoldFull { gap: StreamGap },
}

#[derive(Debug, Default)]
pub struct StreamReassembler {
    limits: ReassemblyLimits,
    held: HashMap<StreamId, BTreeMap<u64, ReplicationEventV1>>,
    heads: HashMap<StreamId, u64>,
    total_held: usize,
}

impl StreamReassembler {
    pub fn new(limits: ReassemblyLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    pub fn total_held(&self) -> usize {
        self.total_held
    }

    /// One entry per stream with held events: the gap to request.
    pub fn pending_gaps(&self) -> Vec<(StreamId, StreamGap)> {
        let mut out: Vec<(StreamId, StreamGap)> = self
            .held
            .iter()
            .filter_map(|(stream, held)| {
                let lowest = *held.keys().next()?;
                let head = self.heads.get(stream).copied().unwrap_or(0);
                detect_gap(head + 1, lowest).map(|g| (*stream, g))
            })
            .collect();
        out.sort_unstable_by_key(|(s, _)| *s.as_bytes());
        out
    }

    async fn head_of<S: EventStore + ?Sized>(
        &mut self,
        store: &S,
        stream: StreamId,
    ) -> Result<u64, EventStoreError> {
        if let Some(h) = self.heads.get(&stream) {
            return Ok(*h);
        }
        let h = store.stream_head(stream).await?;
        self.heads.insert(stream, h);
        Ok(h)
    }

    /// `Ok(())` if `event` is exactly what is stored at its version.
    async fn check_stored<S: EventStore + ?Sized>(
        store: &S,
        event: &ReplicationEventV1,
    ) -> Result<(), ReceiveError> {
        let at = store
            .read_stream(event.stream_id, event.stream_version - 1, 1)
            .await?;
        match at.first() {
            Some(s)
                if s.envelope.stream_version == event.stream_version
                    && s.envelope.event_id == event.event_id =>
            {
                Ok(())
            }
            _ => Err(ReceiveError::Diverged {
                stream: event.stream_id,
                version: event.stream_version,
            }),
        }
    }

    async fn append_next<S: EventStore + ?Sized>(
        &mut self,
        store: &S,
        event: ReplicationEventV1,
    ) -> Result<u64, ReceiveError> {
        let stream = event.stream_id;
        let version = event.stream_version;
        let expected = version - 1;
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: expected,
                events: vec![event.into_new_event()],
            })
            .await;
        match result {
            Ok(r) if r.local_offsets.first().is_some_and(|o| o.is_some()) => {
                self.heads.insert(stream, r.new_version);
                Ok(r.new_version)
            }
            // The event id exists elsewhere but this stream did not
            // advance: not a duplicate of *this* slot.
            Ok(_) => Err(ReceiveError::Diverged { stream, version }),
            Err(e) => {
                self.heads.remove(&stream);
                Err(e.into())
            }
        }
    }

    /// See the module doc for the full contract.
    pub async fn receive<S: EventStore + ?Sized>(
        &mut self,
        store: &S,
        event: ReplicationEventV1,
    ) -> Result<ReceiveOutcome, ReceiveError> {
        let stream = event.stream_id;
        let version = event.stream_version;
        if version == 0 {
            // Versions start at 1; 0 can never name a real event.
            return Err(ReceiveError::Diverged { stream, version });
        }
        let head = self.head_of(store, stream).await?;

        if version <= head {
            Self::check_stored(store, &event).await?;
            return Ok(ReceiveOutcome::Duplicate { head });
        }

        if version > head + 1 {
            return self.hold(stream, head, event);
        }

        // version == head + 1: apply, then drain contiguous successors.
        let mut appended = 1u32;
        let mut new_head = self.append_next(store, event).await?;
        loop {
            let Some(held) = self.held.get_mut(&stream) else {
                break;
            };
            // Anything now at or below the head was overtaken by a
            // direct append: verify it matches, then discard.
            while let Some((&v, _)) = held.iter().next() {
                if v > new_head {
                    break;
                }
                let stale = held.remove(&v).expect("key just observed");
                self.total_held -= 1;
                Self::check_stored(store, &stale).await?;
            }
            let Some(next) = self
                .held
                .get_mut(&stream)
                .and_then(|h| h.remove(&(new_head + 1)))
            else {
                break;
            };
            self.total_held -= 1;
            new_head = self.append_next(store, next).await?;
            appended += 1;
        }
        let still_held = self.held.get(&stream).map_or(0, BTreeMap::len) as u32;
        if still_held == 0 {
            self.held.remove(&stream);
        }
        Ok(ReceiveOutcome::Applied {
            appended,
            head: new_head,
            still_held,
        })
    }

    fn hold(
        &mut self,
        stream: StreamId,
        head: u64,
        event: ReplicationEventV1,
    ) -> Result<ReceiveOutcome, ReceiveError> {
        let version = event.stream_version;
        let gap = StreamGap {
            expected_version: head + 1,
            received_version: version,
        };
        let held = self.held.entry(stream).or_default();

        if let Some(existing) = held.get(&version) {
            return if existing.event_id == event.event_id {
                Ok(ReceiveOutcome::Held {
                    gap,
                    held_for_stream: held.len() as u32,
                })
            } else {
                Err(ReceiveError::Diverged { stream, version })
            };
        }
        if held.len() >= self.limits.max_held_per_stream
            || self.total_held >= self.limits.max_held_total
        {
            if held.is_empty() {
                self.held.remove(&stream);
            }
            return Err(ReceiveError::HoldFull { gap });
        }
        held.insert(version, event);
        self.total_held += 1;
        let lowest = *held.keys().next().expect("just inserted");
        Ok(ReceiveOutcome::Held {
            // Report the nearest hole, not merely this event's.
            gap: StreamGap {
                expected_version: head + 1,
                received_version: lowest,
            },
            held_for_stream: held.len() as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::NewEvent;
    use proptest::prelude::*;
    use siar_domain::DeviceId;

    fn wire(stream: StreamId, version: u64, author: DeviceId) -> ReplicationEventV1 {
        ReplicationEventV1 {
            event_id: EventId::new(),
            stream_id: stream,
            stream_version: version,
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp(version),
            origin_device: author,
            correlation_id: None,
            causation_id: None,
            payload: version.to_le_bytes().to_vec(),
        }
    }

    fn stream() -> StreamId {
        StreamId::from_name("peer/stream")
    }

    #[tokio::test]
    async fn in_order_events_apply_immediately() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::default();
        let a = DeviceId::new();
        for v in 1..=3 {
            let out = r.receive(&store, wire(stream(), v, a)).await.unwrap();
            assert_eq!(
                out,
                ReceiveOutcome::Applied {
                    appended: 1,
                    head: v,
                    still_held: 0
                }
            );
        }
        let stored = store.read_stream(stream(), 0, 10).await.unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].envelope.origin, EventOrigin::RemoteDevice(a));
    }

    #[tokio::test]
    async fn the_spec_example_a_future_event_is_held_then_applied_when_the_gap_fills() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::default();
        let a = DeviceId::new();
        for v in 1..=2 {
            r.receive(&store, wire(stream(), v, a)).await.unwrap();
        }
        // Expecting 3, receive 5 (the §26 shape).
        let out = r.receive(&store, wire(stream(), 5, a)).await.unwrap();
        assert_eq!(
            out,
            ReceiveOutcome::Held {
                gap: StreamGap {
                    expected_version: 3,
                    received_version: 5
                },
                held_for_stream: 1
            }
        );
        assert_eq!(r.pending_gaps().len(), 1);
        assert_eq!(
            store.stream_head(stream()).await.unwrap(),
            2,
            "5 not applied"
        );

        // 4 also early: still a gap, nearest hole is 3.
        let out = r.receive(&store, wire(stream(), 4, a)).await.unwrap();
        assert!(matches!(
            out,
            ReceiveOutcome::Held {
                gap: StreamGap {
                    expected_version: 3,
                    received_version: 4
                },
                held_for_stream: 2
            }
        ));

        // 3 arrives: 3, 4, 5 all land.
        let out = r.receive(&store, wire(stream(), 3, a)).await.unwrap();
        assert_eq!(
            out,
            ReceiveOutcome::Applied {
                appended: 3,
                head: 5,
                still_held: 0
            }
        );
        assert_eq!(r.total_held(), 0);
        assert!(r.pending_gaps().is_empty());
        assert_eq!(store.stream_head(stream()).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn redelivery_is_a_duplicate_and_a_conflicting_event_is_divergence() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::default();
        let a = DeviceId::new();
        let first = wire(stream(), 1, a);
        r.receive(&store, first.clone()).await.unwrap();

        assert_eq!(
            r.receive(&store, first).await.unwrap(),
            ReceiveOutcome::Duplicate { head: 1 }
        );
        let rival = wire(stream(), 1, a); // fresh EventId, same slot
        assert_eq!(
            r.receive(&store, rival).await.unwrap_err(),
            ReceiveError::Diverged {
                stream: stream(),
                version: 1
            }
        );
        // And the same inside the hold buffer.
        r.receive(&store, wire(stream(), 3, a)).await.unwrap();
        assert!(matches!(
            r.receive(&store, wire(stream(), 3, a)).await,
            Err(ReceiveError::Diverged { .. })
        ));
        assert_eq!(
            store.stream_head(stream()).await.unwrap(),
            1,
            "nothing overwritten"
        );
    }

    #[tokio::test]
    async fn a_full_hold_buffer_refuses_loudly_and_loses_nothing() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::new(ReassemblyLimits {
            max_held_per_stream: 2,
            max_held_total: 100,
        });
        let a = DeviceId::new();
        let evs: Vec<_> = (1..=4).map(|v| wire(stream(), v, a)).collect();

        r.receive(&store, evs[3].clone()).await.unwrap(); // v4 held
        r.receive(&store, evs[2].clone()).await.unwrap(); // v3 held
        let err = r.receive(&store, evs[1].clone()).await.unwrap_err(); // v2: no room
        assert!(matches!(err, ReceiveError::HoldFull { .. }), "{err:?}");
        assert_eq!(
            r.total_held(),
            2,
            "refused event was not silently kept or dropped"
        );

        // The sender never got an ack for v2, so it resends in order.
        r.receive(&store, evs[0].clone()).await.unwrap(); // v1 applies
        let out = r.receive(&store, evs[1].clone()).await.unwrap();
        assert_eq!(
            out,
            ReceiveOutcome::Applied {
                appended: 3,
                head: 4,
                still_held: 0
            }
        );
    }

    #[tokio::test]
    async fn the_global_hold_limit_applies_across_streams() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::new(ReassemblyLimits {
            max_held_per_stream: 10,
            max_held_total: 2,
        });
        let a = DeviceId::new();
        r.receive(&store, wire(StreamId::from_name("x"), 2, a))
            .await
            .unwrap();
        r.receive(&store, wire(StreamId::from_name("y"), 2, a))
            .await
            .unwrap();
        assert!(matches!(
            r.receive(&store, wire(StreamId::from_name("z"), 2, a))
                .await,
            Err(ReceiveError::HoldFull { .. })
        ));
        assert_eq!(r.pending_gaps().len(), 2);
    }

    #[tokio::test]
    async fn version_zero_is_rejected() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::default();
        assert!(matches!(
            r.receive(&store, wire(stream(), 0, DeviceId::new())).await,
            Err(ReceiveError::Diverged { .. })
        ));
    }

    #[tokio::test]
    async fn a_stale_head_cache_is_evicted_on_conflict_and_the_retry_succeeds() {
        let store = InMemoryEventStore::new();
        let mut r = StreamReassembler::default();
        let a = DeviceId::new();
        r.receive(&store, wire(stream(), 1, a)).await.unwrap();
        // Another writer slips an event in behind the reassembler's back.
        store
            .append(AppendRequest {
                stream_id: stream(),
                expected_version: 1,
                events: vec![NewEvent {
                    event_id: EventId::new(),
                    event_type: EventTypeId(1),
                    schema_version: 1,
                    created_at: Timestamp(0),
                    origin: EventOrigin::System,
                    correlation_id: None,
                    causation_id: None,
                    payload: vec![],
                }],
            })
            .await
            .unwrap();
        let err = r.receive(&store, wire(stream(), 2, a)).await.unwrap_err();
        assert!(matches!(
            err,
            ReceiveError::Store(EventStoreError::ConcurrencyConflict { .. })
        ));
        // Re-read head (2): version 2 is now occupied by a different
        // event — reported, not overwritten.
        let err = r.receive(&store, wire(stream(), 2, a)).await.unwrap_err();
        assert!(matches!(err, ReceiveError::Diverged { .. }));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// Whatever order — and however many duplicate deliveries —
        /// the network produces, the receiver ends with exactly the
        /// sender's stream, nothing held, nothing lost.
        #[test]
        fn any_arrival_order_converges_to_the_sender_stream(
            order in Just((1u64..=12).collect::<Vec<_>>()).prop_shuffle(),
            dupes in proptest::collection::vec(0usize..12, 0..8),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            rt.block_on(async {
                let author = DeviceId::new();
                let sent: Vec<_> = (1..=12).map(|v| wire(stream(), v, author)).collect();
                let store = InMemoryEventStore::new();
                let mut r = StreamReassembler::default();

                let mut deliveries: Vec<usize> = order.iter().map(|v| (*v - 1) as usize).collect();
                for d in &dupes { deliveries.push(*d); }
                for idx in deliveries {
                    r.receive(&store, sent[idx].clone()).await.unwrap();
                }

                let stored = store.read_stream(stream(), 0, 100).await.unwrap();
                prop_assert_eq!(stored.len(), 12);
                for (s, w) in stored.iter().zip(&sent) {
                    prop_assert_eq!(s.envelope.event_id, w.event_id);
                    prop_assert_eq!(s.envelope.stream_version, w.stream_version);
                }
                prop_assert_eq!(r.total_held(), 0);
                prop_assert!(r.pending_gaps().is_empty());
                Ok(())
            })?;
        }
    }
}
