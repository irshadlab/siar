#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §34's own named gap, closed:
//! `siar_blob_manifest::events::FileEvent` (§34's catalog) and
//! `siar_blob_manifest::transfer_state::TransferState` (§26's state
//! machine) have both existed since Phase 3, but neither one had a
//! real `siar_event_log::EventStore::append` caller anywhere in this
//! workspace — every one of `FileEvent`'s own doc comments says so
//! outright ("no real `EventStore::append` caller anywhere for this
//! domain"), and `ROADMAP.md`'s own §34/§93(#11)/§95 rows repeat it.
//! This crate is that caller — the same shape
//! `siar_messaging::service::MessageService::record_messaging_event`
//! already is for §33's messaging events: a small service that
//! DECIDES (calls [`TransferState::transition`]) and, only once that
//! decision succeeds, RECORDS (builds the matching [`FileEvent`] and
//! calls [`siar_event_log::append_with_retry`]) — exactly the "decide
//! vs record" split `siar_blob_manifest::events`'s own doc comment
//! already names as the intended shape for whichever crate closed
//! this gap.
//!
//! ## Why a new crate, not a method added to `siar-blob-manifest` itself
//!
//! `siar_blob_manifest::events`'s own top doc comment is explicit that
//! the crate deliberately has no dependency on `siar-event-log` for
//! *appending* (only for constructing `NewEvent` values): "a module
//! that called `EventStore::append` itself would be a new,
//! undocumented dependency this crate doesn't otherwise have." Adding
//! that dependency there would reverse a stated design decision rather
//! than build on it. This follows the same shape `siar-event-registry`
//! already used to close §63: a new crate that depends on the domain
//! crate it serves, rather than growing that domain crate's own scope.
//!
//! ## What this closes, and what it doesn't
//!
//! Closes: §34/§95's "no real caller" gap for files, and half of
//! §93's Definition-of-Done item 11 ("file semantic state survives
//! restart") — a real, tested path from a transfer-lifecycle decision
//! to a durably-recordable event now exists. The OTHER half of #11 —
//! some real caller in `apps/*` actually driving a live transfer
//! through [`FileTransferService`], wired to real chunk transfer, not
//! just a test calling it directly — is still not done; nothing in
//! this workspace's uploaded `apps/` crates constructs one yet. This
//! also does not touch §35 (identity) or §36/§37 (DTN/emergency),
//! which have the identical "catalog exists, no real caller" gap
//! under a different domain — this crate's shape is the template for
//! whoever closes those next, not a claim that this closes them too.
//!
//! ## One real mismatch this crate had to resolve, not paper over
//!
//! `siar_blob_manifest::transfer_state::TransferEvent::Fail` is a bare
//! variant with no payload, but `FileEvent::TransferFailed` carries a
//! `reason: String` the transition table has no way to supply. Rather
//! than silently recording an empty/placeholder reason,
//! [`FileTransferService::apply`] refuses `TransferEvent::Fail`
//! outright ([`FileTransferError::FailNeedsReason`]) and
//! [`FileTransferService::fail_transfer`] exists as the one call site
//! that both performs that specific transition and has a real reason
//! to record alongside it.

use siar_blob_manifest::events::FileEvent;
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, ManifestId, TransferId};
use siar_blob_manifest::transfer_state::{InvalidTransition, TransferEvent, TransferState};
use siar_event_log::{append_with_retry, EventOrigin, EventStore};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Same bound `siar_messaging::service`'s own
/// `EVENT_LOG_APPEND_MAX_ATTEMPTS` uses — matched, not re-derived.
const EVENT_LOG_APPEND_MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FileTransferError {
    #[error("transfer {0} already exists")]
    DuplicateTransfer(TransferId),
    #[error("transfer {0} is not known to this service")]
    UnknownTransfer(TransferId),
    #[error(transparent)]
    InvalidTransition(#[from] InvalidTransition),
    /// See this module's own doc comment's last section.
    #[error(
        "TransferEvent::Fail carries no reason — call fail_transfer(reason) instead of apply()"
    )]
    FailNeedsReason,
}

/// §29 "Pure Decision Functions": [`siar_blob_manifest::decide`]'s own
/// error shape mapped onto this one, so [`FileTransferService::apply`]
/// can lean on `?` rather than matching it out by hand — kept as a
/// conversion, not a replacement, so this enum's own variants (and
/// every existing match against them) stay exactly as they were before
/// `decide` existed.
impl From<siar_blob_manifest::DecideError> for FileTransferError {
    fn from(err: siar_blob_manifest::DecideError) -> Self {
        match err {
            siar_blob_manifest::DecideError::InvalidTransition(e) => Self::InvalidTransition(e),
            siar_blob_manifest::DecideError::FailNeedsReason => Self::FailNeedsReason,
        }
    }
}

