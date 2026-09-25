//! §11 "Atomic Append", §12 "Optimistic Concurrency", §20 "Event Store
//! Trait", §21 "Batch Append", §24 "Idempotency", §55 "Event Size
//! Limits".

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::envelope::{EventEnvelope, EventOrigin};
use crate::ids::{CorrelationId, EventId, EventTypeId, LocalLogOffset, StreamId, Timestamp};

/// One event a caller wants appended — everything [`EventEnvelope`]
/// needs except `stream_id`/`stream_version` (assigned by
/// [`EventStore::append`] atomically as part of the transaction, §11)
/// and `event_id`, which the caller *does* supply (§28: offline ID
/// generation is the caller's job, not the store's — this is also
/// exactly the field [`EventStore::append`] uses for §24 idempotency).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub event_id: EventId,
    pub event_type: EventTypeId,
    pub schema_version: u16,
    pub created_at: Timestamp,
    pub origin: EventOrigin,
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<EventId>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredEvent {
    pub envelope: EventEnvelope,
    pub local_offset: LocalLogOffset,
}

/// §21: a batch, one transaction. §12: `expected_version` is the
/// optimistic-concurrency guard — `None` means "this stream must not
/// already exist" (version 0), matching how [`crate::memory_store::InMemoryEventStore`]
/// (see `memory_store.rs`) and any real backend would treat a brand
/// new stream's implicit starting version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendRequest {
    pub stream_id: StreamId,
    pub expected_version: u64,
    pub events: Vec<NewEvent>,
}

/// One slot per input event, in order — `Some` for an event that was
/// actually appended (with the offset it landed at), `None` for one
/// skipped as a §24 idempotent duplicate (an `EventId` already seen).
/// A caller that wants "did anything new happen" can just check
/// whether any entry is `Some`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendResult {
    pub stream_id: StreamId,
    pub new_version: u64,
    pub local_offsets: Vec<Option<LocalLogOffset>>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EventStoreError {
    #[error("stream {stream_id:?} expected version {expected_version}, found {actual_version} — concurrent writer")]
    ConcurrencyConflict {
        stream_id: StreamId,
        expected_version: u64,
        actual_version: u64,
    },
    #[error("stream {0:?} not found")]
    StreamNotFound(StreamId),
    /// §67 names this variant for exactly [`crate::stoolap_store`]'s
    /// reason to exist: a real backend can fail underneath the trait
    /// (disk, the embedded SQL engine itself) in ways an in-memory
    /// store never can. Carries the backend's own error text rather
    /// than trying to re-derive a closed taxonomy of stoolap failure
    /// modes this crate doesn't own.
    #[error("event store backend error: {0}")]
    Backend(String),
    /// §67's `Corrupt`, made real by [`crate::stoolap_store`]'s
    /// on-read checksum verification (§22) — a stored row whose bytes
    /// no longer match the checksum written alongside them at append
    /// time (truncated write, on-disk bit-rot, a hand-edited row).
    #[error("stored event failed integrity verification: {0}")]
    Corrupt(String),
    /// §55 "Event Size Limits": "every event type must have a maximum
    /// size... do not allow huge file / huge recursive object /
    /// unbounded metadata inside a journal event." Rejected before any
    /// write is attempted (see [`validate_payload_size`]) — a caller
    /// hitting this should carry large data as a blob reference
    /// instead (§43 — `siar_blob_manifest::events::FileEvent`'s own
    /// `BlobId`/`ManifestId` fields are exactly that pattern already
    /// in real use), not retry with the same oversized payload.
    #[error(
        "event payload for {event_type:?} was {size} bytes, over the {limit}-byte limit (§55)"
    )]
    PayloadTooLarge {
        event_type: EventTypeId,
        size: usize,
        limit: usize,
    },
    /// §67/§68 "Storage Full Behavior," verbatim: "if storage is full,
    /// do not report 'queued' unless the durable append succeeded."
    /// This variant is what makes that a checkable contract rather
    /// than a guideline — a caller that gets this back has a real,
    /// typed signal to NOT report success on, rather than having to
    /// guess from a generic `Backend(String)`. See
    /// [`crate::memory_store::InMemoryEventStore::with_capacity`] for
    /// a real (in-memory-simulated) source of this error a caller can
    /// test against without an actually full disk, and
    /// [`crate::read_only::ReadOnlyEventStore`] for the recovery path
    /// §69 describes once storage (or anything else) has failed.
    #[error("storage is full — the append did not durably succeed (§68)")]
    StorageFull,
    /// §67/§69 "Read-Only Recovery Mode," verbatim: "if the database is
    /// damaged, read-only mode may allow viewing/export/diagnostics...
    /// without risking additional corruption." Returned by every
    /// `append` call once a store is wrapped in
    /// [`crate::read_only::ReadOnlyEventStore`] — reads keep working
    /// unaffected; see that module's own doc comment.
    #[error(
        "this event store is in read-only recovery mode — no further appends are accepted (§69)"
    )]
    ReadOnly,
}

/// §55's own single, uniform limit — a real, if simple, first cut:
/// the section calls for "every event type must have a maximum size,"
/// which reads as inviting a PER-TYPE registry (some event types
/// legitimately need more headroom than others), not necessarily one
/// number for everything. A per-type registry doesn't exist anywhere
/// in this workspace yet (there's no central place all five domains'
/// `EventTypeId` ranges are even listed together — see `siar_messaging::
/// events`'s own doc comment on that same gap), so building one now
/// would be speculative infrastructure ahead of any caller that has
/// ever needed a different limit for a different event type. One
/// generous, uniform ceiling — comfortably above real message text or
/// a `BlobId` reference, comfortably below "a whole file" — is the
/// honest state of this requirement today: enforced for real, just not
/// yet differentiated per §55's own fuller ask.
pub const DEFAULT_MAX_EVENT_PAYLOAD_BYTES: usize = 256 * 1024;

/// §55, made real — called by every real [`EventStore`] backend (see
/// [`crate::memory_store::InMemoryEventStore::append`]/
/// [`crate::stoolap_store::StoolapEventStore::append`]) before any
/// write is attempted, so a batch containing one oversized event
/// rejects the WHOLE batch (§11's own atomicity — no partial write for
/// the events that would have fit) rather than silently truncating or
/// admitting the rest.
pub fn validate_payload_size(
    event_type: EventTypeId,
    payload_len: usize,
) -> Result<(), EventStoreError> {
    if payload_len > DEFAULT_MAX_EVENT_PAYLOAD_BYTES {
        return Err(EventStoreError::PayloadTooLarge {
            event_type,
            size: payload_len,
            limit: DEFAULT_MAX_EVENT_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

/// §20, verbatim signatures (`async fn` via `async-trait` — already a
/// real workspace dependency, matching `siar-storage`'s own repository
/// traits' reason for needing it: `dyn EventStore` has to be usable as
/// a trait object, which native async-fn-in-traits doesn't support
/// without boxing).
#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError>;

    async fn read_stream(
        &self,
        stream: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;
}
