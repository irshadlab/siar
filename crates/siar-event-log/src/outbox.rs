//! `04-offline-event-log-architecture.md` §14 "Transactional Outbox",
//! §15 "Event Log Is Not a Job Queue", §31 "Exactly-Once Is Not the
//! Goal" — §92 Phase 5's "outbox".
//!
//! §14's recommended hybrid, verbatim: "event log = semantic history,
//! outbox table = efficient pending-work queue", where one transaction
//! may commit the event AND its outbox row together, so that "if the
//! process dies, the outbox still exists." §15's rule is the reason
//! the second structure exists at all: "do not scan millions of old
//! events to find work." The log preserves meaning; this table
//! optimizes execution.
//!
//! ## What lives here
//!
//! - [`OperationId`] — the §31 "stable ID". Derived *deterministically*
//!   from `(source event, effect kind, slot)` rather than minted
//!   randomly, so the same logical effect always has the same id no
//!   matter how many times it is enqueued: by the original command, by
//!   a retry of that command, or by [`crate::outbox_reconcile`]
//!   rebuilding a lost table from history (§48). That single property
//!   is what makes enqueue idempotent and lets a receiver deduplicate
//!   on it ("at-least-once attempts + stable IDs + receiver
//!   deduplication = effectively-once logical outcome").
//! - [`OutboxEntry`] / [`OutboxState`] — one pending operation and its
//!   lifecycle (`Pending → InFlight → Done`, with `DeadLettered` as the
//!   terminal failure state).
//! - [`OutboxTable`] — the whole queue discipline as a plain,
//!   single-threaded data structure. Every real backend's *semantics*
//!   are pinned down once, here, with no locking or IO in the way;
//!   [`InMemoryOutbox`] and
//!   [`crate::memory_store::InMemoryEventStore`]'s embedded outbox are
//!   thin lock wrappers over it.
//! - [`Outbox`] — the queue trait. Native `async fn`-in-trait
//!   (`-> impl Future + Send`), not `async-trait`: no per-call heap
//!   allocation, and consumers ([`crate::effects::EffectProcessor`],
//!   [`crate::outbox_reconcile`]) are generic over it, so the whole
//!   path monomorphizes to direct calls. The trade-off is that
//!   `dyn Outbox` is not available; nothing in this workspace needs it.
//! - [`TransactionalOutbox`] — §14's "one transaction may commit the
//!   event and the outbox row": an [`EventStore`] that can append
//!   events and enqueue entries atomically.
//!
//! ## Leases instead of a separate "crashed worker" pass
//!
//! [`Outbox::claim_due`] marks an entry `InFlight` with a
//! `lease_until` deadline. If the process dies mid-effect, the lease
//! simply expires and the entry becomes claimable again — recovery of
//! a crashed worker needs no special code path. [`Outbox::
//! release_all_in_flight`] exists for the §47 startup case, where the
//! node *knows* no worker from the previous run is alive and would
//! rather not wait out the leases.
//!
//! `attempts` is incremented at **claim** time, not at failure time, so
//! it counts real tries including ones that crashed — an honest input
//! for the §32 backoff policy.
//!
//! ## Payload discipline
//!
//! §15 lists "transfer chunks" among things dedicated work tables hold;
//! this table holds *pointers to work*, not bulk data.
//! [`MAX_OUTBOX_PAYLOAD_BYTES`] is deliberately much smaller than
//! `store::DEFAULT_MAX_EVENT_PAYLOAD_BYTES`: carry a `BlobId`/`ManifestId`
//! (§43), not the bytes.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;

use thiserror::Error;

use crate::envelope::EventEnvelope;
use crate::ids::{EventId, StreamId, Timestamp};
use crate::store::{AppendRequest, AppendResult, EventStore, EventStoreError};

/// §15: a work table carries pointers, not bulk data.
pub const MAX_OUTBOX_PAYLOAD_BYTES: usize = 16 * 1024;

/// Bound on the stored `last_error` text — an error string is
/// diagnostics, not a log sink.
pub const MAX_OUTBOX_ERROR_CHARS: usize = 256;

/// Which kind of external effect an entry represents (§30's examples:
/// send a queued message, run a transfer, route/DTN-forward an
/// emergency report). A plain numeric tag assigned by the owning
/// domain crate, same convention as [`crate::ids::EventTypeId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EffectKind(pub u32);

/// §31's "stable ID". See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId([u8; 16]);

