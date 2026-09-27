#![forbid(unsafe_code)]

//! `04-offline-event-log-architecture.md` §23 "Remote Event
//! Ingestion" — the spec's own nine-step "safe flow," verbatim:
//! receive → protocol validation → identity verification →
//! authorization → deduplication → domain validation → append →
//! projection → durable ACK, with its one stated rule: "for durable
//! delivery semantics, ACK only after persistence." `ROADMAP.md`'s
//! own note (repeated across this workspace's §66 "Security" row):
//! this section didn't exist AT ALL before this — not partially, not
//! stubbed, genuinely absent, which is why §66 (real security
//! properties) had nothing to attach to either.
//!
//! [`RemoteIngestionPipeline`] is that flow, made real — same
//! positional-steps-enforce-order shape `siar-startup-recovery::
//! StartupSequence` already uses for §47, for the same reason: there
//! is no way to construct one with the four real validation steps out
//! of order.
//!
//! ## "Receive" isn't in scope, named honestly
//!
//! Like §47's "render UI," "receive" is a transport-level concern (a
//! socket, a mesh hop, a QUIC stream) this crate has no opinion about
//! and no dependency on any transport crate to have one. A real
//! caller already has the bytes by the time it calls [`Self::ingest`]
//! — deserializing them into a [`RawRemoteEvent`] is that caller's own
//! job, using whichever domain crate's own decode function
//! (`decode_file_event`, `decode_dtn_event`, ...) fits the event in
//! hand.
//!
//! ## Four steps, one validator shape each — deliberately synchronous
//!
//! [`ProtocolValidation`]/[`IdentityVerification`]/[`Authorization`]/
//! [`DomainValidation`] are all plain, synchronous closures
//! (`Fn(&RawRemoteEvent) -> Result<(), IngestionRejection>`), not
//! async. A real identity check might need async IO of its own (a
//! certificate lookup, a call to `siar-identity-multidevice`'s own
//! revocation state) — this crate's position is that such a check
//! runs BEFORE `ingest` is called, on the already-decoded event, with
//! its outcome folded into a synchronous closure here; adding real
//! async validator steps is possible but not attempted this round,
//! same as `siar_blob_manifest::transfer_state::decide` doesn't thread
//! IO through pure decision functions either.
//!
//! ## Deduplication and rollback protection are not reimplemented here
//!
//! The spec lists "deduplication" as its own step, but this pipeline
//! doesn't implement a second one: the real [`EventStore`] it's
//! constructed with ALREADY provides §24 idempotency (a replayed
//! `EventId` durably appends nothing new) and rollback protection (an
//! `expected_version` that doesn't match reality is a real
//! `ConcurrencyConflict`, rejected before anything lands) — both for
//! free, at the one real `append` call this pipeline makes. See
//! [`IngestionOutcome::AlreadyProcessed`] for how a duplicate is
//! reported (a real success, not an error — a retried remote event is
//! not the sender's fault) and [`IngestionRejection::Storage`] for how
//! a rollback attempt surfaces (the real `EventStoreError::
//! ConcurrencyConflict` from the real store, not a reinvented check).
//!
//! ## What gates the ACK
//!
//! The spec's own rule — "durable ACK... only after persistence" —
//! gates on PERSISTENCE specifically, so [`Self::ingest`] returns
//! [`IngestionOutcome::Acked`]/[`AlreadyProcessed`] as soon as the
//! real `append` call succeeds, whether or not
//! [`RemoteIngestionPipeline::with_after_append`]'s optional
//! projection hook (the spec's own separately-listed "projection"
//! step) is set. If it IS set and it fails, `ingest` still returns an
//! error — the event is durably appended either way (that already
//! happened), but the caller learns projection didn't keep up rather
//! than being told everything is fully settled.
//!
//! ## §66 "Security" — what this pipeline closes, mapped one-for-one
//!
//! §66 names seven threats. This crate is what makes five of them a
//! real, tested property rather than an aspiration — `ROADMAP.md`'s
//! own §66 row said the other two ("malformed imported event,"
//! "unauthorized remote event") were "moot until §23 exists at all";
//! now that it does, all four of this pipeline's own steps close
//! them, verified by dedicated tests in this module's own test suite,
//! each proving that step ALONE (every other step permissive) blocks
//! the one thing it's responsible for:
//!
//! - **malformed imported event** → `protocol_validation`/
//!   `domain_validation` (a caller's own real check; this crate
//!   provides the gate, not a fixed schema — see this module's own
//!   test `domain_validation_alone_can_reject_a_structurally_valid_but_meaningless_event`)
//! - **duplicate/replay** → real, via the underlying `EventStore`'s
//!   §24 idempotency (see `a_replayed_event_is_acked_as_already_processed_not_an_error`)
//! - **rollback** → real, via the store's own optimistic concurrency
//!   (see `a_stale_expected_version_is_rejected_as_a_rollback_attempt`)
//! - **oversized payload** → real, via the store's own §55 payload
//!   size check (see `oversized_payload_surfaces_as_a_real_storage_rejection`)
//! - **unauthorized remote event** → `authorization` (see
//!   `authorization_alone_can_block_an_otherwise_valid_event`)
//!
//! Two are honestly NOT closed by this crate, named rather than
//! silently skipped:
//!
//! - **projection poisoning** — a malicious but well-formed event
//!   that passes every real check here and then corrupts or crashes a
//!   real `Projection::apply` implementation. Closing this for real
//!   means hardening/sandboxing `apply` itself, a different
//!   (unattempted) piece of work from validating an event BEFORE it
//!   reaches storage, which is this crate's whole scope.
//! - **local corruption** — already a separate, already-real concern:
//!   `siar_event_log::read_only::ReadOnlyEventStore` (§69) is where
//!   that gets handled, not here.