/// The real caller — see this module's own doc comment.
///
/// `event_log` is optional, same reason `siar_messaging::service::
/// MessageService`'s own `event_log` field is: a caller that hasn't
/// wired one yet still gets correct `TransferState` decisions (the
/// in-memory `states` map is authoritative for those either way); it
/// just doesn't get them durably recorded. Note this means
/// `FileTransferService` on its own is NOT §93's durability guarantee
/// — `states` is process-memory only, with no `siar-storage` backing
/// of its own (unlike `siar_messaging::MessageService`, which persists
/// through `siar-storage` independently of the event log). A real
/// caller that needs transfer state to survive a restart still needs
/// its own persistence for `states`, or to rebuild it by replaying
/// this crate's own recorded `FileEvent`s — this service's own scope
/// is the decide/record wiring, not a second durable store.
pub struct FileTransferService {
    event_log: Option<Arc<dyn EventStore + Send + Sync>>,
    states: Mutex<HashMap<TransferId, TransferState>>,
}

impl Default for FileTransferService {
    fn default() -> Self {
        Self {
            event_log: None,
            states: Mutex::new(HashMap::new()),
        }
    }
}

impl FileTransferService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_log(mut self, event_log: Arc<dyn EventStore + Send + Sync>) -> Self {
        self.event_log = Some(event_log);
        self
    }

    /// The `TransferState` this service currently has for
    /// `transfer_id`, if any.
    pub fn current_state(&self, transfer_id: TransferId) -> Option<TransferState> {
        self.states
            .lock()
            .expect("state lock")
            .get(&transfer_id)
            .copied()
    }

    /// §26's initial state (`TransferState::Offered`) plus §34's
    /// `FileEvent::TransferCreated` — the one transition this service
    /// doesn't get from `TransferState::transition` itself, since
    /// there is no "before" state to transition from.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_transfer(
        &self,
        transfer_id: TransferId,
        manifest_id: ManifestId,
        logical_attachment_id: LogicalAttachmentId,
        blob_id: BlobId,
        total_size_bytes: u64,
        chunk_count: u32,
        origin: EventOrigin,
    ) -> Result<TransferState, FileTransferError> {
        {
            let mut states = self.states.lock().expect("state lock");
            if states.contains_key(&transfer_id) {
                return Err(FileTransferError::DuplicateTransfer(transfer_id));
            }
            states.insert(transfer_id, TransferState::Offered);
        }
        self.record(
            FileEvent::TransferCreated {
                transfer_id,
                manifest_id,
                logical_attachment_id,
                blob_id,
                total_size_bytes,
                chunk_count,
            },
            origin,
        )
        .await;
        Ok(TransferState::Offered)
    }

    /// §26's own transition table, decided first — recorded (§34) only
    /// if the decision succeeds. As of §29 "Pure Decision Functions,"
    /// the decide step itself is [`siar_blob_manifest::decide`], not
    /// duplicated here — this method's own job is now just: load
    /// current state, call that pure function, apply its result, and
    /// record whatever events (zero or more — this domain's `decide`
    /// always returns exactly one for a non-`Fail` command, but the
    /// shape doesn't assume that) it decided actually happened.
    ///
    /// Every `TransferEvent` variant except `Fail` goes through here
    /// — see [`Self::fail_transfer`] for why `Fail` doesn't.
    pub async fn apply(
        &self,
        transfer_id: TransferId,
        event: TransferEvent,
        origin: EventOrigin,
    ) -> Result<TransferState, FileTransferError> {
        let current = {
            let states = self.states.lock().expect("state lock");
            let Some(&current) = states.get(&transfer_id) else {
                return Err(FileTransferError::UnknownTransfer(transfer_id));
            };
            current
        };
        let (next, events) = siar_blob_manifest::decide(current, event, transfer_id)?;
        self.states
            .lock()
            .expect("state lock")
            .insert(transfer_id, next);
        for file_event in events {
            self.record(file_event, origin).await;
        }
        Ok(next)
    }

    /// §26 `TransferEvent::Fail`, with the `reason` the transition
    /// table itself has no field for — see this module's own doc
    /// comment's last section for why this isn't just another arm of
    /// [`Self::apply`].
    pub async fn fail_transfer(
        &self,
        transfer_id: TransferId,
        reason: String,
        origin: EventOrigin,
    ) -> Result<TransferState, FileTransferError> {
        let next = {
            let mut states = self.states.lock().expect("state lock");
            let Some(&current) = states.get(&transfer_id) else {
                return Err(FileTransferError::UnknownTransfer(transfer_id));
            };
            let next = current.transition(TransferEvent::Fail)?;
            states.insert(transfer_id, next);
            next
        };
        self.record(
            FileEvent::TransferFailed {
                transfer_id,
                reason,
            },
            origin,
        )
        .await;
        Ok(next)
    }

    /// §34's `BlobVerified` — not a `TransferState` transition (see
    /// `siar_blob_manifest::events`'s own doc comment: verification
    /// can happen either before or after `Completed`), so this doesn't
    /// go through [`Self::apply`] or touch `states`. It still requires
    /// `transfer_id` to be a transfer this service already knows
    /// about — the same "recorded only for something real" discipline
    /// the other methods have, rather than accepting an arbitrary,
    /// unregistered id.
    pub async fn record_blob_verified(
        &self,
        transfer_id: TransferId,
        blob_id: BlobId,
        origin: EventOrigin,
    ) -> Result<(), FileTransferError> {
        if !self
            .states
            .lock()
            .expect("state lock")
            .contains_key(&transfer_id)
        {
            return Err(FileTransferError::UnknownTransfer(transfer_id));
        }
        self.record(
            FileEvent::BlobVerified {
                transfer_id,
                blob_id,
            },
            origin,
        )
        .await;
        Ok(())
    }

    /// Same pattern `siar_messaging::service::MessageService::
    /// record_messaging_event` uses: silently a no-op with no
    /// `event_log` configured (see the struct's own doc comment on
    /// why that's a deliberate, not an accidental, silence); a real
    /// append failure is logged rather than propagated, since by this
    /// point the STATE decision has already succeeded — this service
    /// doesn't roll a decision back just because recording it failed,
    /// matching §14's own outbox-adjacent framing that a durability
    /// hiccup shouldn't corrupt an already-real local decision.
    async fn record(&self, event: FileEvent, origin: EventOrigin) {
        let Some(store) = self.event_log.as_deref() else {
            return;
        };
        let stream_id = event.stream_id();
        let new_event = event.into_new_event(origin);
        if let Err(e) =
            append_with_retry(store, stream_id, new_event, EVENT_LOG_APPEND_MAX_ATTEMPTS).await
        {
            tracing::warn!(error = %e, "failed to record file event to the event log");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_blob_manifest::events::decode_file_event;
    use siar_domain::DeviceId;
    use siar_event_log::InMemoryEventStore;

    fn local_origin() -> EventOrigin {
        EventOrigin::LocalDevice(DeviceId::new())
    }

    fn sample_ids() -> (TransferId, ManifestId, LogicalAttachmentId, BlobId) {
        (
            TransferId::new(),
            ManifestId::new(),
            LogicalAttachmentId::new(),
            BlobId::from_ciphertext(b"some ciphertext"),
        )
    }

    #[tokio::test]
    async fn a_normal_transfer_walks_offer_through_completion_and_records_every_step() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = FileTransferService::new().with_event_log(store.clone());
        let (transfer_id, manifest_id, attachment_id, blob_id) = sample_ids();

        let state = service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                4096,
                4,
                local_origin(),
            )
            .await
            .unwrap();
        assert_eq!(state, TransferState::Offered);

        let state = service
            .apply(transfer_id, TransferEvent::Accept, local_origin())
            .await
            .unwrap();
        assert_eq!(state, TransferState::Accepted);

        let state = service
            .apply(transfer_id, TransferEvent::Start, local_origin())
            .await
            .unwrap();
        assert_eq!(state, TransferState::InProgress);

        let state = service
            .apply(transfer_id, TransferEvent::ChunksComplete, local_origin())
            .await
            .unwrap();
        assert_eq!(state, TransferState::Completed);
        assert_eq!(
            service.current_state(transfer_id),
            Some(TransferState::Completed)
        );

        service
            .record_blob_verified(transfer_id, blob_id, local_origin())
            .await
            .unwrap();

        // Four lifecycle events plus the verification, in order, all
        // decodable — this is the real proof, not just that `apply`
        // returned `Ok`.
        let stream_id = siar_blob_manifest::events::transfer_stream_id(transfer_id);
        let stream = store.read_stream(stream_id, 0, 100).await.unwrap();
        assert_eq!(stream.len(), 5);
        let decoded: Vec<_> = stream
            .iter()
            .map(|e| decode_file_event(e.envelope.schema_version, &e.envelope.payload).unwrap())
            .collect();
        assert!(matches!(decoded[0], FileEvent::TransferCreated { .. }));
        assert!(matches!(decoded[1], FileEvent::TransferAccepted { .. }));
        assert!(matches!(decoded[2], FileEvent::TransferStarted { .. }));
        assert!(matches!(decoded[3], FileEvent::TransferCompleted { .. }));
        assert!(matches!(decoded[4], FileEvent::BlobVerified { .. }));
    }

    #[tokio::test]
    async fn an_invalid_transition_is_rejected_and_records_nothing() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = FileTransferService::new().with_event_log(store.clone());
        let (transfer_id, manifest_id, attachment_id, blob_id) = sample_ids();
        service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                10,
                1,
                local_origin(),
            )
            .await
            .unwrap();

        // Offered can't Start without first being Accepted (see
        // `siar_blob_manifest::transfer_state`'s own transition table).
        let result = service
            .apply(transfer_id, TransferEvent::Start, local_origin())
            .await;
        assert!(matches!(
            result,
            Err(FileTransferError::InvalidTransition(_))
        ));
        // State didn't move, and only the earlier TransferCreated is
        // in the log — the rejected transition recorded nothing.
        assert_eq!(
            service.current_state(transfer_id),
            Some(TransferState::Offered)
        );
        let stream_id = siar_blob_manifest::events::transfer_stream_id(transfer_id);
        assert_eq!(store.read_stream(stream_id, 0, 100).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn fail_carries_a_real_reason_and_apply_refuses_the_bare_variant() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = FileTransferService::new().with_event_log(store.clone());
        let (transfer_id, manifest_id, attachment_id, blob_id) = sample_ids();
        service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                10,
                1,
                local_origin(),
            )
            .await
            .unwrap();
        service
            .apply(transfer_id, TransferEvent::Accept, local_origin())
            .await
            .unwrap();
        service
            .apply(transfer_id, TransferEvent::Start, local_origin())
            .await
            .unwrap();

        let rejected = service
            .apply(transfer_id, TransferEvent::Fail, local_origin())
            .await;
        assert_eq!(rejected, Err(FileTransferError::FailNeedsReason));

        let state = service
            .fail_transfer(
                transfer_id,
                "peer disconnected mid-chunk".to_string(),
                local_origin(),
            )
            .await
            .unwrap();
        assert_eq!(state, TransferState::Failed);

        let stream_id = siar_blob_manifest::events::transfer_stream_id(transfer_id);
        let stream = store.read_stream(stream_id, 0, 100).await.unwrap();
        let last = decode_file_event(
            stream.last().unwrap().envelope.schema_version,
            &stream.last().unwrap().envelope.payload,
        )
        .unwrap();
        assert_eq!(
            last,
            FileEvent::TransferFailed {
                transfer_id,
                reason: "peer disconnected mid-chunk".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn an_unknown_transfer_is_rejected_without_touching_the_event_log() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let service = FileTransferService::new().with_event_log(store.clone());
        let result = service
            .apply(TransferId::new(), TransferEvent::Accept, local_origin())
            .await;
        assert!(matches!(result, Err(FileTransferError::UnknownTransfer(_))));
    }

    #[tokio::test]
    async fn creating_the_same_transfer_twice_is_rejected() {
        let service = FileTransferService::new();
        let (transfer_id, manifest_id, attachment_id, blob_id) = sample_ids();
        service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                10,
                1,
                local_origin(),
            )
            .await
            .unwrap();
        let result = service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                10,
                1,
                local_origin(),
            )
            .await;
        assert_eq!(
            result,
            Err(FileTransferError::DuplicateTransfer(transfer_id))
        );
    }

    /// Without an `event_log` configured, decisions still work — see
    /// the struct's own doc comment on why this is deliberate.
    #[tokio::test]
    async fn works_without_an_event_log_configured_decisions_still_apply() {
        let service = FileTransferService::new();
        let (transfer_id, manifest_id, attachment_id, blob_id) = sample_ids();
        service
            .create_transfer(
                transfer_id,
                manifest_id,
                attachment_id,
                blob_id,
                10,
                1,
                local_origin(),
            )
            .await
            .unwrap();
        let state = service
            .apply(transfer_id, TransferEvent::Accept, local_origin())
            .await
            .unwrap();
        assert_eq!(state, TransferState::Accepted);
    }
}
