//! `04-offline-event-log-architecture.md` §49 "Replication Scope", §50
//! "Own-Device Sync", §51 "Peer and Group Sync", §52 "Local Storage
//! Envelope vs Network Envelope" — the outbound half of §92 Phase 6.
//! (The inbound half, §25/§26/§53, is [`crate::reassembly`]; the
//! cursors both halves share are [`crate::sync_cursor`].)
//!
//! ## §49: scope is decided per event type, and the default is "stay home"
//!
//! "Not every local event should leave the device." [`ScopeTable`]
//! maps an [`EventTypeId`] to a [`ReplicationScope`]. A type with no
//! entry is [`ReplicationScope::LocalOnly`]: a new event type — a
//! diagnostic, a draft, something added next release — never leaks to a
//! peer because someone forgot to classify it. Leaving the device
//! requires an explicit, reviewable registration.
//!
//! §50's "draft sync should remain optional" falls out of this: drafts
//! are simply not registered as `OwnDevices` unless the user opts in
//! (a settings layer calls [`ScopeTable::set`]).
//!
//! ## §51: the four checks belong to the caller's authorizer
//!
//! "Remote replication must check authorization, membership,
//! epoch/version, and history visibility policy before sending or
//! appending events." Those four facts live in
//! `siar-identity-multidevice`, group/MLS state, and per-conversation
//! policy — none of which this crate may depend on (see `lib.rs`).
//! [`ReplicationAuthorizer`] is the seam: one question per candidate
//! event, answered with a three-way [`Decision`]:
//!
//! - [`Decision::Allow`] — send it.
//! - [`Decision::Skip`] — *permanently* not for this peer (history
//!   predates their membership). The cursor may advance past it.
//! - [`Decision::Defer`] — *not yet* (epoch mismatch, authorization
//!   still pending). The batch **stops before it** and the cursor must
//!   not pass it, so the event is still there to send once the
//!   condition clears. Treating "not yet" as "never" would silently lose
//!   history for a peer who is about to be authorized.
//!
//! ## §52: the wire form is its own type
//!
//! [`ReplicationEventV1`] is deliberately *not* [`StoredEvent`]. It
//! drops `local_offset` (§6: device-local, "never global truth across
//! devices"), names its origin as a bare [`DeviceId`], and carries an
//! explicit wire version in its name, so storage can evolve — new
//! columns, a different offset scheme — without a wire break, and wire
//! can add fields without a migration. Conversion goes through
//! [`ReplicationEventV1::from_stored`] / [`ReplicationEventV1::
//! into_new_event`]; neither direction is a plain field copy, and the
//! second one *re-labels* the origin as
//! [`EventOrigin::RemoteDevice`].
//!
//! `into_new_event` does **not** verify that `origin_device` really
//! authored the event — that is the §23 pipeline's identity-verification
//! step (`siar-remote-ingestion`), which must run before a received
//! event reaches [`crate::reassembly`].
//!
//! ## Stream layout rule: replicate only streams whose events all replicate
//!
//! Cursor replication orders by contiguous stream version, and the
//! receiver ([`crate::reassembly`]) holds anything after a missing
//! version. A [`ReplicationScope::LocalOnly`] or authorizer-skipped event
//! *inside* a replicated stream therefore leaves a hole the receiver
//! cannot distinguish from loss. The fix is in stream layout, not in
//! this code: give local-only facts (diagnostics, unsynced drafts)
//! their own stream, and keep per-peer-visibility differences (history
//! before a member joined) to stream boundaries rather than mixing them
//! in one. [`OutboundBatch::has_version_holes`] detects a violation at
//! send time so it is caught in testing instead of surfacing as a
//! mysteriously stalled replica.
//!
//! ## §54: no generic conflict resolver, on purpose
//!
//! Nothing here resolves conflicts, and nothing will: "the event store
//! records facts; the domain resolves meaning." Two replicas appending
//! to the *same* stream concurrently surface as
//! [`crate::reassembly::ReceiveError::Diverged`] — a fact reported
//! upward for the owning domain's state machine (message streams: both
//! valid; revocations: merge conservatively; membership: authenticated
//! state machine) — never silently merged or overwritten here. The
//! practical rule this implies: replicate streams that have a single
//! writer (a conversation's per-sender stream, a device's own state
//! stream); where a stream is genuinely multi-writer, the domain
//! chooses its stream layout so that it isn't.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;