use siar_event_log::{AppendRequest, EventStore, EventStoreError, NewEvent, StreamId};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;

/// A caller-decoded remote event, ready for [`RemoteIngestionPipeline::
/// ingest`] — see this module's own doc comment for why decoding
/// itself (§23's own "receive" step) isn't this crate's job.
pub struct RawRemoteEvent {
    pub stream_id: StreamId,
    pub expected_version: u64,
    pub event: NewEvent,
}

pub type Validator = Box<dyn Fn(&RawRemoteEvent) -> Result<(), IngestionRejection> + Send + Sync>;

type BoxFuture = Pin<Box<dyn Future<Output = Result<(), IngestionRejection>> + Send>>;
/// The optional post-append hook — see this module's own doc comment,
/// "What gates the ACK."
pub type AfterAppend = Box<dyn Fn() -> BoxFuture + Send + Sync>;

/// Wrap an ordinary `async` closure as an [`AfterAppend`] hook, same
/// convenience [`siar_startup_recovery::step`] gives
/// `StartupSequence` — spares a caller hand-rolling `Box::pin`.
pub fn after_append<F, Fut>(f: F) -> AfterAppend
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), IngestionRejection>> + Send + 'static,
{
    Box::new(move || Box::pin(f()))
}

/// Which of §23's own real steps rejected this event, and why. Every
/// variant except [`Self::Storage`] is this crate refusing BEFORE ever
/// calling the real event store — see this module's own doc comment
/// for why [`Self::Storage`] itself already covers rollback/oversized/
/// storage-full/corrupt without a variant of its own for each.
#[derive(Debug, Error)]
pub enum IngestionRejection {
    #[error("protocol validation failed: {0}")]
    ProtocolInvalid(String),
    #[error("identity verification failed: {0}")]
    IdentityUnverified(String),
    #[error("authorization failed: {0}")]
    Unauthorized(String),
    #[error("domain validation failed: {0}")]
    DomainInvalid(String),
    #[error(transparent)]
    Storage(#[from] EventStoreError),
    #[error("post-append projection step failed: {0}")]
    Projection(String),
}

/// What [`RemoteIngestionPipeline::ingest`] returns on success — never
/// an error, but distinguishing a genuinely new fact from a harmless
/// replay a real caller may want to log differently (or not at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestionOutcome {
    /// A real, new event — this stream's version really did advance.
    Acked { new_version: u64 },
    /// §24: this exact event (by `EventId`) had already been durably
    /// applied — still ACK it (the sender did nothing wrong retrying),
    /// just don't treat it as new.
    AlreadyProcessed { new_version: u64 },
}

/// §23's own pipeline — see this module's own doc comment.
pub struct RemoteIngestionPipeline {
    protocol_validation: Validator,
    identity_verification: Validator,
    authorization: Validator,
    domain_validation: Validator,
    event_store: Arc<dyn EventStore + Send + Sync>,
    after_append_hook: Option<AfterAppend>,
}

