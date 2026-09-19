//! `04-offline-event-log-architecture.md` §34 "File Events", §92 Phase
//! 3 ("messaging, files, identity integration") — the third and last
//! of Phase 3's three named domains, same shape as
//! `siar_identity_multidevice::audit_log` (§35) and
//! `siar_messaging::events` (§33): `EventTypeId` constants this crate
//! owns, a typed payload enum (postcard-serialized), an
//! `into_new_event` constructor per event, a `decode` counterpart, and
//! the same "construct only, never append" discipline both of those
//! modules already state outright — this crate's own top doc comment
//! already says as much for the crate as a whole ("No local store...
//! persisting them is a `siar-storage` integration this crate doesn't
//! depend on"), so a module that called `EventStore::append` itself
//! would be a new, undocumented dependency this crate doesn't
//! otherwise have.
//!
//! ## Streams
//!
//! One stream per transfer ([`transfer_stream_id`]), keyed by the new
//! [`crate::ids::TransferId`] this module needed and
//! [`crate::transfer_state::TransferState`] didn't have — see that
//! type's own doc comment for why a transfer isn't identifiable by
//! [`crate::ids::ManifestId`] alone. Every payload also carries
//! `transfer_id` even though it's implied by the stream, same
//! denormalize-for-a-standalone-reader reasoning
//! `siar_messaging::events`'s own doc comment already gives for
//! `conversation_id`.
//!
//! ## §26 state machine, §34 events: related, not merged
//!
//! [`FileEvent`]'s nine variants line up one-for-one with
//! [`crate::transfer_state::TransferState`]'s reachable states plus
//! [`crate::verify::verify_complete_blob`]'s own outcome
//! (`BlobVerified` isn't a state — verification can happen either
//! before or after `Completed`, and a transfer's state machine says
//! nothing about whether its bytes were ever checked). This module
//! does not drive or call `TransferState::transition` itself: exactly
//! the same "decide vs record" split
//! `siar_identity_multidevice::audit_log`'s own doc comment names for
//! that crate (device linking decides nothing here; recording it is
//! all this does) — a real caller wiring §26's transitions to a
//! running transfer would call `TransferState::transition` for the
//! decision and one of this module's constructors, separately, to
//! record it.
//!
//! ## Numeric range
//!
//! 200-208 — the third block in the informal per-domain convention
//! `siar_messaging::events`'s own doc comment already names as a real,
//! unenforced gap (identity: 1-8, messaging: 100-108, this: 200-208).
//! Nothing here fixes that gap; it just doesn't make it worse by
//! reusing either earlier range.

use crate::ids::{BlobId, LogicalAttachmentId, ManifestId, TransferId};
use serde::{Deserialize, Serialize};
use siar_event_log::envelope::EventOrigin;
use siar_event_log::ids::{EventId, EventTypeId, StreamId, Timestamp};
use siar_event_log::store::NewEvent;

/// One stream per transfer — see this module's own doc comment.
pub fn transfer_stream_id(transfer_id: TransferId) -> StreamId {
    StreamId::from_name(&format!("transfer:{transfer_id}"))
}

pub const EVENT_TYPE_TRANSFER_CREATED: EventTypeId = EventTypeId(200);
pub const EVENT_TYPE_TRANSFER_ACCEPTED: EventTypeId = EventTypeId(201);
pub const EVENT_TYPE_TRANSFER_STARTED: EventTypeId = EventTypeId(202);
pub const EVENT_TYPE_TRANSFER_PAUSED: EventTypeId = EventTypeId(203);
pub const EVENT_TYPE_TRANSFER_RESUMED: EventTypeId = EventTypeId(204);
pub const EVENT_TYPE_TRANSFER_COMPLETED: EventTypeId = EventTypeId(205);
pub const EVENT_TYPE_TRANSFER_CANCELLED: EventTypeId = EventTypeId(206);
pub const EVENT_TYPE_TRANSFER_FAILED: EventTypeId = EventTypeId(207);
pub const EVENT_TYPE_BLOB_VERIFIED: EventTypeId = EventTypeId(208);

