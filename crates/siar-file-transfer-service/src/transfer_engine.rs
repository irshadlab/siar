//! Wires [`crate::FileTransferService`] (the §26 state machine and the
//! §34 event recording) to `siar-blob-manifest`'s local blob store and
//! reference model, so that a transfer *moves bytes*, not just state.
//!
//! Before this module the service decided and recorded transitions and
//! nothing wrote a byte anywhere; before round 13 of Part 05 there was
//! nowhere to write them. [`TransferEngine`] is the one place those meet:
//!
//! ```text
//! begin_receive ─ create (Offered) ─ add reference ─ Accept ─ Start
//!               ─ reserve space ─ stage
//! receive_chunk ─ verify chunk hash ─ write at offset         (repeat)
//! finish_receive─ verify whole blob ─ publish atomically
//!               ─ ChunksComplete (→ Completed) ─ BlobVerified
//! ```
//!
//! ## Why the order is exactly this (the state machine's own rules)
//!
//! [`siar_blob_manifest::transfer_state::TransferState::transition`]
//! allows `Fail` only from `InProgress` or `Paused`, and sends
//! `ChunksComplete` straight to the terminal `Completed`. Two orderings
//! follow, and getting either wrong is not a style problem but an
//! *illegal transition*:
//!
//! - **`Start` comes before the space reservation.** A transfer the
//!   store refuses for lack of room must be able to end `Failed`; from
//!   `Accepted` it cannot (`Accepted` only allows `Start` or `Cancel`).
//!   So it enters `InProgress` first and the refusal is a legal
//!   `InProgress → Failed`.
//! - **Verify-and-publish comes before `ChunksComplete`.** Once
//!   `Completed` is recorded nothing may fail the transfer, so
//!   completion must not be recorded until the blob has passed its
//!   whole-hash check and been atomically published. A crash in the
//!   instant between publish and `ChunksComplete` leaves a verified blob
//!   with the transfer still "in progress" — exactly the situation
//!   [`TransferEngine::complete_late`] exists to settle.
//!
//! ## Why the reference is added *first* (§148/§149)
//!
//! The reference ("this attachment uses blob B") is recorded at
//! `begin_receive`, before any byte exists, and removed again if the
//! transfer fails or is cancelled. The alternative — add it after the
//! blob is published — leaves a window (crash between publish and add)
//! in which a real, verified blob exists with **no reference**, which is
//! exactly the state [`siar_blob_manifest::gc`] is allowed to delete
//! after its grace period. The order chosen can only fail in the safe
//! direction: a reference to a blob that isn't there yet is the *normal*
//! shape of a download in progress (GC reports it as informational and
//! never acts on it), while a leaked reference after a failed transfer
//! is removed by the compensation below or, at worst, cleared by the
//! next reconciliation.
//!
//! ## What each failure does
//!
//! | failure | result |
//! |---|---|
//! | store refuses space up front | transfer `Failed` ("storage reservation denied"); nothing staged; reference removed |
//! | a chunk fails its hash | that chunk is rejected and not written; after [`TransferEngine::MAX_BAD_CHUNKS`] the transfer `Failed` and staging discarded |
//! | whole blob fails identity check | transfer `Failed`; the bytes are in `quarantine/`, never published; reference removed |
//! | crash mid-transfer | staging survives; [`TransferEngine::restore`] + [`TransferEngine::resume_after_restart`] re-verify and continue |
//!
//! ## Restart (§52 + §93)
//!
//! [`TransferEngine::restore`] replays the event log into the state
//! machine and runs the store's own staging recovery.
//! [`TransferEngine::reconcile_after_restart`] then settles every
//! transfer that was in flight against **what is actually on disk**,
//! because the log can lag (three of the nine file events are
//! best-effort): staging that survived is resumable; a blob that was
//! published just before the crash is completable; staging that is gone
//! means the transfer is failed honestly rather than left "in progress"
//! forever.
//!
//! ## Blocking
//!
//! Like the store it drives, the byte path is synchronous file I/O
//! inside `async fn`s; call from a blocking-capable context on a
//! multi-threaded runtime.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use siar_blob_manifest::blob_io::BlobWriter;
use siar_blob_manifest::descriptor::BlobDescriptor;
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::fs_store::{FsBlobStore, FsBlobWriter, RecoveryReport};
use siar_blob_manifest::ids::{BlobId, LogicalAttachmentId, ManifestId, TransferId};
use siar_blob_manifest::manifest::BlobManifest;
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::{
    BlobReference, BlobRole, ObjectId, RefOp, ReferenceStore,
};
use siar_blob_manifest::store_error::BlobError;
use siar_blob_manifest::transfer_state::{TransferEvent, TransferState};
use siar_event_log::namespace::NamespaceId;
use siar_event_log::{EventOrigin, EventStore};
use thiserror::Error;