use crate::command_flow::EventRef;
use crate::envelope::EventOrigin;
use crate::ids::{CorrelationId, EventId, EventTypeId, StreamId, Timestamp};
use crate::store::{EventStore, EventStoreError, NewEvent, StoredEvent};

/// §49, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplicationScope {
    LocalOnly,
    OwnDevices,
    ConversationPeers,
    GroupMembers,
    ExplicitRecipients,
    PublicSigned,
}

impl ReplicationScope {
    pub const fn leaves_device(self) -> bool {
        !matches!(self, Self::LocalOnly)
    }
}

/// See the module doc: unregistered types are `LocalOnly`.
#[derive(Debug, Clone, Default)]
pub struct ScopeTable {
    scopes: HashMap<EventTypeId, ReplicationScope>,
}

impl ScopeTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder form of [`Self::set`].
    pub fn with(mut self, event_type: EventTypeId, scope: ReplicationScope) -> Self {
        self.set(event_type, scope);
        self
    }

    pub fn set(&mut self, event_type: EventTypeId, scope: ReplicationScope) {
        self.scopes.insert(event_type, scope);
    }

    pub fn scope_of(&self, event_type: EventTypeId) -> ReplicationScope {
        self.scopes
            .get(&event_type)
            .copied()
            .unwrap_or(ReplicationScope::LocalOnly)
    }
}

/// Why an authorizer said "not for this peer, ever".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The event predates the peer's membership / visible history.
    HistoryNotVisible,
    NotAMember,
}

/// Why an authorizer said "not yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    NotYetAuthorized,
    EpochMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Skip(SkipReason),
    Defer(DeferReason),
}

/// §51's four checks, behind one question. See the module doc.
pub trait ReplicationAuthorizer: Send + Sync {
    fn decide(&self, peer: DeviceId, event: &EventRef<'_>, scope: ReplicationScope) -> Decision;
}

/// Allows only [`ReplicationScope::OwnDevices`] events, and only to
/// peers `is_own_device` accepts. The §50 default for a node that has
/// not yet wired group/peer authorization: own-device sync works, and
/// everything else fails closed.
pub struct OwnDevicesOnly<F: Fn(DeviceId) -> bool + Send + Sync>(pub F);

impl<F: Fn(DeviceId) -> bool + Send + Sync> ReplicationAuthorizer for OwnDevicesOnly<F> {
    fn decide(&self, peer: DeviceId, _event: &EventRef<'_>, scope: ReplicationScope) -> Decision {
        match scope {
            ReplicationScope::OwnDevices if (self.0)(peer) => Decision::Allow,
            ReplicationScope::OwnDevices => Decision::Skip(SkipReason::NotAMember),
            // Peer/group/recipient scopes need real membership state
            // this authorizer does not have. Defer, never Skip: the
            // owning layer may authorize them later.
            _ => Decision::Defer(DeferReason::NotYetAuthorized),
        }
    }
}

/// §52's wire event. See the module doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplicationEventV1 {
    pub event_id: EventId,
    pub stream_id: StreamId,
    pub stream_version: u64,
    pub event_type: EventTypeId,
    pub schema_version: u16,
    pub created_at: Timestamp,
    /// The device that authored the event.
    pub origin_device: DeviceId,
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<EventId>,
    pub payload: Vec<u8>,
}