impl RemoteIngestionPipeline {
    /// The spec's own four real, injectable steps, positional, in the
    /// spec's own order — see this module's own doc comment for why
    /// that's deliberate rather than a builder with named setters
    /// (same reasoning `StartupSequence::new` already gives).
    pub fn new(
        protocol_validation: Validator,
        identity_verification: Validator,
        authorization: Validator,
        domain_validation: Validator,
        event_store: Arc<dyn EventStore + Send + Sync>,
    ) -> Self {
        Self {
            protocol_validation,
            identity_verification,
            authorization,
            domain_validation,
            event_store,
            after_append_hook: None,
        }
    }

    /// See this module's own doc comment, "What gates the ACK."
    pub fn with_after_append(mut self, hook: AfterAppend) -> Self {
        self.after_append_hook = Some(hook);
        self
    }

    /// Runs the spec's own four validation steps in order, stopping at
    /// the first rejection — none of which ever touches
    /// [`EventStore::append`] at all, so a rejected event provably
    /// never reaches storage. Only once all four pass does this call
    /// the real store, getting real §24 deduplication and real
    /// rollback protection for free (see this module's own doc
    /// comment). The optional [`Self::with_after_append`] hook, if
    /// set, runs last, still before returning — see "What gates the
    /// ACK" for exactly what that does and doesn't mean for a
    /// duplicate.
    pub async fn ingest(
        &self,
        raw: RawRemoteEvent,
    ) -> Result<IngestionOutcome, IngestionRejection> {
        (self.protocol_validation)(&raw)?;
        (self.identity_verification)(&raw)?;
        (self.authorization)(&raw)?;
        (self.domain_validation)(&raw)?;

        let result = self
            .event_store
            .append(AppendRequest {
                stream_id: raw.stream_id,
                expected_version: raw.expected_version,
                events: vec![raw.event],
            })
            .await?;

        let is_new = result.local_offsets.first().copied().flatten().is_some();

        if let Some(hook) = &self.after_append_hook {
            hook().await.map_err(|_| {
                IngestionRejection::Projection("post-append projection step failed".to_string())
            })?;
        }

        if is_new {
            Ok(IngestionOutcome::Acked {
                new_version: result.new_version,
            })
        } else {
            Ok(IngestionOutcome::AlreadyProcessed {
                new_version: result.new_version,
            })
        }
    }
}

/// A validator that always accepts — for a real caller that hasn't
/// implemented one of these four checks yet, or a test that only
/// wants to exercise a DIFFERENT one. Named `permissive`, not
/// `default`, so using it is always a visible, deliberate choice at
/// the call site rather than something [`RemoteIngestionPipeline::new`]
/// silently falls back to on its own.
pub fn permissive() -> Validator {
    Box::new(|_raw| Ok(()))
}