use crate::{FileTransferError, FileTransferService, RestoredTransfer};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineError {
    #[error(transparent)]
    Service(#[from] FileTransferError),
    #[error(transparent)]
    Blob(#[from] BlobError),
    #[error("transfer {0} has no active write")]
    NotActive(TransferId),
    #[error("transfer {0} has nothing on disk to resume from")]
    NothingToResume(TransferId),
    #[error("transfer {id} failed: {reason}")]
    TransferFailed {
        id: TransferId,
        reason: &'static str,
    },
}

/// Everything needed to start receiving a transfer.
#[derive(Debug, Clone)]
pub struct ReceiveRequest {
    pub transfer_id: TransferId,
    pub manifest_id: ManifestId,
    pub logical_attachment_id: LogicalAttachmentId,
    pub descriptor: BlobDescriptor,
    pub manifest: BlobManifest,
    /// The domain object (a message id…) that will hold the reference.
    pub object: ObjectId,
    pub role: BlobRole,
    pub expires: TransferExpiry,
    pub origin: EventOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkProgress {
    pub received: u32,
    pub total: u32,
    pub complete: bool,
}

/// What survives across pause/restart: how to rebuild the writer.
#[derive(Debug, Clone)]
struct Ctx {
    descriptor: BlobDescriptor,
    manifest: BlobManifest,
    object: ObjectId,
    role: BlobRole,
    origin: EventOrigin,
}

struct Live<M: BlobMetadataStore> {
    writer: FsBlobWriter<M>,
    bad_chunks: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreSummary {
    pub replayed: Vec<RestoredTransfer>,
    pub staging: RecoveryReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcileOutcome {
    /// In flight, staging intact: call [`TransferEngine::resume_after_restart`].
    pub resumable: Vec<TransferId>,
    /// The blob was published before the crash:
    /// call [`TransferEngine::complete_late`].
    pub completable_late: Vec<TransferId>,
    /// Staging was lost; these were marked `Failed`.
    pub failed_lost_staging: Vec<TransferId>,
}

pub struct TransferEngine<M: BlobMetadataStore, R: ReferenceStore + ?Sized> {
    service: FileTransferService,
    store: FsBlobStore<M>,
    refs: Arc<R>,
    namespace: NamespaceId,
    live: Mutex<HashMap<TransferId, Live<M>>>,
    ctx: Mutex<HashMap<TransferId, Ctx>>,
    restored: Mutex<HashMap<TransferId, RestoredTransfer>>,
}

impl<M: BlobMetadataStore, R: ReferenceStore + ?Sized> TransferEngine<M, R> {
    /// A chunk that fails its hash this many times fails the transfer.
    pub const MAX_BAD_CHUNKS: u32 = 3;

    pub fn new(
        service: FileTransferService,
        store: FsBlobStore<M>,
        refs: Arc<R>,
        namespace: NamespaceId,
    ) -> Self {
        Self {
            service,
            store,
            refs,
            namespace,
            live: Mutex::new(HashMap::new()),
            ctx: Mutex::new(HashMap::new()),
            restored: Mutex::new(HashMap::new()),
        }
    }

    pub fn service(&self) -> &FileTransferService {
        &self.service
    }

    pub fn store(&self) -> &FsBlobStore<M> {
        &self.store
    }

    fn reference(&self, ctx: &Ctx) -> BlobReference {
        BlobReference {
            owner_namespace: self.namespace.clone(),
            object_id: ctx.object.clone(),
            blob_id: ctx.descriptor.blob_id,
            role: ctx.role,
        }
    }

    fn drop_reference(&self, ctx: &Ctx) {
        // Compensation must never mask the real error: a failure to
        // remove is logged; the next reconciliation clears it.
        if let Err(e) = self.refs.apply(&[RefOp::Remove(self.reference(ctx))]) {
            tracing::warn!(error = %e, "failed to remove a blob reference during compensation");
        }
    }

    /// Marks the transfer failed and undoes its reference. The store's
    /// own staging is left to the caller (quarantined, discarded or
    /// kept) because the right choice depends on the failure.
    async fn fail(&self, id: TransferId, reason: &'static str, origin: EventOrigin) -> EngineError {
        let ctx = self.ctx.lock().expect("ctx").remove(&id);
        self.live.lock().expect("live").remove(&id);
        if let Some(ctx) = &ctx {
            self.drop_reference(ctx);
        }
        if let Err(e) = self
            .service
            .fail_transfer(id, reason.to_string(), origin)
            .await
        {
            return EngineError::Service(e);
        }
        EngineError::TransferFailed { id, reason }
    }

    /// Offer → accepted → space reserved → staging open → in progress.
    /// If the blob is already stored (§59 dedup) the transfer completes
    /// immediately without writing anything.
    pub async fn begin_receive(&self, req: ReceiveRequest) -> Result<TransferState, EngineError> {
        let id = req.transfer_id;
        let ctx = Ctx {
            descriptor: req.descriptor,
            manifest: req.manifest.clone(),
            object: req.object.clone(),
            role: req.role,
            origin: req.origin,
        };
        let total = req.descriptor.size.bytes();
        let chunk_count = u32::try_from(req.manifest.chunks.len()).unwrap_or(u32::MAX);

        self.service
            .create_transfer(
                id,
                req.manifest_id,
                req.logical_attachment_id,
                req.descriptor.blob_id,
                total,
                chunk_count,
                req.origin,
            )
            .await?;
        // §148: the reference exists before any byte does.
        self.refs.apply(&[RefOp::Add(self.reference(&ctx))])?;
        self.ctx.lock().expect("ctx").insert(id, ctx.clone());

        self.service
            .apply(id, TransferEvent::Accept, req.origin)
            .await?;
        // `Start` first: see the module doc — only an in-progress
        // transfer can legally fail.
        self.service
            .apply(id, TransferEvent::Start, req.origin)
            .await?;

        let writer = match self
            .store
            .begin_write_with_manifest(req.descriptor, Some(req.manifest))
        {
            Ok(w) => w,
            Err(e) => {
                let reason = match &e {
                    BlobError::ReservationDenied { .. } | BlobError::BlobTooLarge { .. } => {
                        "storage reservation denied"
                    }
                    BlobError::TooManyConcurrentWrites { .. } => "too many concurrent transfers",
                    BlobError::AlreadyStaging(_) => "blob is already being received",
                    _ => "could not open staging",
                };
                let _ = self.fail(id, reason, req.origin).await;
                return Err(EngineError::Blob(e));
            }
        };

        if writer.was_already_present() {
            // Nothing to receive: the bytes are already here and verified.
            return self
                .complete_present(id, req.descriptor.blob_id, req.origin)
                .await;
        }
        self.live.lock().expect("live").insert(
            id,
            Live {
                writer,
                bad_chunks: 0,
            },
        );
        Ok(TransferState::InProgress)
    }

    async fn complete_present(
        &self,
        id: TransferId,
        blob: BlobId,
        origin: EventOrigin,
    ) -> Result<TransferState, EngineError> {
        let state = self
            .service
            .apply(id, TransferEvent::ChunksComplete, origin)
            .await?;
        self.service.record_blob_verified(id, blob, origin).await?;
        Ok(state)
    }

    /// Writes one chunk. See the module doc's failure table.
    pub async fn receive_chunk(
        &self,
        id: TransferId,
        index: u32,
        data: &[u8],
    ) -> Result<ChunkProgress, EngineError> {
        let (result, bad, progress, origin) = {
            let mut live = self.live.lock().expect("live");
            let Some(l) = live.get_mut(&id) else {
                return Err(EngineError::NotActive(id));
            };
            let result = l.writer.write_chunk_sync(index, data);
            if matches!(result, Err(BlobError::ChunkHashMismatch { .. })) {
                l.bad_chunks += 1;
            }
            let received = l.writer.received_chunks() as u32;
            let total = received
                + l.writer
                    .missing_ranges()
                    .iter()
                    .map(|(a, b)| b - a)
                    .sum::<u32>();
            let origin = self.ctx.lock().expect("ctx").get(&id).map(|c| c.origin);
            (
                result,
                l.bad_chunks,
                ChunkProgress {
                    received,
                    total,
                    complete: received == total,
                },
                origin,
            )
        };
        match result {
            Ok(()) => Ok(progress),
            Err(e) => {
                if bad >= Self::MAX_BAD_CHUNKS {
                    if let Some(origin) = origin {
                        // Drop the writer first so the staging can go.
                        let blob = self
                            .ctx
                            .lock()
                            .expect("ctx")
                            .get(&id)
                            .map(|c| c.descriptor.blob_id);
                        self.live.lock().expect("live").remove(&id);
                        if let Some(b) = blob {
                            let _ = self.store.discard_staging(b);
                        }
                        return Err(self.fail(id, "too many corrupt chunks", origin).await);
                    }
                }
                Err(EngineError::Blob(e))
            }
        }
    }

    /// Verifies and publishes the blob; completes the transfer.
    pub async fn finish_receive(&self, id: TransferId) -> Result<BlobId, EngineError> {
        let (ctx, live) = {
            let ctx = self.ctx.lock().expect("ctx").get(&id).cloned();
            let live = self.live.lock().expect("live").remove(&id);
            (ctx, live)
        };
        let (Some(ctx), Some(live)) = (ctx, live) else {
            return Err(EngineError::NotActive(id));
        };
        let origin = ctx.origin;
        let blob = ctx.descriptor.blob_id;

        // Refuse early (and keep the writer) if chunks are missing: an
        // incomplete transfer is not a failed one.
        if !live.writer.missing_ranges().is_empty() {
            let missing = live
                .writer
                .missing_ranges()
                .iter()
                .map(|(a, b)| b - a)
                .sum::<u32>();
            self.live.lock().expect("live").insert(id, live);
            return Err(EngineError::Blob(BlobError::Incomplete {
                missing_chunks: missing,
            }));
        }

        // Verify and publish FIRST; only then record completion.
        match live.writer.finalize_sync() {
            Ok(published) => {
                // If recording fails after this point the blob is
                // already published and verified; a restart settles the
                // transfer via `complete_late`.
                self.service
                    .apply(id, TransferEvent::ChunksComplete, origin)
                    .await?;
                self.service
                    .record_blob_verified(id, published, origin)
                    .await?;
                self.ctx.lock().expect("ctx").remove(&id);
                Ok(published)
            }
            Err(BlobError::BlobHashMismatch { .. }) => {
                // The store already quarantined the bytes.
                Err(self
                    .fail(id, "blob failed identity verification", origin)
                    .await)
            }
            Err(e) => {
                let _ = self.fail(id, "could not publish blob", origin).await;
                let _ = blob;
                Err(EngineError::Blob(e))
            }
        }
    }

    /// Stops writing but keeps the staging for resume, releasing the
    /// space reservation in the meantime.
    pub async fn pause(&self, id: TransferId) -> Result<TransferState, EngineError> {
        let origin = self.origin_of(id)?;
        let live = self.live.lock().expect("live").remove(&id);
        match self.service.apply(id, TransferEvent::Pause, origin).await {
            Ok(s) => {
                drop(live); // releases the reservation, keeps the staging
                Ok(s)
            }
            Err(e) => {
                if let Some(l) = live {
                    self.live.lock().expect("live").insert(id, l);
                }
                Err(e.into())
            }
        }
    }

    /// Re-opens the writer (re-verifying staged chunks) and resumes.
    pub async fn resume(&self, id: TransferId) -> Result<TransferState, EngineError> {
        let ctx = self
            .ctx
            .lock()
            .expect("ctx")
            .get(&id)
            .cloned()
            .ok_or(EngineError::NotActive(id))?;
        let writer = self
            .store
            .begin_write_with_manifest(ctx.descriptor, Some(ctx.manifest.clone()))?;
        let state = self
            .service
            .apply(id, TransferEvent::Resume, ctx.origin)
            .await?;
        self.live.lock().expect("live").insert(
            id,
            Live {
                writer,
                bad_chunks: 0,
            },
        );
        Ok(state)
    }

    /// Cancels: discards staging and releases the reference.
    pub async fn cancel(&self, id: TransferId) -> Result<TransferState, EngineError> {
        let ctx = self
            .ctx
            .lock()
            .expect("ctx")
            .get(&id)
            .cloned()
            .ok_or(EngineError::NotActive(id))?;
        let state = self
            .service
            .apply(id, TransferEvent::Cancel, ctx.origin)
            .await?;
        self.live.lock().expect("live").remove(&id);
        let _ = self.store.discard_staging(ctx.descriptor.blob_id);
        self.drop_reference(&ctx);
        self.ctx.lock().expect("ctx").remove(&id);
        Ok(state)
    }

    /// Blobs with a transfer in flight: GC must never collect these.
    pub fn active_blob_ids(&self) -> HashSet<BlobId> {
        self.ctx
            .lock()
            .expect("ctx")
            .values()
            .map(|c| c.descriptor.blob_id)
            .collect()
    }

    /// One mark-and-sweep (§151) over this engine's own store and
    /// reference model, with the in-flight blobs protected. Scheduling
    /// is [`crate::gc_schedule::GcScheduler`]'s job.
    pub fn gc_sweep(
        &self,
        pinned: &HashSet<BlobId>,
        now_millis: u64,
        policy: &siar_blob_manifest::storage_lifecycle::GcPolicy,
        page: usize,
    ) -> Result<siar_blob_manifest::gc::GcReport, EngineError> {
        let active = self.active_blob_ids();
        Ok(siar_blob_manifest::gc::sweep(
            &self.store,
            &*self.refs,
            pinned,
            &active,
            now_millis,
            policy,
            page,
        )?)
    }

    /// Contiguous `[start, end)` runs still missing for a live transfer
    /// — what a receiver puts in a `ChunkRequest`/`Resume` (§31, §132).
    pub fn missing_ranges(&self, id: TransferId) -> Result<Vec<(u32, u32)>, EngineError> {
        let live = self.live.lock().expect("live");
        live.get(&id)
            .map(|l| l.writer.missing_ranges())
            .ok_or(EngineError::NotActive(id))
    }

    fn origin_of(&self, id: TransferId) -> Result<EventOrigin, EngineError> {
        self.ctx
            .lock()
            .expect("ctx")
            .get(&id)
            .map(|c| c.origin)
            .ok_or(EngineError::NotActive(id))
    }

    /// Restart step 1: replay the log and run staging recovery.
    pub async fn restore(
        &self,
        event_log: &(dyn EventStore + Send + Sync),
    ) -> Result<RestoreSummary, EngineError> {
        let replayed = self.service.restore_from_log(event_log).await?;
        let staging = self.store.recover()?;
        let mut map = self.restored.lock().expect("restored");
        for t in &replayed {
            map.insert(t.transfer_id, *t);
        }
        Ok(RestoreSummary { replayed, staging })
    }

    /// Restart step 2: settle every in-flight transfer against disk.
    /// See the module doc.
    pub async fn reconcile_after_restart(
        &self,
        origin: EventOrigin,
    ) -> Result<ReconcileOutcome, EngineError> {
        let mut out = ReconcileOutcome::default();
        let mut inflight: Vec<RestoredTransfer> = self
            .restored
            .lock()
            .expect("restored")
            .values()
            .copied()
            .filter(|t| {
                matches!(
                    t.state,
                    TransferState::Offered
                        | TransferState::Accepted
                        | TransferState::InProgress
                        | TransferState::Paused
                )
            })
            .collect();
        // Deterministic order: by the id's display form.
        inflight.sort_by_key(|t| t.transfer_id.to_string());

        for t in inflight {
            if self.store.contains_sync(t.blob_id) {
                out.completable_late.push(t.transfer_id);
                continue;
            }
            let has_staging = matches!(self.store.staging_meta(t.blob_id)?, Some((_, Some(_))));
            if has_staging {
                out.resumable.push(t.transfer_id);
            } else {
                // Nothing verifiable survived: say so instead of
                // leaving the transfer "in progress" forever.
                self.service
                    .fail_transfer(t.transfer_id, "staging lost across restart".into(), origin)
                    .await?;
                out.failed_lost_staging.push(t.transfer_id);
            }
        }
        Ok(out)
    }

    fn ctx_from_staging(
        &self,
        id: TransferId,
        object: ObjectId,
        role: BlobRole,
        origin: EventOrigin,
    ) -> Result<Ctx, EngineError> {
        let restored = *self
            .restored
            .lock()
            .expect("restored")
            .get(&id)
            .ok_or(EngineError::NotActive(id))?;
        let (descriptor, manifest) = match self.store.staging_meta(restored.blob_id)? {
            Some((d, Some(m))) => (d, m),
            _ => return Err(EngineError::NothingToResume(id)),
        };
        Ok(Ctx {
            descriptor,
            manifest,
            object,
            role,
            origin,
        })
    }

    /// Brings a restored transfer's service state to `InProgress`,
    /// applying whichever transition the (possibly lagging) log state
    /// needs.
    async fn bring_to_in_progress(
        &self,
        id: TransferId,
        origin: EventOrigin,
    ) -> Result<(), EngineError> {
        loop {
            match self.service.current_state(id) {
                Some(TransferState::Offered) => {
                    self.service
                        .apply(id, TransferEvent::Accept, origin)
                        .await?;
                }
                Some(TransferState::Accepted) => {
                    self.service.apply(id, TransferEvent::Start, origin).await?;
                }
                Some(TransferState::Paused) => {
                    self.service
                        .apply(id, TransferEvent::Resume, origin)
                        .await?;
                }
                Some(TransferState::InProgress) => return Ok(()),
                _ => return Err(EngineError::NotActive(id)),
            }
        }
    }

    /// Restart step 3a: continue a transfer whose staging survived.
    pub async fn resume_after_restart(
        &self,
        id: TransferId,
        object: ObjectId,
        role: BlobRole,
        origin: EventOrigin,
    ) -> Result<ChunkProgress, EngineError> {
        let ctx = self.ctx_from_staging(id, object, role, origin)?;
        let writer = self
            .store
            .begin_write_with_manifest(ctx.descriptor, Some(ctx.manifest.clone()))?;
        // The reference may not have survived (or may have): adding is
        // idempotent, so just assert it.
        self.refs.apply(&[RefOp::Add(self.reference(&ctx))])?;
        self.bring_to_in_progress(id, origin).await?;
        let received = writer.received_chunks() as u32;
        let total = ctx.manifest.chunks.len() as u32;
        self.ctx.lock().expect("ctx").insert(id, ctx);
        self.live.lock().expect("live").insert(
            id,
            Live {
                writer,
                bad_chunks: 0,
            },
        );
        Ok(ChunkProgress {
            received,
            total,
            complete: received == total,
        })
    }

    /// Restart step 3b: the blob was published just before the crash;
    /// finish the bookkeeping. The stored bytes are re-hashed first —
    /// "published" is verified, not assumed.
    pub async fn complete_late(
        &self,
        id: TransferId,
        object: ObjectId,
        role: BlobRole,
        origin: EventOrigin,
    ) -> Result<BlobId, EngineError> {
        let restored = *self
            .restored
            .lock()
            .expect("restored")
            .get(&id)
            .ok_or(EngineError::NotActive(id))?;
        let blob = restored.blob_id;
        if !self.store.verify_object(blob)? {
            self.service
                .fail_transfer(id, "stored blob failed re-verification".into(), origin)
                .await?;
            return Err(EngineError::TransferFailed {
                id,
                reason: "stored blob failed re-verification",
            });
        }
        let reference = BlobReference {
            owner_namespace: self.namespace.clone(),
            object_id: object,
            blob_id: blob,
            role,
        };
        self.refs.apply(&[RefOp::Add(reference)])?;
        self.bring_to_in_progress(id, origin).await?;
        self.service
            .apply(id, TransferEvent::ChunksComplete, origin)
            .await?;
        self.service.record_blob_verified(id, blob, origin).await?;
        Ok(blob)
    }
}
