#![forbid(unsafe_code)]

//! Spec 05 §201-205: the public file API.
//!
//! ```text
//! applications / siar-messaging
//!        │   send_file · next_offer · import_attachment · open_attachment
//!        ▼
//!     FilesService ──► FileChannel (the transport, supplied by the host)
//!        │
//!        ├─ siar-file-transfer-service  (files/1 protocol, engine, GC)
//!        └─ siar-blob-manifest          (blob store, references, wire)
//! ```
//!
//! **§202/§205 (file-only):** [`FilesService::send_file`] needs a
//! [`Destination`] and a [`FileSource`], never a conversation, and hands
//! back a [`SendHandle`]; the receiving side calls
//! [`FilesService::next_offer`] and answers with
//! [`IncomingOffer::accept`]. Nobody touches a chunk.
//!
//! **§203 (blob level):** [`FilesService::import_attachment`] /
//! [`FilesService::open_attachment`] are the lower-level pair, kept
//! apart from the transfer API.
//!
//! **§204 (messaging):** [`ImportedAttachment::reference`] yields the
//! `siar_domain::AttachmentReference` a message carries. It is wire- and
//! storage-compatible with the existing `siar_crypto` attachments (both
//! are `nonce ‖ ChaCha20-Poly1305` and BLAKE3 of the ciphertext), so a
//! message sent this way is fetched by today's `fetch_attachment`
//! unchanged; whether bytes then move eagerly or on request is the
//! caller's policy, exactly as §204 says.
//!
//! **§120-129:** sources and sinks are the async [`FileSource`] /
//! [`FileSink`] traits (platform adapters implement them; the core never
//! assumes a path), [`FilesService::export_attachment`] writes the
//! decrypted file to a sink (§125-126), the optional plaintext cache is
//! off by default (§127), and [`FilesService::open_verified_attachment`]
//! returns [`Verified`] bytes for media decoders (§129).
//!
//! Known limit, inherited and surfaced rather than hidden: the domain's
//! `BlobSize` caps a single described blob at
//! `siar_domain::MAX_ATTACHMENT_BYTES` (200 MiB), well under the 10 GiB
//! `ManifestLimits` default, so larger files are refused with
//! [`FilesError::AttachmentTooLarge`] until that constant is raised.
//! Whole-blob AEAD also means an import holds the file in memory once.

mod access_ledger;
mod import;
mod manifest_cache;
mod offer_meta;
mod service;

use std::future::Future;
use std::pin::Pin;

use siar_blob_manifest::encryption::EncryptionError;
use siar_blob_manifest::manifest::ManifestError;
use siar_blob_manifest::store_error::BlobError;
use siar_blob_manifest::transfer_state::TransferState;
use siar_blob_manifest::wire::{NegotiationError, WireError};
use siar_domain::{BlobSizeError, DeviceId};
use siar_file_transfer_service::protocol::ProtocolError;
use siar_file_transfer_service::transfer_engine::EngineError;
use thiserror::Error;

pub use access_ledger::{AccessLedger, MemoryAccessLedger};
pub use import::ImportedAttachment;
pub use offer_meta::ReceivedFile;
pub use service::{
    AutoDownload, DestinationPolicy, FileSendPolicy, FilesConfig, FilesService, IncomingOffer,
    KeyDelivery, KeyStatus, SendHandle,
};
pub use siar_blob_manifest::file_io::{
    FileError, FileSink, FileSource, FsFileSink, FsFileSource, MemorySink,
};
pub use siar_blob_manifest::key_wrapping::{Revocation, RevocationEffect};
pub use siar_blob_manifest::preprocess::Verified;

/// §205 `Destination::Device(device_id)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    Device(DeviceId),
}

impl Destination {
    pub fn device(&self) -> DeviceId {
        match self {
            Self::Device(d) => *d,
        }
    }
}

/// What kind of bytes a frame carries; a one-byte tag the transport
/// delivers alongside the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 1,
    Manifest = 2,
    Chunk = 3,
}

