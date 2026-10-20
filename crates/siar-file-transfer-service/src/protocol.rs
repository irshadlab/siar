//! `files/1` wired to the engine (spec 05 §132, §207 Phase 4).
//!
//! `siar_blob_manifest::wire` defines the bytes; the transfer engine
//! moves blobs; until now nothing connected them. [`FileReceiver`]
//! turns inbound `files/1` messages into engine calls and says what to
//! send back; [`FileSender`] serves a receiver's chunk requests from a
//! [`BlobReader`]. Neither owns a transport — they take and produce
//! frames, so the same code runs over iroh, BLE or a test buffer.
//!
//! Flow (§132): `Offer → (user/policy decides) → ManifestRequest →
//! Manifest → ChunkRequest → chunk frames → Complete(BlobVerified)`.
//!
//! What is verified where: the wire layer checks structure before it
//! allocates; this layer checks the manifest is *the offered blob's*
//! (§77) and agrees with the offer; the engine's writer checks every
//! chunk hash before storage and the whole blob before publishing.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use siar_blob_manifest::blob_io::BlobReader;
use siar_blob_manifest::byte_range::ByteRange;
use siar_blob_manifest::expiry::TransferExpiry;
use siar_blob_manifest::file_offer::{FileOffer, FileOfferDecision};
use siar_blob_manifest::ids::{LogicalAttachmentId, ManifestId, TransferId};
use siar_blob_manifest::manifest::BlobManifest;
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::{BlobRole, ObjectId, ReferenceStore};
use siar_blob_manifest::resume::ResumeBitmap;
use siar_blob_manifest::source_trust::verify_manifest_authenticity;
use siar_blob_manifest::store_error::BlobError;
use siar_blob_manifest::transfer_control::{allows_transfer_completion, TransferAckKind};
use siar_blob_manifest::transfer_state::TransferState;
use siar_blob_manifest::wire::{
    decode_manifest, negotiate, read_chunk_frame, write_chunk_frame, CancelV1, ChunkHeaderV1,
    ChunkRequestV1, FileControlV1, FileOfferV1, FilesCapabilities, NegotiatedFiles,
    NegotiationError, OfferDecisionV1, TransferCompleteV1, WireError, WireLimits,
};
use siar_event_log::EventOrigin;
use thiserror::Error;

