//! The one physical-removal primitive the append-only log offers —
//! the mechanism behind §40 "Compaction" and §41 "Privacy and
//! Deletion", and the reason §10's "append-only" is still true.
//!
//! §10 says committed events "should not be modified in ordinary
//! operation" and that "storage compaction and legal/privacy deletion
//! are separate retention operations." [`RedactableStore::redact_payloads`]
//! is that separate operation, and it is deliberately narrow: it
//! **empties the payload and nothing else**.
//!
//! ## Why redact instead of delete the row
//!
//! Everything else about the event is load-bearing for other
//! guarantees, so it stays:
//!
//! - `event_id` — §24 idempotency. A deleted row would let a late
//!   duplicate of the same event be accepted as new, resurrecting
//!   the very data that was removed.
//! - `stream_version` / `local_offset` — §12 optimistic concurrency,
//!   §53 sync cursors, §26 gap detection and every projection
//!   checkpoint count in versions and offsets. Removing a row punches a
//!   hole in both and every reader would then see "loss".
//! - type, timestamps, origin, correlation/causation — the *shape* of
//!   history (that something happened, when, caused by what) is usually
//!   what retention policy keeps while dropping content.
//!
//! The payload is the sensitive or bulky part. After redaction the
//! stored event is a tombstone: same slot, no content,
//! [`RedactableStore::is_redacted`] true. The §22 integrity checksum is
//! recomputed over the emptied row, so a redacted event still verifies;
//! only *tampering* fails it.
//!
//! ## What redaction does not reach
//!
//! Derived copies. Projections, snapshots ([`crate::snapshot`]) and the
//! outbox ([`crate::outbox`]) can all hold data derived from a payload.
//! [`crate::deletion::execute_deletion`] is the orchestration that
//! covers the ones this crate owns and reports the rest.
//!
//! Redaction is idempotent and irreversible.

use std::future::Future;

use crate::ids::EventId;
use crate::store::{EventStore, EventStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RedactionReport {
    /// Payloads emptied by this call.
    pub redacted: u32,
    /// Already tombstones; untouched.
    pub already_redacted: u32,
    /// No such event.
    pub not_found: u32,
}

pub trait RedactableStore: EventStore {
    /// Empties the payload of each listed event. All-or-nothing per
    /// call (one transaction on durable backends).
    fn redact_payloads(
        &self,
        event_ids: &[EventId],
    ) -> impl Future<Output = Result<RedactionReport, EventStoreError>> + Send;

    fn is_redacted(
        &self,
        event_id: EventId,
    ) -> impl Future<Output = Result<bool, EventStoreError>> + Send;
}