impl ReplicationEventV1 {
    /// `local_device` stands in as author for events with no device
    /// origin (`Imported`, `Recovery`, `System`) — they originate here.
    pub fn from_stored(stored: &StoredEvent, local_device: DeviceId) -> Self {
        let e = &stored.envelope;
        let origin_device = match e.origin {
            EventOrigin::LocalDevice(d) | EventOrigin::RemoteDevice(d) => d,
            EventOrigin::Imported | EventOrigin::Recovery | EventOrigin::System => local_device,
        };
        Self {
            event_id: e.event_id,
            stream_id: e.stream_id,
            stream_version: e.stream_version,
            event_type: e.event_type,
            schema_version: e.schema_version,
            created_at: e.created_at,
            origin_device,
            correlation_id: e.correlation_id,
            causation_id: e.causation_id,
            payload: e.payload.clone(),
        }
    }

    /// Re-labels the origin as [`EventOrigin::RemoteDevice`]; see the
    /// module doc on what this does *not* verify.
    pub fn into_new_event(self) -> NewEvent {
        NewEvent {
            event_id: self.event_id,
            event_type: self.event_type,
            schema_version: self.schema_version,
            created_at: self.created_at,
            origin: EventOrigin::RemoteDevice(self.origin_device),
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            payload: self.payload,
        }
    }
}

/// Which slice of which stream to prepare for which peer.
#[derive(Debug, Clone, Copy)]
pub struct OutboundRequest {
    pub local_device: DeviceId,
    pub peer: DeviceId,
    pub stream: StreamId,
    /// The peer's [`crate::sync_cursor::SyncCursor::acknowledged_version`].
    pub acknowledged_version: u64,
    pub limit: usize,
}

/// What an outbound pass left out, for diagnostics (§81).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkipCounts {
    pub local_only: u32,
    pub skipped_by_policy: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundBatch {
    pub events: Vec<ReplicationEventV1>,
    /// Highest stream version this pass is *finished with*: sent,
    /// scoped out, or permanently skipped. Acknowledging it is safe;
    /// it is never past a [`Decision::Defer`]red event.
    pub through_version: u64,
    pub skipped: SkipCounts,
    /// A [`Decision::Defer`] ended the batch early.
    pub deferred: Option<DeferReason>,
    /// The page was full; there may be more behind it.
    pub more_available: bool,
    /// `true` if a sent event's version is not exactly one past the
    /// previous sent version (or the acknowledged version, for the
    /// first): something was filtered out between them. A receiver
    /// reassembling by contiguous version ([`crate::reassembly`]) cannot
    /// tell a filtered event from a lost one and would hold everything
    /// after the hole forever. See the module doc's "stream layout"
    /// rule — treat this flag as a bug in the stream's layout, not as
    /// a runtime condition to work around.
    pub has_version_holes: bool,
}