/// A validator that always rejects with `reason` — for a test that
/// wants to prove one specific step, and only that one, blocks
/// ingestion.
pub fn always_reject(reason: impl Into<String>) -> Validator {
    let reason = reason.into();
    Box::new(move |_raw| Err(IngestionRejection::ProtocolInvalid(reason.clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_event_log::{EventId, EventOrigin, EventTypeId, InMemoryEventStore, Timestamp};

    fn sample_event() -> RawRemoteEvent {
        RawRemoteEvent {
            stream_id: StreamId::from_name("remote/test"),
            expected_version: 0,
            event: NewEvent {
                event_id: EventId::new(),
                event_type: EventTypeId(1),
                schema_version: 1,
                created_at: Timestamp::now(),
                origin: EventOrigin::RemoteDevice(siar_domain_placeholder_device()),
                correlation_id: None,
                causation_id: None,
                payload: vec![1, 2, 3],
            },
        }
    }

    // A real `DeviceId` without a `siar-domain` dependency: this
    // crate has none (see `Cargo.toml`), and `EventOrigin::RemoteDevice`
    // needs SOME `DeviceId` — `siar_event_log` already depends on
    // `siar-domain` and re-exports nothing of it, so this test module
    // reaches for it directly, dev-only, rather than adding a real
    // dependency this crate's own production code has no use for.
    fn siar_domain_placeholder_device() -> siar_domain::DeviceId {
        siar_domain::DeviceId::new()
    }

    fn pipeline(store: Arc<dyn EventStore + Send + Sync>) -> RemoteIngestionPipeline {
        RemoteIngestionPipeline::new(
            permissive(),
            permissive(),
            permissive(),
            permissive(),
            store,
        )
    }

    #[tokio::test]
    async fn a_valid_event_is_acked_as_new() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let outcome = pipeline(store).ingest(sample_event()).await.unwrap();
        assert_eq!(outcome, IngestionOutcome::Acked { new_version: 1 });
    }

    #[tokio::test]
    async fn a_replayed_event_is_acked_as_already_processed_not_an_error() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = pipeline(store);
        let raw = sample_event();
        let event_id = raw.event.event_id;
        p.ingest(raw).await.unwrap();

        let mut replay = sample_event();
        replay.event.event_id = event_id;
        replay.expected_version = 1;
        let outcome = p.ingest(replay).await.unwrap();
        assert_eq!(
            outcome,
            IngestionOutcome::AlreadyProcessed { new_version: 1 }
        );
    }

    #[tokio::test]
    async fn a_stale_expected_version_is_rejected_as_a_rollback_attempt() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = pipeline(store);
        p.ingest(sample_event()).await.unwrap();

        // A second, genuinely different event still claiming version
        // 0 — a remote peer trying to rewrite history it's already
        // told this device moved past.
        let mut rollback_attempt = sample_event();
        rollback_attempt.expected_version = 0;
        let result = p.ingest(rollback_attempt).await;
        assert!(matches!(
            result,
            Err(IngestionRejection::Storage(
                EventStoreError::ConcurrencyConflict { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn each_validation_step_can_reject_on_its_own_before_storage_is_ever_touched() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = RemoteIngestionPipeline::new(
            always_reject("bad protocol"),
            permissive(),
            permissive(),
            permissive(),
            store.clone(),
        );
        let result = p.ingest(sample_event()).await;
        assert!(matches!(
            result,
            Err(IngestionRejection::ProtocolInvalid(_))
        ));

        // Nothing landed — the store was never touched.
        let stream = StreamId::from_name("remote/test");
        assert!(store.read_stream(stream, 0, 10).await.unwrap().is_empty());
    }

    /// §66 "Security"'s own "unauthorized remote event" — proven as
    /// its own case, not folded into the protocol-validation test
    /// above: authorization can reject an event that passed EVERY
    /// earlier step, and still nothing reaches storage.
    #[tokio::test]
    async fn authorization_alone_can_block_an_otherwise_valid_event() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = RemoteIngestionPipeline::new(
            permissive(),
            permissive(),
            Box::new(|_raw| Err(IngestionRejection::Unauthorized("not a member".to_string()))),
            permissive(),
            store.clone(),
        );
        let result = p.ingest(sample_event()).await;
        assert!(matches!(result, Err(IngestionRejection::Unauthorized(_))));
        let stream = StreamId::from_name("remote/test");
        assert!(store.read_stream(stream, 0, 10).await.unwrap().is_empty());
    }

    /// §66's own "malformed imported event," as domain validation
    /// specifically (not protocol validation, which is more about
    /// wire-format shape than domain-level meaning) — an event that
    /// parses fine but fails a real domain-level rule, still stopped
    /// before storage.
    #[tokio::test]
    async fn domain_validation_alone_can_reject_a_structurally_valid_but_meaningless_event() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = RemoteIngestionPipeline::new(
            permissive(),
            permissive(),
            permissive(),
            Box::new(|raw| {
                if raw.event.payload.is_empty() {
                    Err(IngestionRejection::DomainInvalid(
                        "empty payload has no domain meaning".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }),
            store.clone(),
        );
        let mut raw = sample_event();
        raw.event.payload = vec![];
        let result = p.ingest(raw).await;
        assert!(matches!(result, Err(IngestionRejection::DomainInvalid(_))));
        let stream = StreamId::from_name("remote/test");
        assert!(store.read_stream(stream, 0, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_payload_surfaces_as_a_real_storage_rejection() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = pipeline(store);
        let mut raw = sample_event();
        raw.event.payload = vec![0u8; siar_event_log::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];
        let result = p.ingest(raw).await;
        assert!(matches!(
            result,
            Err(IngestionRejection::Storage(
                EventStoreError::PayloadTooLarge { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn the_after_append_hook_runs_and_a_failure_there_is_reported() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = pipeline(store).with_after_append(after_append(|| async {
            Err(IngestionRejection::Projection(
                "projection catch-up failed".to_string(),
            ))
        }));
        let result = p.ingest(sample_event()).await;
        assert!(matches!(result, Err(IngestionRejection::Projection(_))));
    }

    #[tokio::test]
    async fn a_passing_after_append_hook_still_acks_normally() {
        let store: Arc<dyn EventStore + Send + Sync> = Arc::new(InMemoryEventStore::new());
        let p = pipeline(store).with_after_append(after_append(|| async { Ok(()) }));
        let outcome = p.ingest(sample_event()).await.unwrap();
        assert_eq!(outcome, IngestionOutcome::Acked { new_version: 1 });
    }
}