use crate::transfer_engine::{ChunkProgress, EngineError, ReceiveRequest, TransferEngine};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    #[error(transparent)]
    Blob(#[from] BlobError),
    #[error("no pending offer or active transfer {0}")]
    UnknownTransfer(TransferId),
    #[error("offer {0} expired before it was answered")]
    OfferExpired(TransferId),
    #[error("offer is outside what was negotiated")]
    OutsideNegotiated,
    #[error("manifest does not belong to the offered blob")]
    ManifestNotOffered,
    #[error("manifest disagrees with the offer about {0}")]
    ManifestDisagreesWithOffer(&'static str),
    #[error("message is not valid in this role or state")]
    Unexpected,
    #[error("transfer was cancelled")]
    Cancelled,
    #[error("transfer is paused")]
    Paused,
    #[error("transfer {0} is already known")]
    Duplicate(TransferId),
}

/// Where a received blob will be referenced from, and until when.
#[derive(Debug, Clone)]
pub struct ReceiveContext {
    pub object: ObjectId,
    pub role: BlobRole,
    pub expires: TransferExpiry,
    pub origin: EventOrigin,
    pub logical_attachment_id: LogicalAttachmentId,
}

struct Pending {
    offer: FileOffer,
    negotiated: NegotiatedFiles,
}

struct Active {
    manifest: Arc<BlobManifest>,
    ceiling: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkOutcome {
    Progress(ChunkProgress),
    /// Every chunk arrived, the blob verified and published: send this.
    Complete(FileControlV1),
}

// ---------------------------------------------------------------------
// Receiver
// ---------------------------------------------------------------------

pub struct FileReceiver<M: BlobMetadataStore, R: ReferenceStore + ?Sized> {
    engine: Arc<TransferEngine<M, R>>,
    limits: WireLimits,
    local: FilesCapabilities,
    pending: Mutex<HashMap<TransferId, Pending>>,
    active: Mutex<HashMap<TransferId, Active>>,
}

impl<M: BlobMetadataStore, R: ReferenceStore + ?Sized> FileReceiver<M, R> {
    pub fn new(
        engine: Arc<TransferEngine<M, R>>,
        limits: WireLimits,
        local: FilesCapabilities,
    ) -> Self {
        Self {
            engine,
            local: local.clamped_to(&limits),
            limits,
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// §134: what to advertise.
    pub fn capabilities(&self) -> FilesCapabilities {
        self.local
    }

    /// §134: agree on behaviour with a sender's stated wishes.
    pub fn negotiate_with(
        &self,
        sender: &FilesCapabilities,
    ) -> Result<NegotiatedFiles, ProtocolError> {
        Ok(negotiate(&sender.clamped_to(&self.limits), &self.local)?)
    }

    /// Validates an inbound offer against local limits and the
    /// negotiated envelope and parks it until [`Self::decide`]. Returns
    /// the domain offer for whatever shows it to the user (§144).
    pub fn on_offer(
        &self,
        offer: FileOfferV1,
        negotiated: NegotiatedFiles,
        now_millis: u64,
    ) -> Result<FileOffer, ProtocolError> {
        let offer = offer.into_domain(&self.limits)?;
        if offer.is_expired(now_millis) {
            return Err(ProtocolError::OfferExpired(offer.transfer_id));
        }
        if !negotiated.admits(offer.blob.size.bytes(), offer.blob.chunking.chunk_size) {
            return Err(ProtocolError::OutsideNegotiated);
        }
        let id = offer.transfer_id;
        let mut pending = self.pending.lock().expect("pending");
        if pending.contains_key(&id) || self.active.lock().expect("active").contains_key(&id) {
            return Err(ProtocolError::Duplicate(id));
        }
        pending.insert(
            id,
            Pending {
                offer: offer.clone(),
                negotiated,
            },
        );
        Ok(offer)
    }

    /// Answers a parked offer. `Accept` asks for the manifest; every
    /// other decision drops the offer and says so.
    pub fn decide(
        &self,
        id: TransferId,
        decision: FileOfferDecision,
    ) -> Result<FileControlV1, ProtocolError> {
        let mut pending = self.pending.lock().expect("pending");
        if !pending.contains_key(&id) {
            return Err(ProtocolError::UnknownTransfer(id));
        }
        if decision == FileOfferDecision::Accept {
            return Ok(FileControlV1::ManifestRequest { transfer_id: id });
        }
        pending.remove(&id);
        Ok(FileControlV1::OfferReply {
            transfer_id: id,
            decision: OfferDecisionV1::from(decision),
        })
    }

    /// Takes the manifest frame, proves it is the offered blob's, starts
    /// the engine, and returns the first `ChunkRequest` — or `Complete`
    /// at once if the blob was already stored (§59 dedup).
    pub async fn on_manifest(
        &self,
        manifest_frame: &[u8],
        ctx: ReceiveContext,
    ) -> Result<FileControlV1, ProtocolError> {
        let frame = decode_manifest(manifest_frame, &self.limits)?;
        let id = frame.transfer_id;
        let (offer, negotiated) = {
            let pending = self.pending.lock().expect("pending");
            let p = pending.get(&id).ok_or(ProtocolError::UnknownTransfer(id))?;
            (p.offer.clone(), p.negotiated)
        };
        let m = &frame.manifest;
        if !verify_manifest_authenticity(m, offer.blob.blob_id) {
            return Err(ProtocolError::ManifestNotOffered);
        }
        if m.total_size != offer.blob.size.bytes() {
            return Err(ProtocolError::ManifestDisagreesWithOffer("size"));
        }
        if m.chunk_size != offer.blob.chunking.chunk_size {
            return Err(ProtocolError::ManifestDisagreesWithOffer("chunk size"));
        }
        if m.chunk_size > negotiated.chunk_size_ceiling {
            return Err(ProtocolError::OutsideNegotiated);
        }

        let mut descriptor = offer.blob;
        descriptor.manifest_id.get_or_insert_with(ManifestId::new);
        let req = ReceiveRequest {
            transfer_id: id,
            manifest_id: descriptor.manifest_id.unwrap_or_else(ManifestId::new),
            logical_attachment_id: ctx.logical_attachment_id,
            descriptor,
            manifest: frame.manifest.clone(),
            object: ctx.object,
            role: ctx.role,
            expires: ctx.expires,
            origin: ctx.origin,
        };
        let state = self.engine.begin_receive(req).await?;
        self.pending.lock().expect("pending").remove(&id);

        if state == TransferState::Completed {
            return Ok(complete_message(id, offer.blob.blob_id));
        }
        self.active.lock().expect("active").insert(
            id,
            Active {
                manifest: Arc::new(frame.manifest),
                ceiling: negotiated.chunk_size_ceiling,
            },
        );
        Ok(FileControlV1::ChunkRequest(ChunkRequestV1 {
            transfer_id: id,
            ranges: self.engine.missing_ranges(id)?,
        }))
    }

    /// Reads one chunk frame for transfer `id` and writes it. On the
    /// last chunk the blob is verified and published and the
    /// `Complete(BlobVerified)` reply is returned.
    pub async fn on_chunk<Rd: Read>(
        &self,
        id: TransferId,
        input: &mut Rd,
    ) -> Result<ChunkOutcome, ProtocolError> {
        let (manifest, ceiling) = {
            let active = self.active.lock().expect("active");
            let a = active.get(&id).ok_or(ProtocolError::UnknownTransfer(id))?;
            (Arc::clone(&a.manifest), a.ceiling)
        };
        let (header, body) = read_chunk_frame(input, id, &manifest, ceiling)?;
        let progress = self
            .engine
            .receive_chunk(id, header.chunk_index, &body)
            .await?;
        if !progress.complete {
            return Ok(ChunkOutcome::Progress(progress));
        }
        let blob = self.engine.finish_receive(id).await?;
        self.active.lock().expect("active").remove(&id);
        Ok(ChunkOutcome::Complete(complete_message(id, blob)))
    }

    /// Asks again for whatever is still missing — what a receiver sends
    /// when chunks it asked for never arrived (lost or stalled link). The
    /// sender replaces its queue with exactly these ranges, so nothing
    /// already received is sent twice.
    pub fn request_missing(&self, id: TransferId) -> Result<FileControlV1, ProtocolError> {
        if !self.active.lock().expect("active").contains_key(&id) {
            return Err(ProtocolError::UnknownTransfer(id));
        }
        Ok(FileControlV1::ChunkRequest(ChunkRequestV1 {
            transfer_id: id,
            ranges: self.engine.missing_ranges(id)?,
        }))
    }

    /// Re-issues the manifest request for an accepted offer whose
    /// manifest never arrived.
    pub fn request_manifest_again(&self, id: TransferId) -> Result<FileControlV1, ProtocolError> {
        if !self.pending.lock().expect("pending").contains_key(&id) {
            return Err(ProtocolError::UnknownTransfer(id));
        }
        Ok(FileControlV1::ManifestRequest { transfer_id: id })
    }

    /// Local user/policy pauses (§102): the engine keeps the staging.
    pub async fn pause(&self, id: TransferId) -> Result<FileControlV1, ProtocolError> {
        self.engine.pause(id).await?;
        Ok(FileControlV1::Pause { transfer_id: id })
    }

    /// §31: resume asks only for what is still missing — no restart.
    pub async fn resume(&self, id: TransferId) -> Result<FileControlV1, ProtocolError> {
        self.engine.resume(id).await?;
        Ok(FileControlV1::Resume(ChunkRequestV1 {
            transfer_id: id,
            ranges: self.engine.missing_ranges(id)?,
        }))
    }

    /// Local cancel (§101). The engine discards staging, so
    /// `keep_partial` is always `false` from this side.
    pub async fn cancel(&self, id: TransferId) -> Result<FileControlV1, ProtocolError> {
        self.engine.cancel(id).await?;
        self.active.lock().expect("active").remove(&id);
        Ok(FileControlV1::Cancel(CancelV1 {
            transfer_id: id,
            keep_partial: false,
            reason: None,
        }))
    }

    /// Messages a *sender* may send a receiver. Anything else is
    /// [`ProtocolError::Unexpected`].
    pub async fn on_sender_message(&self, msg: FileControlV1) -> Result<(), ProtocolError> {
        match msg {
            FileControlV1::Cancel(c) => {
                self.pending.lock().expect("pending").remove(&c.transfer_id);
                self.engine.cancel(c.transfer_id).await?;
                self.active.lock().expect("active").remove(&c.transfer_id);
                Ok(())
            }
            _ => Err(ProtocolError::Unexpected),
        }
    }
}

fn complete_message(id: TransferId, blob_id: siar_blob_manifest::ids::BlobId) -> FileControlV1 {
    FileControlV1::Complete(TransferCompleteV1 {
        transfer_id: id,
        blob_id,
        ack: TransferAckKind::BlobVerified,
    })
}

// ---------------------------------------------------------------------
// Sender
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStatus {
    InFlight,
    Paused,
    Cancelled,
    /// The receiver confirmed `BlobVerified` for this very blob (§100).
    Completed,
}

struct SendState {
    paused: bool,
    completed: bool,
    acked: ResumeBitmap,
}

pub struct FileSender<R: BlobReader> {
    transfer_id: TransferId,
    manifest: Arc<BlobManifest>,
    reader: R,
    ceiling: u32,
    cancelled: AtomicBool,
    state: Mutex<SendState>,
}

impl<R: BlobReader> FileSender<R> {
    pub fn new(
        transfer_id: TransferId,
        manifest: Arc<BlobManifest>,
        reader: R,
        negotiated: &NegotiatedFiles,
    ) -> Self {
        let n = manifest.chunks.len();
        Self {
            transfer_id,
            manifest,
            reader,
            ceiling: negotiated.chunk_size_ceiling,
            cancelled: AtomicBool::new(false),
            state: Mutex::new(SendState {
                paused: false,
                completed: false,
                acked: ResumeBitmap::new(n),
            }),
        }
    }

    pub fn status(&self) -> SendStatus {
        if self.cancelled.load(Ordering::Acquire) {
            return SendStatus::Cancelled;
        }
        let s = self.state.lock().expect("send state");
        if s.completed {
            SendStatus::Completed
        } else if s.paused {
            SendStatus::Paused
        } else {
            SendStatus::InFlight
        }
    }

    /// The reply to a `ManifestRequest`.
    pub fn manifest_frame(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(siar_blob_manifest::wire::encode_manifest(
            &siar_blob_manifest::wire::ManifestFrameV1 {
                transfer_id: self.transfer_id,
                manifest: (*self.manifest).clone(),
            },
        )?)
    }

    /// Serves the requested ranges as chunk frames, in order, reading
    /// each chunk by its own byte range (never the whole blob). Stops
    /// between chunks if the transfer is cancelled or paused meanwhile.
    /// Returns how many chunks were written.
    pub async fn serve<W: Write>(
        &self,
        req: &ChunkRequestV1,
        out: &mut W,
    ) -> Result<u32, ProtocolError> {
        if req.transfer_id != self.transfer_id {
            return Err(ProtocolError::UnknownTransfer(req.transfer_id));
        }
        let total = self.manifest.chunks.len() as u64;
        let mut sent = 0u32;
        for &(start, end) in &req.ranges {
            if u64::from(end) > total || start >= end {
                return Err(WireError::ChunkIndexOutOfRange {
                    index: end,
                    chunks: total,
                }
                .into());
            }
            for index in start..end {
                self.serve_chunk(index, out).await?;
                sent += 1;
            }
        }
        Ok(sent)
    }

    /// One chunk frame (header + bytes) for `index`.
    pub async fn serve_chunk<W: Write>(
        &self,
        index: u32,
        out: &mut W,
    ) -> Result<(), ProtocolError> {
        match self.status() {
            SendStatus::Cancelled => return Err(ProtocolError::Cancelled),
            SendStatus::Paused => return Err(ProtocolError::Paused),
            _ => {}
        }
        let total = self.manifest.chunks.len() as u64;
        let d = usize::try_from(index)
            .ok()
            .and_then(|i| self.manifest.chunks.get(i))
            .ok_or(WireError::ChunkIndexOutOfRange {
                index,
                chunks: total,
            })?;
        let bytes = self
            .reader
            .read_range(ByteRange::new(d.offset, u64::from(d.size)))
            .await?;
        let header = ChunkHeaderV1::for_chunk(self.transfer_id, self.manifest.blob_id, d);
        if header.size > self.ceiling {
            return Err(WireError::TooLarge {
                what: "chunk size",
                actual: u64::from(header.size),
                max: u64::from(self.ceiling),
            }
            .into());
        }
        write_chunk_frame(out, &header, &bytes)?;
        Ok(())
    }

    /// Messages a *receiver* may send. Chunk requests are answered with
    /// [`Self::serve`] by the caller, since they need an output.
    pub fn on_receiver_message(&self, msg: &FileControlV1) -> Result<SendStatus, ProtocolError> {
        if msg.transfer_id() != Some(self.transfer_id) {
            return Err(ProtocolError::Unexpected);
        }
        match msg {
            FileControlV1::Pause { .. } => {
                self.state.lock().expect("send state").paused = true;
            }
            FileControlV1::Resume(_) | FileControlV1::ChunkRequest(_) => {
                self.state.lock().expect("send state").paused = false;
            }
            FileControlV1::Cancel(_) => self.cancelled.store(true, Ordering::Release),
            FileControlV1::ChunkAck(a) => {
                let mut s = self.state.lock().expect("send state");
                for &(start, end) in &a.received_ranges {
                    for i in start..end.min(self.manifest.chunks.len() as u32) {
                        s.acked.mark_received(i);
                    }
                }
            }
            FileControlV1::Complete(c) => {
                if c.blob_id == self.manifest.blob_id && allows_transfer_completion(c.ack) {
                    self.state.lock().expect("send state").completed = true;
                }
            }
            _ => return Err(ProtocolError::Unexpected),
        }
        Ok(self.status())
    }

    pub fn acked_chunks(&self) -> usize {
        self.state
            .lock()
            .expect("send state")
            .acked
            .received_count()
    }
}