/// §34's nine event names. No per-event timestamp field — the
/// envelope's own `created_at` ([`NewEvent::created_at`], set by
/// [`Self::into_new_event`]) already carries that, same choice
/// `siar_messaging::events::MessagingEvent` and
/// `audit_log::IdentityAuditPayload` both already make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileEvent {
    /// A manifest exists and has been offered as a transfer — §26's
    /// `TransferState::Offered`.
    TransferCreated {
        transfer_id: TransferId,
        manifest_id: ManifestId,
        logical_attachment_id: LogicalAttachmentId,
        blob_id: BlobId,
        total_size_bytes: u64,
        chunk_count: u32,
    },
    /// §26 `TransferEvent::Accept` succeeded.
    TransferAccepted { transfer_id: TransferId },
    /// §26 `TransferEvent::Start` succeeded.
    TransferStarted { transfer_id: TransferId },
    /// §26 `TransferEvent::Pause` succeeded.
    TransferPaused { transfer_id: TransferId },
    /// §26 `TransferEvent::Resume` succeeded.
    TransferResumed { transfer_id: TransferId },
    /// §26 `TransferEvent::ChunksComplete` succeeded — all chunks
    /// received, not yet necessarily verified (see [`Self::BlobVerified`]).
    TransferCompleted { transfer_id: TransferId },
    /// §26 `TransferEvent::Cancel` succeeded, from whichever state
    /// allowed it.
    TransferCancelled { transfer_id: TransferId },
    /// §26 `TransferEvent::Fail` succeeded.
    TransferFailed {
        transfer_id: TransferId,
        reason: String,
    },
    /// [`crate::verify::verify_complete_blob`] passed for this
    /// transfer's blob — a verification outcome, not a state
    /// transition (see this module's own doc comment).
    BlobVerified {
        transfer_id: TransferId,
        blob_id: BlobId,
    },
}

impl FileEvent {
    pub fn event_type(&self) -> EventTypeId {
        match self {
            Self::TransferCreated { .. } => EVENT_TYPE_TRANSFER_CREATED,
            Self::TransferAccepted { .. } => EVENT_TYPE_TRANSFER_ACCEPTED,
            Self::TransferStarted { .. } => EVENT_TYPE_TRANSFER_STARTED,
            Self::TransferPaused { .. } => EVENT_TYPE_TRANSFER_PAUSED,
            Self::TransferResumed { .. } => EVENT_TYPE_TRANSFER_RESUMED,
            Self::TransferCompleted { .. } => EVENT_TYPE_TRANSFER_COMPLETED,
            Self::TransferCancelled { .. } => EVENT_TYPE_TRANSFER_CANCELLED,
            Self::TransferFailed { .. } => EVENT_TYPE_TRANSFER_FAILED,
            Self::BlobVerified { .. } => EVENT_TYPE_BLOB_VERIFIED,
        }
    }

    pub fn transfer_id(&self) -> TransferId {
        match self {
            Self::TransferCreated { transfer_id, .. }
            | Self::TransferAccepted { transfer_id, .. }
            | Self::TransferStarted { transfer_id, .. }
            | Self::TransferPaused { transfer_id, .. }
            | Self::TransferResumed { transfer_id, .. }
            | Self::TransferCompleted { transfer_id, .. }
            | Self::TransferCancelled { transfer_id, .. }
            | Self::TransferFailed { transfer_id, .. }
            | Self::BlobVerified { transfer_id, .. } => *transfer_id,
        }
    }

    /// Thin wrapper over [`transfer_stream_id`] — see
    /// `siar_messaging::events::MessagingEvent::stream_id`'s own doc
    /// comment for why this exists (so a caller building an
    /// [`siar_event_log::store::AppendRequest`] doesn't have to
    /// destructure the payload itself).
    pub fn stream_id(&self) -> StreamId {
        transfer_stream_id(self.transfer_id())
    }

    /// Same reasoning `siar_messaging::events::MessagingEvent::
    /// into_new_event`'s own doc comment gives for taking `origin`
    /// explicitly rather than inferring it: a transfer can be offered
    /// by the local device (sending a file out) or by a remote one
    /// (receiving an offer), and nothing in a bare `FileEvent` payload
    /// says which — the caller, who dispatched or received the
    /// underlying protocol message, is the only one who knows.
    pub fn into_new_event(self, origin: EventOrigin) -> NewEvent {
        let event_type = self.event_type();
        let payload = postcard::to_allocvec(&self).expect("FileEvent always postcard-serializes");
        NewEvent {
            event_id: EventId::new(),
            event_type,
            schema_version: 1,
            created_at: Timestamp::now(),
            origin,
            correlation_id: None,
            causation_id: None,
            payload,
        }
    }
}