/// Reads the next slice of `stream` after `acknowledged_version` and
/// returns what may be sent to `peer`. Pure read — it neither sends nor
/// moves any cursor; the caller sends `events`, and only after the peer
/// acknowledges does it advance the cursor to `through_version` (see
/// [`crate::sync_cursor::CursorStore::acknowledge`]).
pub async fn next_outbound_batch<S, A>(
    store: &S,
    scopes: &ScopeTable,
    authorizer: &A,
    request: &OutboundRequest,
) -> Result<OutboundBatch, EventStoreError>
where
    S: EventStore + ?Sized,
    A: ReplicationAuthorizer + ?Sized,
{
    let OutboundRequest {
        local_device,
        peer,
        stream,
        acknowledged_version,
        limit,
    } = *request;
    let page = store
        .read_stream(stream, acknowledged_version, limit)
        .await?;
    let mut batch = OutboundBatch {
        events: Vec::new(),
        through_version: acknowledged_version,
        skipped: SkipCounts::default(),
        deferred: None,
        more_available: page.len() >= limit && limit > 0,
        has_version_holes: false,
    };
    let mut expected_next = acknowledged_version + 1;

    for stored in &page {
        let version = stored.envelope.stream_version;
        let scope = scopes.scope_of(stored.envelope.event_type);
        if !scope.leaves_device() {
            batch.skipped.local_only += 1;
            batch.through_version = version;
            continue;
        }
        match authorizer.decide(peer, &EventRef::from_stored(stored), scope) {
            Decision::Allow => {
                if version != expected_next {
                    batch.has_version_holes = true;
                }
                expected_next = version + 1;
                batch
                    .events
                    .push(ReplicationEventV1::from_stored(stored, local_device));
                batch.through_version = version;
            }
            Decision::Skip(_) => {
                batch.skipped.skipped_by_policy += 1;
                batch.through_version = version;
            }
            Decision::Defer(reason) => {
                batch.deferred = Some(reason);
                batch.more_available = true;
                break;
            }
        }
    }
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_store::InMemoryEventStore;
    use crate::store::AppendRequest;

    const MSG: EventTypeId = EventTypeId(1);
    const DIAG: EventTypeId = EventTypeId(2);
    const DRAFT: EventTypeId = EventTypeId(3);

    fn ev(ty: EventTypeId, payload: u8, origin: EventOrigin) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(7),
            origin,
            correlation_id: Some(CorrelationId::new()),
            causation_id: None,
            payload: vec![payload],
        }
    }

    async fn seeded(local: DeviceId, types: &[EventTypeId]) -> (InMemoryEventStore, StreamId) {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("own/state");
        let events = types
            .iter()
            .enumerate()
            .map(|(i, t)| ev(*t, i as u8, EventOrigin::LocalDevice(local)))
            .collect();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events,
            })
            .await
            .unwrap();
        (store, stream)
    }

    #[test]
    fn unregistered_types_never_leave_the_device() {
        let t = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
        assert_eq!(t.scope_of(MSG), ReplicationScope::OwnDevices);
        assert_eq!(t.scope_of(EventTypeId(999)), ReplicationScope::LocalOnly);
        assert!(!t.scope_of(EventTypeId(999)).leaves_device());
    }

    #[test]
    fn wire_round_trip_relabels_origin_and_drops_the_local_offset() {
        let author = DeviceId::new();
        let me = DeviceId::new();
        let original = ev(MSG, 9, EventOrigin::LocalDevice(author));
        let stored = StoredEvent {
            envelope: crate::envelope::EventEnvelope {
                event_id: original.event_id,
                stream_id: StreamId::from_name("s"),
                stream_version: 5,
                event_type: original.event_type,
                schema_version: 1,
                created_at: original.created_at,
                origin: original.origin,
                correlation_id: original.correlation_id,
                causation_id: None,
                payload: original.payload.clone(),
            },
            local_offset: crate::ids::LocalLogOffset(12345),
        };
        let wire = ReplicationEventV1::from_stored(&stored, me);
        assert_eq!(wire.origin_device, author);
        assert_eq!(wire.stream_version, 5);
        let back = wire.into_new_event();
        assert_eq!(back.origin, EventOrigin::RemoteDevice(author));
        assert_eq!(back.event_id, original.event_id);
        assert_eq!(back.payload, original.payload);

        // No device origin: the local device is the author.
        let mut sys = stored.clone();
        sys.envelope.origin = EventOrigin::System;
        assert_eq!(ReplicationEventV1::from_stored(&sys, me).origin_device, me);
    }

    #[tokio::test]
    async fn only_registered_events_to_authorized_peers_are_sent() {
        let me = DeviceId::new();
        let phone = DeviceId::new();
        let stranger = DeviceId::new();
        let (store, stream) = seeded(me, &[MSG, DIAG, MSG, DRAFT, MSG]).await;
        let scopes = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
        let auth = OwnDevicesOnly(move |d| d == phone);

        let b = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: phone,
                stream,
                acknowledged_version: 0,
                limit: 100,
            },
        )
        .await
        .unwrap();
        let payloads: Vec<u8> = b.events.iter().map(|e| e.payload[0]).collect();
        assert_eq!(payloads, vec![0, 2, 4], "diag and draft stayed home");
        assert_eq!(b.skipped.local_only, 2);
        assert_eq!(b.through_version, 5);
        assert!(b.deferred.is_none() && !b.more_available);

        // A device that is not ours gets nothing — and the cursor may
        // pass, since "not your device" is permanent.
        let b = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: stranger,
                stream,
                acknowledged_version: 0,
                limit: 100,
            },
        )
        .await
        .unwrap();
        assert!(b.events.is_empty());
        assert_eq!(b.skipped.skipped_by_policy, 3);
        assert_eq!(b.through_version, 5);
    }

    #[tokio::test]
    async fn a_batch_resumes_after_the_acknowledged_version_and_pages() {
        let me = DeviceId::new();
        let phone = DeviceId::new();
        let (store, stream) = seeded(me, &[MSG; 5]).await;
        let scopes = ScopeTable::new().with(MSG, ReplicationScope::OwnDevices);
        let auth = OwnDevicesOnly(move |d| d == phone);

        let b1 = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: phone,
                stream,
                acknowledged_version: 0,
                limit: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (b1.events.len(), b1.through_version, b1.more_available),
            (2, 2, true)
        );
        let b2 = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: phone,
                stream,
                acknowledged_version: b1.through_version,
                limit: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(b2.events[0].stream_version, 3);
        let b3 = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: phone,
                stream,
                acknowledged_version: b2.through_version,
                limit: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (b3.events.len(), b3.through_version, b3.more_available),
            (1, 5, false)
        );
    }

    struct DeferAt(u64);
    impl ReplicationAuthorizer for DeferAt {
        fn decide(&self, _p: DeviceId, e: &EventRef<'_>, _s: ReplicationScope) -> Decision {
            if e.payload[0] as u64 + 1 == self.0 {
                Decision::Defer(DeferReason::EpochMismatch)
            } else {
                Decision::Allow
            }
        }
    }

    #[tokio::test]
    async fn a_deferred_event_stops_the_batch_and_holds_the_cursor_back() {
        let me = DeviceId::new();
        let peer = DeviceId::new();
        let (store, stream) = seeded(me, &[MSG; 4]).await;
        let scopes = ScopeTable::new().with(MSG, ReplicationScope::GroupMembers);

        // Version 3 is "not yet": versions 1-2 go, the cursor stops at
        // 2, and 3-4 are NOT sent past the hole.
        let b = next_outbound_batch(
            &store,
            &scopes,
            &DeferAt(3),
            &OutboundRequest {
                local_device: me,
                peer,
                stream,
                acknowledged_version: 0,
                limit: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(b.events.len(), 2);
        assert_eq!(b.through_version, 2);
        assert_eq!(b.deferred, Some(DeferReason::EpochMismatch));
        assert!(b.more_available);

        // Once the condition clears, the same call picks up version 3.
        let b = next_outbound_batch(
            &store,
            &scopes,
            &DeferAt(99),
            &OutboundRequest {
                local_device: me,
                peer,
                stream,
                acknowledged_version: 2,
                limit: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            b.events
                .iter()
                .map(|e| e.stream_version)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[tokio::test]
    async fn group_scopes_fail_closed_without_real_membership_state() {
        let me = DeviceId::new();
        let phone = DeviceId::new();
        let (store, stream) = seeded(me, &[MSG]).await;
        let scopes = ScopeTable::new().with(MSG, ReplicationScope::GroupMembers);
        let auth = OwnDevicesOnly(move |d| d == phone);
        let b = next_outbound_batch(
            &store,
            &scopes,
            &auth,
            &OutboundRequest {
                local_device: me,
                peer: phone,
                stream,
                acknowledged_version: 0,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert!(b.events.is_empty());
        assert_eq!(b.deferred, Some(DeferReason::NotYetAuthorized));
        assert_eq!(b.through_version, 0, "nothing lost, nothing skipped");
    }
}