impl FrameKind {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::Control),
            2 => Some(Self::Manifest),
            3 => Some(Self::Chunk),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ChannelError {
    #[error("peer is unreachable")]
    Unreachable,
    #[error("channel is closed")]
    Closed,
}

/// The transport, supplied by the host. Delivery is assumed in order per
/// peer (a QUIC stream, a BLE connection); `files/1` reorders nothing.
pub trait FileChannel: Send + Sync {
    fn send_frame(
        &self,
        to: DeviceId,
        kind: FrameKind,
        frame: Vec<u8>,
    ) -> impl Future<Output = Result<(), ChannelError>> + Send;
}

/// Closed, `anyhow`-free (§188).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FilesError {
    #[error("file is {actual} bytes, over the {max}-byte limit")]
    TooLarge { actual: u64, max: u64 },
    #[error(transparent)]
    File(#[from] FileError),
    #[error("file is too large to describe as an attachment: {0}")]
    AttachmentTooLarge(#[from] BlobSizeError),
    #[error(transparent)]
    Encryption(#[from] EncryptionError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    Blob(#[from] BlobError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error("chunk-wise (v2) blob failed authentication, was truncated, or is malformed")]
    Chunked,
    #[error("file name is over the 255-byte limit")]
    FileNameTooLong,
    #[error("the peer does not have, or will not serve, that blob")]
    BlobUnavailable,
    #[error("the fetch did not complete in time")]
    FetchTimeout,
    #[error("the peer sent no answer at all to the request")]
    NoResponse,
    #[error("the transfer was cancelled or failed before the blob arrived")]
    FetchFailed,
    #[error("no pairwise secret is available to wrap a key for {0:?}")]
    NoPairwiseSecret(DeviceId),
    #[error("access to this blob has been revoked for that device")]
    AccessRevoked,
    #[error("sending to several recipients requires KeyDelivery::Wrapped")]
    KeyMustBeWrapped,
    #[error("key wrapping needs this service to know its own device id")]
    WrappingUnavailable,
    #[error("could not wrap the blob key")]
    WrapFailed,
    #[error("no capabilities exchanged with {0:?} yet")]
    NoCapabilities(DeviceId),
    #[error("transfer is not known to this service")]
    UnknownTransfer,
    #[error("message is not valid from this peer or in this state")]
    Unexpected,
    #[error("unknown frame kind {0}")]
    UnknownFrameKind(u8),
    #[error("the offer queue is closed")]
    OfferQueueClosed,
}

/// Where a locally-stored blob's ciphertext can be read by hash — what a
/// blob-serving transport (`siar_transport::BlobStore`) needs. Implemented
/// by [`FilesService`]; the adapter lives in siar-messaging.
pub trait CiphertextSource: Send + Sync {
    fn ciphertext(&self, blob_hash: &[u8; 32]) -> Option<Vec<u8>>;
}

/// Decides whether a paired peer may *pull* a stored blob by its id.
/// The default (no gate configured) serves any blob that is currently
/// referenced — i.e. one this device still considers live — because a
/// blob id is a hash of ciphertext and is not guessable; install a gate to
/// narrow that (per-conversation membership, share lists…).
pub trait ServeGate: Send + Sync {
    fn may_serve(&self, peer: DeviceId, blob: siar_blob_manifest::ids::BlobId) -> bool;
}

/// Where wrapping secrets come from. The host owns the long-term keys
/// (this crate never sees them); for each peer it returns the pairwise
/// secret both ends can compute — in this workspace
/// `siar_crypto::FileKeyWrapSecret`, via `siar-messaging`'s adapter.
pub trait PairwiseSecrets: Send + Sync {
    /// `None` if no secret is known for `peer` (not paired, no key yet).
    fn secret_for(&self, peer: DeviceId) -> Option<zeroize::Zeroizing<[u8; 32]>>;
}

pub(crate) type BoxFut<'a> = Pin<Box<dyn Future<Output = Result<(), FilesError>> + Send + 'a>>;

/// Final state of a transfer as the sender sees it.
pub type SendState = TransferState;