impl OperationId {
    /// Deterministic: same inputs, same id, on every device and every
    /// run. `slot` distinguishes several effects of the same kind
    /// planned from one event (almost always `0`).
    pub fn derive(source: EventId, kind: EffectKind, slot: u16) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"siar/outbox-operation/v1");
        hasher.update(source.as_uuid().as_bytes());
        hasher.update(&kind.0.to_le_bytes());
        hasher.update(&slot.to_le_bytes());
        let mut out = [0u8; 16];
        out.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Self(out)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub(crate) fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(32);
        for b in self.0 {
            s.push(HEX[(b >> 4) as usize] as char);
            s.push(HEX[(b & 0x0f) as usize] as char);
        }
        s
    }

    pub(crate) fn from_hex(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();
        if bytes.len() != 32 {
            return None;
        }
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 16];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Some(Self(out))
    }
}

/// Lifecycle of one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxState {
    /// Waiting for `not_before`.
    Pending,
    /// Claimed by a worker until `lease_until`.
    InFlight,
    /// The effect's logical outcome is achieved.
    Done,
    /// Terminal failure — retries exhausted or the handler declared
    /// the failure permanent. Retained for diagnostics (§81); never
    /// retried automatically.
    DeadLettered,
}

impl OutboxState {
    pub(crate) const fn code(self) -> i64 {
        match self {
            Self::Pending => 0,
            Self::InFlight => 1,
            Self::Done => 2,
            Self::DeadLettered => 3,
        }
    }

    pub(crate) const fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Pending),
            1 => Some(Self::InFlight),
            2 => Some(Self::Done),
            3 => Some(Self::DeadLettered),
            _ => None,
        }
    }

    /// `Pending` or `InFlight` — work still owed.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::InFlight)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    pub operation_id: OperationId,
    pub kind: EffectKind,
    /// The event whose commit created this obligation (§8 causation).
    pub source_event_id: EventId,
    pub stream_id: StreamId,
    pub payload: Vec<u8>,
    pub state: OutboxState,
    /// Incremented at claim time — see the module doc.
    pub attempts: u32,
    /// Earliest time the next attempt may start.
    pub not_before: Timestamp,
    pub lease_until: Option<Timestamp>,
    pub created_at: Timestamp,
    /// Last state change; drives [`Outbox::purge_done`].
    pub updated_at: Timestamp,
    pub last_error: Option<String>,
}

impl OutboxEntry {
    /// Builds a `Pending` entry, immediately due. The id is derived,
    /// never random — see [`OperationId::derive`].
    pub fn new(
        source_event_id: EventId,
        stream_id: StreamId,
        kind: EffectKind,
        slot: u16,
        payload: Vec<u8>,
        now: Timestamp,
    ) -> Result<Self, OutboxError> {
        validate_payload(payload.len())?;
        Ok(Self {
            operation_id: OperationId::derive(source_event_id, kind, slot),
            kind,
            source_event_id,
            stream_id,
            payload,
            state: OutboxState::Pending,
            attempts: 0,
            not_before: now,
            lease_until: None,
            created_at: now,
            updated_at: now,
            last_error: None,
        })
    }

    /// Convenience for the common case: the entry is caused by a
    /// stored event.
    pub fn for_event(
        event: &EventEnvelope,
        kind: EffectKind,
        slot: u16,
        payload: Vec<u8>,
        now: Timestamp,
    ) -> Result<Self, OutboxError> {
        Self::new(event.event_id, event.stream_id, kind, slot, payload, now)
    }
}

pub(crate) fn validate_entry(entry: &OutboxEntry) -> Result<(), OutboxError> {
    validate_payload(entry.payload.len())
}

