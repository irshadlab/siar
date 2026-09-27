//! §27 "Transfer Record".
//!
//! §27's own code block is field-for-field: `transfer_id`, `blob_id`,
//! `direction`, `peer`, `state`, `created_at`, `updated_at`. Two
//! deliberate departures from a literal transcription, both named:
//! `direction` isn't in the spec's own snippet at all (implied by the
//! surrounding text — a transfer is either incoming or outgoing — so
//! [`TransferDirection`] is this crate's own reasonable reading, same
//! posture §26's own doc comment already takes toward "shown
//! conceptually rather than as an exhaustive table"); and
//! `created_at`/`updated_at` are `u64` millisecond timestamps, matching
//! [`crate::descriptor::FileMetadata::created_at_millis`]'s own
//! convention, rather than a `Timestamp` type this crate has never
//! needed and doesn't want a new dependency for.

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;

use crate::ids::{BlobId, TransferId};
use crate::transfer_state::{InvalidTransition, TransferEvent, TransferState};

/// Not named in §27's own snippet — implied by the surrounding text
/// ("incoming"/"outgoing" transfers). See this module's own top doc
/// comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    Incoming,
    Outgoing,
}

/// §27, field-for-field (plus `direction` — see this module's own top
/// doc comment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferRecord {
    pub transfer_id: TransferId,
    pub blob_id: BlobId,
    pub direction: TransferDirection,
    pub peer: DeviceId,
    pub state: TransferState,
    pub created_at_millis: u64,
    pub updated_at_millis: u64,
}

impl TransferRecord {
    /// A brand-new record always starts `Offered` — the same starting
    /// point [`TransferState::transition`]'s own table treats as the
    /// only real entry point, and `created_at`/`updated_at` are the
    /// same instant, since nothing has happened to this transfer yet.
    pub fn new(
        transfer_id: TransferId,
        blob_id: BlobId,
        direction: TransferDirection,
        peer: DeviceId,
        now_millis: u64,
    ) -> Self {
        Self {
            transfer_id,
            blob_id,
            direction,
            peer,
            state: TransferState::Offered,
            created_at_millis: now_millis,
            updated_at_millis: now_millis,
        }
    }

    /// Advances `self.state` via [`TransferState::transition`] and
    /// bumps `updated_at_millis` — the one place a `TransferRecord`'s
    /// own state and its own "when did this last change" timestamp are
    /// kept in sync, rather than trusting every call site to update
    /// both fields itself. Leaves the record untouched on an illegal
    /// transition (no partial update), matching
    /// [`crate::transfer_state::decide`]'s own "check before mutating
    /// anything" discipline.
    pub fn advance(
        &mut self,
        event: TransferEvent,
        now_millis: u64,
    ) -> Result<(), InvalidTransition> {
        let next = self.state.transition(event)?;
        self.state = next;
        self.updated_at_millis = now_millis;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_blob_id() -> BlobId {
        BlobId::from_ciphertext(b"whatever ciphertext")
    }

    #[test]
    fn a_new_record_starts_offered_with_matching_created_and_updated_timestamps() {
        let record = TransferRecord::new(
            TransferId::new(),
            sample_blob_id(),
            TransferDirection::Outgoing,
            DeviceId::new(),
            1_000,
        );
        assert_eq!(record.state, TransferState::Offered);
        assert_eq!(record.created_at_millis, 1_000);
        assert_eq!(record.updated_at_millis, 1_000);
    }

    #[test]
    fn advancing_updates_state_and_bumps_updated_at_but_not_created_at() {
        let mut record = TransferRecord::new(
            TransferId::new(),
            sample_blob_id(),
            TransferDirection::Incoming,
            DeviceId::new(),
            1_000,
        );
        record.advance(TransferEvent::Accept, 2_000).unwrap();
        assert_eq!(record.state, TransferState::Accepted);
        assert_eq!(record.created_at_millis, 1_000);
        assert_eq!(record.updated_at_millis, 2_000);
    }

    #[test]
    fn an_illegal_transition_leaves_the_record_completely_unchanged() {
        let mut record = TransferRecord::new(
            TransferId::new(),
            sample_blob_id(),
            TransferDirection::Incoming,
            DeviceId::new(),
            1_000,
        );
        let before = record.clone();
        let result = record.advance(TransferEvent::Start, 2_000); // Offered can't Start
        assert!(result.is_err());
        assert_eq!(record, before);
    }
}