/// Read-side counterpart to [`FileEvent::into_new_event`] — same role
/// `siar_messaging::events::decode_messaging_event`/`audit_log::
/// decode_audit_payload` play for their own domains.
pub fn decode_file_event(payload: &[u8]) -> Result<FileEvent, postcard::Error> {
    postcard::from_bytes(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::DeviceId;

    fn sample_created(transfer_id: TransferId) -> FileEvent {
        FileEvent::TransferCreated {
            transfer_id,
            manifest_id: ManifestId::new(),
            logical_attachment_id: LogicalAttachmentId::new(),
            blob_id: BlobId::from_ciphertext(b"some ciphertext"),
            total_size_bytes: 4096,
            chunk_count: 4,
        }
    }

    #[test]
    fn same_transfer_always_derives_the_same_stream_id() {
        let transfer_id = TransferId::new();
        assert_eq!(
            transfer_stream_id(transfer_id),
            transfer_stream_id(transfer_id)
        );
    }

    #[test]
    fn different_transfers_derive_different_stream_ids() {
        assert_ne!(
            transfer_stream_id(TransferId::new()),
            transfer_stream_id(TransferId::new())
        );
    }

    #[test]
    fn transfer_created_round_trips_through_postcard_and_carries_its_own_stream() {
        let transfer_id = TransferId::new();
        let event = sample_created(transfer_id);
        assert_eq!(event.stream_id(), transfer_stream_id(transfer_id));

        let device = DeviceId::new();
        let new_event = event
            .clone()
            .into_new_event(EventOrigin::LocalDevice(device));
        assert_eq!(new_event.event_type, EVENT_TYPE_TRANSFER_CREATED);
        assert_eq!(new_event.origin, EventOrigin::LocalDevice(device));

        let decoded = decode_file_event(&new_event.payload).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn transfer_created_from_a_remote_offer_uses_remote_device_origin() {
        let transfer_id = TransferId::new();
        let event = sample_created(transfer_id);
        let remote = DeviceId::new();
        let new_event = event.into_new_event(EventOrigin::RemoteDevice(remote));
        assert_eq!(new_event.origin, EventOrigin::RemoteDevice(remote));
    }

    #[test]
    fn each_event_type_gets_a_distinct_tag_and_does_not_collide_with_other_domains() {
        let tags = [
            EVENT_TYPE_TRANSFER_CREATED,
            EVENT_TYPE_TRANSFER_ACCEPTED,
            EVENT_TYPE_TRANSFER_STARTED,
            EVENT_TYPE_TRANSFER_PAUSED,
            EVENT_TYPE_TRANSFER_RESUMED,
            EVENT_TYPE_TRANSFER_COMPLETED,
            EVENT_TYPE_TRANSFER_CANCELLED,
            EVENT_TYPE_TRANSFER_FAILED,
            EVENT_TYPE_BLOB_VERIFIED,
        ];
        for (i, a) in tags.iter().enumerate() {
            for b in &tags[i + 1..] {
                assert_ne!(a, b);
            }
            // identity: 1-8, messaging: 100-108 — see this module's
            // own doc comment.
            assert!(a.0 >= 200);
        }
    }

    #[test]
    fn every_variant_round_trips_and_reports_its_own_transfer_id() {
        let transfer_id = TransferId::new();
        let blob_id = BlobId::from_ciphertext(b"other ciphertext");
        let events = vec![
            FileEvent::TransferAccepted { transfer_id },
            FileEvent::TransferStarted { transfer_id },
            FileEvent::TransferPaused { transfer_id },
            FileEvent::TransferResumed { transfer_id },
            FileEvent::TransferCompleted { transfer_id },
            FileEvent::TransferCancelled { transfer_id },
            FileEvent::TransferFailed {
                transfer_id,
                reason: "peer disconnected".to_string(),
            },
            FileEvent::BlobVerified {
                transfer_id,
                blob_id,
            },
        ];
        for event in events {
            assert_eq!(event.transfer_id(), transfer_id);
            let new_event = event
                .clone()
                .into_new_event(EventOrigin::LocalDevice(DeviceId::new()));
            let decoded = decode_file_event(&new_event.payload).unwrap();
            assert_eq!(decoded, event);
        }
    }
}