fn validate_payload(len: usize) -> Result<(), OutboxError> {
    if len > MAX_OUTBOX_PAYLOAD_BYTES {
        return Err(OutboxError::PayloadTooLarge {
            size: len,
            limit: MAX_OUTBOX_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

pub(crate) fn truncate_error(mut s: String) -> String {
    if s.chars().count() > MAX_OUTBOX_ERROR_CHARS {
        s = s.chars().take(MAX_OUTBOX_ERROR_CHARS).collect();
    }
    s
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OutboxError {
    #[error("outbox payload was {size} bytes, over the {limit}-byte limit (§15)")]
    PayloadTooLarge { size: usize, limit: usize },
    #[error("outbox operation {0:?} not found")]
    NotFound(OperationId),
    #[error("outbox operation {id:?} is {from:?}; cannot move it to {to:?}")]
    InvalidTransition {
        id: OperationId,
        from: OutboxState,
        to: OutboxState,
    },
    #[error("outbox backend error: {0}")]
    Backend(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Inserted,
    /// An entry with this [`OperationId`] already exists (in any
    /// state). Nothing changed — §24-style idempotency, applied to
    /// work instead of events.
    AlreadyPresent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutboxCounts {
    pub pending: u64,
    pub in_flight: u64,
    pub done: u64,
    pub dead_lettered: u64,
}

/// The queue discipline, with no IO and no locking. See the module doc.
#[derive(Debug, Default)]
pub struct OutboxTable {
    entries: HashMap<OperationId, OutboxEntry>,
}

impl OutboxTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, entry: OutboxEntry) -> Result<EnqueueOutcome, OutboxError> {
        validate_payload(entry.payload.len())?;
        if self.entries.contains_key(&entry.operation_id) {
            return Ok(EnqueueOutcome::AlreadyPresent);
        }
        self.entries.insert(entry.operation_id, entry);
        Ok(EnqueueOutcome::Inserted)
    }

    pub fn claim_due(&mut self, now: Timestamp, lease_ms: u64, limit: usize) -> Vec<OutboxEntry> {
        let mut due: Vec<(Timestamp, Timestamp, OperationId)> = self
            .entries
            .values()
            .filter(|e| match e.state {
                OutboxState::Pending => e.not_before <= now,
                // An expired lease means the previous worker is gone.
                OutboxState::InFlight => e.lease_until.is_none_or(|l| l <= now),
                _ => false,
            })
            .map(|e| (e.not_before, e.created_at, e.operation_id))
            .collect();
        due.sort_unstable();
        due.truncate(limit);

        let lease_until = Timestamp(now.0.saturating_add(lease_ms));
        let mut claimed = Vec::with_capacity(due.len());
        for (_, _, id) in due {
            if let Some(e) = self.entries.get_mut(&id) {
                e.state = OutboxState::InFlight;
                e.attempts = e.attempts.saturating_add(1);
                e.lease_until = Some(lease_until);
                e.updated_at = now;
                claimed.push(e.clone());
            }
        }
        claimed
    }

    /// `Pending`/`InFlight` → `Done`; already-`Done` is an idempotent
    /// success. A `DeadLettered` entry cannot be completed.
    pub fn complete(&mut self, id: OperationId, now: Timestamp) -> Result<(), OutboxError> {
        let e = self.entries.get_mut(&id).ok_or(OutboxError::NotFound(id))?;
        match e.state {
            OutboxState::Done => Ok(()),
            OutboxState::Pending | OutboxState::InFlight => {
                e.state = OutboxState::Done;
                e.lease_until = None;
                e.updated_at = now;
                Ok(())
            }
            from @ OutboxState::DeadLettered => Err(OutboxError::InvalidTransition {
                id,
                from,
                to: OutboxState::Done,
            }),
        }
    }

    /// `InFlight` → `Pending`, due again at `not_before`.
    pub fn reschedule(
        &mut self,
        id: OperationId,
        not_before: Timestamp,
        error: Option<String>,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        let e = self.entries.get_mut(&id).ok_or(OutboxError::NotFound(id))?;
        if e.state != OutboxState::InFlight {
            return Err(OutboxError::InvalidTransition {
                id,
                from: e.state,
                to: OutboxState::Pending,
            });
        }
        e.state = OutboxState::Pending;
        e.not_before = not_before;
        e.lease_until = None;
        e.last_error = error.map(truncate_error);
        e.updated_at = now;
        Ok(())
    }

    /// `Pending`/`InFlight` → `DeadLettered`; already-dead is an
    /// idempotent success.
    pub fn dead_letter(
        &mut self,
        id: OperationId,
        reason: String,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        let e = self.entries.get_mut(&id).ok_or(OutboxError::NotFound(id))?;
        match e.state {
            OutboxState::DeadLettered => Ok(()),
            OutboxState::Pending | OutboxState::InFlight => {
                e.state = OutboxState::DeadLettered;
                e.lease_until = None;
                e.last_error = Some(truncate_error(reason));
                e.updated_at = now;
                Ok(())
            }
            from @ OutboxState::Done => Err(OutboxError::InvalidTransition {
                id,
                from,
                to: OutboxState::DeadLettered,
            }),
        }
    }

    pub fn get(&self, id: OperationId) -> Option<OutboxEntry> {
        self.entries.get(&id).cloned()
    }

    /// Entries still owed work, oldest first.
    pub fn list_open(&self, limit: usize) -> Vec<OutboxEntry> {
        let mut open: Vec<&OutboxEntry> = self
            .entries
            .values()
            .filter(|e| e.state.is_open())
            .collect();
        open.sort_unstable_by_key(|e| (e.created_at, e.operation_id));
        open.into_iter().take(limit).cloned().collect()
    }

    pub fn release_all_in_flight(&mut self, now: Timestamp) -> u64 {
        let mut released = 0;
        for e in self.entries.values_mut() {
            if e.state == OutboxState::InFlight {
                e.state = OutboxState::Pending;
                e.lease_until = None;
                e.updated_at = now;
                released += 1;
            }
        }
        released
    }

    pub fn purge_done(&mut self, before: Timestamp) -> u64 {
        let start = self.entries.len();
        self.entries
            .retain(|_, e| !(e.state == OutboxState::Done && e.updated_at < before));
        (start - self.entries.len()) as u64
    }

    pub fn counts(&self) -> OutboxCounts {
        let mut c = OutboxCounts::default();
        for e in self.entries.values() {
            match e.state {
                OutboxState::Pending => c.pending += 1,
                OutboxState::InFlight => c.in_flight += 1,
                OutboxState::Done => c.done += 1,
                OutboxState::DeadLettered => c.dead_lettered += 1,
            }
        }
        c
    }
}

/// §14/§15's pending-work queue. See the module doc for why this uses
/// `-> impl Future + Send` rather than `async-trait`.
pub trait Outbox: Send + Sync {
    /// Idempotent on [`OperationId`].
    fn enqueue(
        &self,
        entry: OutboxEntry,
    ) -> impl Future<Output = Result<EnqueueOutcome, OutboxError>> + Send;

    /// Claims up to `limit` due entries (see [`OutboxTable::claim_due`]).
    fn claim_due(
        &self,
        now: Timestamp,
        lease_ms: u64,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<OutboxEntry>, OutboxError>> + Send;

    fn complete(
        &self,
        id: OperationId,
        now: Timestamp,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    fn reschedule(
        &self,
        id: OperationId,
        not_before: Timestamp,
        error: Option<String>,
        now: Timestamp,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    fn dead_letter(
        &self,
        id: OperationId,
        reason: String,
        now: Timestamp,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    fn get(
        &self,
        id: OperationId,
    ) -> impl Future<Output = Result<Option<OutboxEntry>, OutboxError>> + Send;

    fn list_open(
        &self,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<OutboxEntry>, OutboxError>> + Send;

    /// §47 startup: no worker from a previous run is alive, so return
    /// every `InFlight` entry to `Pending` immediately.
    fn release_all_in_flight(
        &self,
        now: Timestamp,
    ) -> impl Future<Output = Result<u64, OutboxError>> + Send;

    /// Retention for finished work: drops `Done` entries last touched
    /// before `before`. `DeadLettered` entries are kept for diagnostics.
    fn purge_done(
        &self,
        before: Timestamp,
    ) -> impl Future<Output = Result<u64, OutboxError>> + Send;

    fn counts(&self) -> impl Future<Output = Result<OutboxCounts, OutboxError>> + Send;
}

/// A standalone in-memory [`Outbox`]. For an outbox that must commit
/// *atomically with events*, use
/// [`crate::memory_store::InMemoryEventStore::outbox`] instead.
#[derive(Debug, Default)]
pub struct InMemoryOutbox {
    table: Mutex<OutboxTable>,
}

impl InMemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OutboxTable> {
        self.table.lock().expect("InMemoryOutbox lock poisoned")
    }
}

impl Outbox for InMemoryOutbox {
    async fn enqueue(&self, entry: OutboxEntry) -> Result<EnqueueOutcome, OutboxError> {
        self.lock().insert(entry)
    }
    async fn claim_due(
        &self,
        now: Timestamp,
        lease_ms: u64,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, OutboxError> {
        Ok(self.lock().claim_due(now, lease_ms, limit))
    }
    async fn complete(&self, id: OperationId, now: Timestamp) -> Result<(), OutboxError> {
        self.lock().complete(id, now)
    }
    async fn reschedule(
        &self,
        id: OperationId,
        not_before: Timestamp,
        error: Option<String>,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        self.lock().reschedule(id, not_before, error, now)
    }
    async fn dead_letter(
        &self,
        id: OperationId,
        reason: String,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        self.lock().dead_letter(id, reason, now)
    }
    async fn get(&self, id: OperationId) -> Result<Option<OutboxEntry>, OutboxError> {
        Ok(self.lock().get(id))
    }
    async fn list_open(&self, limit: usize) -> Result<Vec<OutboxEntry>, OutboxError> {
        Ok(self.lock().list_open(limit))
    }
    async fn release_all_in_flight(&self, now: Timestamp) -> Result<u64, OutboxError> {
        Ok(self.lock().release_all_in_flight(now))
    }
    async fn purge_done(&self, before: Timestamp) -> Result<u64, OutboxError> {
        Ok(self.lock().purge_done(before))
    }
    async fn counts(&self) -> Result<OutboxCounts, OutboxError> {
        Ok(self.lock().counts())
    }
}

/// §14 says one transaction may commit `MessageQueued`, the
/// `OutboxOperation`, the message projection and the conversation
/// summary together. This is the event-log half of that sentence:
/// events and outbox rows land together or not at all.
///
/// Guarantees, for every implementor:
///
/// - If `append` would fail (§12 conflict, §55 size, §68 storage-full,
///   §69 read-only), **no** outbox row is written.
/// - If the outbox rows are invalid (oversized payload), **no** event
///   is written.
/// - Entries are idempotent on [`OperationId`]: re-submitting the same
///   command (same `EventId`s, same derived ids) after a crash adds
///   nothing new to either structure.
pub trait TransactionalOutbox: EventStore {
    fn append_with_outbox(
        &self,
        request: AppendRequest,
        entries: Vec<OutboxEntry>,
    ) -> impl Future<Output = Result<AppendResult, EventStoreError>> + Send;
}

pub(crate) fn outbox_err_to_store(e: OutboxError) -> EventStoreError {
    EventStoreError::Backend(format!("outbox: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::StreamId;

    fn entry(n: u8, now: u64) -> OutboxEntry {
        let src = EventId::from_uuid(uuid::Uuid::from_bytes([n; 16]));
        OutboxEntry::new(
            src,
            StreamId::from_name("s"),
            EffectKind(1),
            0,
            vec![n],
            Timestamp(now),
        )
        .unwrap()
    }

    #[test]
    fn operation_ids_are_deterministic_and_input_sensitive() {
        let e = EventId::from_uuid(uuid::Uuid::from_bytes([7; 16]));
        let a = OperationId::derive(e, EffectKind(1), 0);
        assert_eq!(a, OperationId::derive(e, EffectKind(1), 0));
        assert_ne!(a, OperationId::derive(e, EffectKind(2), 0));
        assert_ne!(a, OperationId::derive(e, EffectKind(1), 1));
        let other = EventId::from_uuid(uuid::Uuid::from_bytes([8; 16]));
        assert_ne!(a, OperationId::derive(other, EffectKind(1), 0));
    }

    #[test]
    fn operation_id_hex_round_trips_and_rejects_garbage() {
        let id = OperationId::derive(EventId::new(), EffectKind(9), 3);
        assert_eq!(OperationId::from_hex(&id.to_hex()), Some(id));
        assert_eq!(OperationId::from_hex("zz"), None);
        assert_eq!(OperationId::from_hex(&"g".repeat(32)), None);
    }

    #[test]
    fn oversized_payload_is_rejected_at_construction() {
        let r = OutboxEntry::new(
            EventId::new(),
            StreamId::from_name("s"),
            EffectKind(1),
            0,
            vec![0; MAX_OUTBOX_PAYLOAD_BYTES + 1],
            Timestamp(0),
        );
        assert!(matches!(r, Err(OutboxError::PayloadTooLarge { .. })));
    }

    #[test]
    fn enqueue_is_idempotent_on_operation_id() {
        let mut t = OutboxTable::new();
        assert_eq!(t.insert(entry(1, 0)).unwrap(), EnqueueOutcome::Inserted);
        assert_eq!(
            t.insert(entry(1, 99)).unwrap(),
            EnqueueOutcome::AlreadyPresent
        );
        assert_eq!(t.counts().pending, 1);
    }

    #[test]
    fn claim_respects_not_before_limit_and_order() {
        let mut t = OutboxTable::new();
        t.insert(entry(1, 10)).unwrap();
        t.insert(entry(2, 5)).unwrap();
        t.insert(entry(3, 100)).unwrap();
        // Not yet due: nothing.
        assert!(t.claim_due(Timestamp(4), 1000, 10).is_empty());
        // Due ones come back oldest-first and respect `limit`.
        let got = t.claim_due(Timestamp(50), 1000, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload, vec![2]);
        assert_eq!(got[0].attempts, 1);
        assert_eq!(got[0].state, OutboxState::InFlight);
        let got = t.claim_due(Timestamp(50), 1000, 10);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].payload, vec![1]);
    }

    #[test]
    fn an_expired_lease_makes_in_flight_work_claimable_again() {
        let mut t = OutboxTable::new();
        t.insert(entry(1, 0)).unwrap();
        assert_eq!(t.claim_due(Timestamp(0), 100, 10).len(), 1);
        // Lease still live.
        assert!(t.claim_due(Timestamp(99), 100, 10).is_empty());
        // Lease expired: the "crashed worker" case, no special pass.
        let again = t.claim_due(Timestamp(100), 100, 10);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].attempts, 2);
    }

    #[test]
    fn complete_is_idempotent_and_dead_letter_is_terminal() {
        let mut t = OutboxTable::new();
        let e = entry(1, 0);
        let id = e.operation_id;
        t.insert(e).unwrap();
        t.complete(id, Timestamp(1)).unwrap();
        t.complete(id, Timestamp(2)).unwrap();
        assert!(matches!(
            t.dead_letter(id, "x".into(), Timestamp(3)),
            Err(OutboxError::InvalidTransition { .. })
        ));

        let e = entry(2, 0);
        let id = e.operation_id;
        t.insert(e).unwrap();
        t.dead_letter(id, "gone".into(), Timestamp(1)).unwrap();
        assert!(matches!(
            t.complete(id, Timestamp(2)),
            Err(OutboxError::InvalidTransition { .. })
        ));
        assert!(t.claim_due(Timestamp(10), 10, 10).is_empty());
    }

    #[test]
    fn reschedule_requires_a_claim_and_truncates_the_error() {
        let mut t = OutboxTable::new();
        let e = entry(1, 0);
        let id = e.operation_id;
        t.insert(e).unwrap();
        assert!(matches!(
            t.reschedule(id, Timestamp(5), None, Timestamp(0)),
            Err(OutboxError::InvalidTransition { .. })
        ));
        t.claim_due(Timestamp(0), 100, 1);
        t.reschedule(id, Timestamp(500), Some("e".repeat(10_000)), Timestamp(1))
            .unwrap();
        let got = t.get(id).unwrap();
        assert_eq!(got.state, OutboxState::Pending);
        assert_eq!(got.not_before, Timestamp(500));
        assert_eq!(
            got.last_error.unwrap().chars().count(),
            MAX_OUTBOX_ERROR_CHARS
        );
        assert!(t.claim_due(Timestamp(499), 10, 10).is_empty());
        assert_eq!(t.claim_due(Timestamp(500), 10, 10).len(), 1);
    }

    #[test]
    fn unknown_ids_are_not_found() {
        let mut t = OutboxTable::new();
        let id = OperationId::from_bytes([0; 16]);
        assert_eq!(t.complete(id, Timestamp(0)), Err(OutboxError::NotFound(id)));
    }

    #[test]
    fn release_all_in_flight_and_purge_done() {
        let mut t = OutboxTable::new();
        for n in 1..=3 {
            t.insert(entry(n, 0)).unwrap();
        }
        let claimed = t.claim_due(Timestamp(0), 10_000, 2);
        assert_eq!(t.counts().in_flight, 2);
        assert_eq!(t.release_all_in_flight(Timestamp(1)), 2);
        assert_eq!(t.counts().in_flight, 0);
        assert_eq!(t.counts().pending, 3);

        t.complete(claimed[0].operation_id, Timestamp(10)).unwrap();
        assert_eq!(t.purge_done(Timestamp(10)), 0, "strictly-before cutoff");
        assert_eq!(t.purge_done(Timestamp(11)), 1);
        assert_eq!(t.counts().done, 0);
        // Open work is never purged.
        assert_eq!(t.list_open(10).len(), 2);
    }
}
