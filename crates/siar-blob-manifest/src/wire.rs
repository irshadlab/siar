//! §132-139: the `files/1` wire layer.
//!
//! Three kinds of bytes cross the wire and each gets the cheapest
//! encoding that is still safe (§137):
//!
//! * **Control messages** ([`FileControlV1`]) — small, postcard, behind
//!   a 2-byte big-endian major version (§135). The whole frame is
//!   length-checked *before* postcard sees it, and every decoded field
//!   is re-validated afterwards (§139).
//! * **The manifest** ([`ManifestFrameV1`]) — can be large (§16), so it
//!   has its own frame and its own bound ([`ManifestLimits`]), and its
//!   internal consistency is checked ([`validate_manifest_shape`]).
//! * **Chunk data** ([`ChunkHeaderV1`] + raw bytes, §138) — a fixed
//!   88-byte header, hand-laid-out so there is no serializer in the hot
//!   path, followed by the bytes themselves. The header is validated
//!   against the manifest *before* the body buffer is allocated.
//!
//! §136: none of the domain types ([`crate::transfer_record::TransferRecord`],
//! [`FileOffer`]…) is serialized directly; the `*V1` types are the wire
//! contract and conversions are explicit.
//!
//! §133/§134: [`FilesCapabilities`] and [`negotiate`]. The receiver
//! advertises, the sender states what it wants, and the result is the
//! common compatible behaviour — never something either side did not
//! offer.
//!
//! Not here: the transport that carries these bytes (§207 Phase 6) and
//! any decision about *when* to send them — see
//! `siar-file-transfer-service::protocol`.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use siar_domain::{BlobSize, MediaType};

use crate::descriptor::{
    BlobDescriptor, ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor,
};
use crate::file_offer::{FileOffer, FileOfferDecision};
use crate::ids::{BlobId, ChunkHash, ManifestId, TransferId};
use crate::limits::ManifestLimits;
use crate::manifest::{BlobManifest, ChunkDescriptor};
use crate::transfer_control::TransferAckKind;

/// §133: the Part 01 extension name.
pub const FILES_PROTOCOL_NAME: &str = "files";
/// §135: the current stable major version. A change that old peers
/// cannot ignore becomes `files/2`, never a silent edit of `files/1`.
pub const FILES_PROTOCOL_MAJOR: u16 = 1;
/// §135 `files/1`.
pub const FILES_PROTOCOL_ID: &str = "files/1";

/// §138: transfer id (16) + blob id (32) + index (4) + size (4) + hash (32).
pub const CHUNK_HEADER_LEN: usize = 16 + 32 + 4 + 4 + 32;

/// Longest free-text reason accepted on the wire.
pub const MAX_REASON_BYTES: usize = 256;

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// Closed, `anyhow`-free (§188). Nothing here carries peer-supplied
/// bytes back out, so an error is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("frame ended before its declared content")]
    Truncated,
    #[error("{what} is {actual}, over the local limit of {max}")]
    TooLarge {
        what: &'static str,
        actual: u64,
        max: u64,
    },
    #[error("peer speaks files/{peer_major}; this build speaks files/{FILES_PROTOCOL_MAJOR}")]
    UnsupportedVersion { peer_major: u16 },
    #[error("frame is not valid files/1 content")]
    Malformed,
    #[error("{what} is not valid: {why}")]
    Invalid {
        what: &'static str,
        why: &'static str,
    },
    #[error("chunk frame belongs to a different transfer")]
    WrongTransfer,
    #[error("chunk frame belongs to a different blob")]
    WrongBlob,
    #[error("chunk index {index} is outside the manifest ({chunks} chunks)")]
    ChunkIndexOutOfRange { index: u32, chunks: u64 },
    #[error("chunk header disagrees with the manifest about {0}")]
    HeaderDisagreesWithManifest(&'static str),
    #[error("could not reserve {0} bytes for a chunk")]
    AllocationRefused(u64),
    #[error("transport error while reading or writing a frame")]
    Io,
}

/// What this build accepts before it allocates anything (§139).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireLimits {
    /// Longest control frame, version prefix included.
    pub max_control_bytes: usize,
    pub max_metadata_bytes: usize,
    /// Most `[start, end)` runs in a request or ack.
    pub max_ranges: usize,
    pub manifest: ManifestLimits,
}

impl Default for WireLimits {
    fn default() -> Self {
        Self {
            max_control_bytes: 64 * 1024,
            max_metadata_bytes: 16 * 1024,
            max_ranges: 4096,
            manifest: ManifestLimits::default(),
        }
    }
}

// ---------------------------------------------------------------------
// §139 numeric safety
// ---------------------------------------------------------------------

/// "Never cast untrusted `u64` directly to `usize`": compare with the
/// local limit first, then convert with `try_from`.
pub fn bounded_len(value: u64, limit: u64, what: &'static str) -> Result<usize, WireError> {
    if value > limit {
        return Err(WireError::TooLarge {
            what,
            actual: value,
            max: limit,
        });
    }
    usize::try_from(value).map_err(|_| WireError::TooLarge {
        what,
        actual: value,
        max: limit,
    })
}

/// The reverse direction: a local length that must fit the wire's `u32`.
pub fn wire_len_u32(len: usize, what: &'static str) -> Result<u32, WireError> {
    u32::try_from(len).map_err(|_| WireError::TooLarge {
        what,
        actual: len as u64,
        max: u64::from(u32::MAX),
    })
}

// ---------------------------------------------------------------------
// §133-134 capabilities
// ---------------------------------------------------------------------

/// §133's capability list. Unknown bits from a newer peer are dropped on
/// decode ([`FilesFeatures::from_bits_truncate`]), never an error: that
/// is what lets `files/1` grow compatible features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FilesFeatures(u32);

impl FilesFeatures {
    pub const NONE: Self = Self(0);
    pub const CHUNKING: Self = Self(1 << 0);
    pub const RESUME: Self = Self(1 << 1);
    pub const PARALLEL: Self = Self(1 << 2);
    pub const CONTENT_ADDRESSING: Self = Self(1 << 3);
    pub const PARTIAL_READ: Self = Self(1 << 4);
    pub const MULTI_SOURCE: Self = Self(1 << 5);
    const KNOWN: u32 = 0b11_1111;

    /// What every `files/1` implementation must have.
    pub const BASELINE: Self = Self(Self::CHUNKING.0 | Self::CONTENT_ADDRESSING.0);

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & Self::KNOWN)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// §134: "max chunk size, parallelism, max object size, resume support"
/// (resume lives in [`FilesFeatures::RESUME`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesCapabilities {
    pub max_chunk_size: u32,
    pub parallelism: u16,
    pub max_object_size: u64,
    pub features: FilesFeatures,
}

impl FilesCapabilities {
    /// A reasonable desktop-class advertisement.
    pub fn desktop_default() -> Self {
        Self {
            max_chunk_size: 4 * 1024 * 1024,
            parallelism: 4,
            max_object_size: ManifestLimits::default().max_file_bytes,
            features: FilesFeatures::BASELINE
                .union(FilesFeatures::RESUME)
                .union(FilesFeatures::PARALLEL)
                .union(FilesFeatures::PARTIAL_READ),
        }
    }

    /// Clamps a peer's advertisement to what this build can honour, so a
    /// peer cannot make it promise more than its own limits (§139).
    pub fn clamped_to(self, local: &WireLimits) -> Self {
        Self {
            max_object_size: self.max_object_size.min(local.manifest.max_file_bytes),
            features: FilesFeatures::from_bits_truncate(self.features.bits()),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NegotiationError {
    #[error("an advertised limit is zero")]
    ZeroLimit,
    #[error("peers share no files/1 baseline (chunking + content addressing)")]
    NoCommonBaseline,
}

/// The behaviour both sides agreed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiatedFiles {
    pub chunk_size_ceiling: u32,
    pub parallelism: u16,
    pub max_object_size: u64,
    pub features: FilesFeatures,
}

impl NegotiatedFiles {
    pub fn can_resume(&self) -> bool {
        self.features.contains(FilesFeatures::RESUME)
    }

    /// Whether an object of `total_size` cut into `chunk_size` pieces is
    /// something both sides said they can handle.
    pub fn admits(&self, total_size: u64, chunk_size: u32) -> bool {
        total_size <= self.max_object_size && chunk_size <= self.chunk_size_ceiling
    }
}

/// §134 "Sender chooses common compatible behavior": each numeric limit
/// is the smaller of the two, features are the intersection, and
/// parallelism collapses to 1 unless both sides do [`FilesFeatures::PARALLEL`].
pub fn negotiate(
    sender: &FilesCapabilities,
    receiver: &FilesCapabilities,
) -> Result<NegotiatedFiles, NegotiationError> {
    for c in [sender, receiver] {
        if c.max_chunk_size == 0 || c.parallelism == 0 || c.max_object_size == 0 {
            return Err(NegotiationError::ZeroLimit);
        }
    }
    let features = sender.features.intersection(receiver.features);
    if !features.contains(FilesFeatures::BASELINE) {
        return Err(NegotiationError::NoCommonBaseline);
    }
    let parallelism = if features.contains(FilesFeatures::PARALLEL) {
        sender.parallelism.min(receiver.parallelism)
    } else {
        1
    };
    Ok(NegotiatedFiles {
        chunk_size_ceiling: sender.max_chunk_size.min(receiver.max_chunk_size),
        parallelism,
        max_object_size: sender.max_object_size.min(receiver.max_object_size),
        features,
    })
}

// ---------------------------------------------------------------------
// §136 wire types
// ---------------------------------------------------------------------

/// Wire mirror of [`EncryptionAlgorithm`]: a new AEAD is a new variant
/// here deliberately, never an implicit renumbering of the domain enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlgorithmV1 {
    ChaCha20Poly1305,
    /// Appended, never inserted: older peers fail to decode the offer
    /// (a clean refusal) instead of misreading it.
    ChaCha20Poly1305Chunked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferDecisionV1 {
    Accept,
    Reject,
    Defer,
    MetadataOnly,
}

impl From<FileOfferDecision> for OfferDecisionV1 {
    fn from(d: FileOfferDecision) -> Self {
        match d {
            FileOfferDecision::Accept => Self::Accept,
            FileOfferDecision::Reject => Self::Reject,
            FileOfferDecision::Defer => Self::Defer,
            FileOfferDecision::MetadataOnly => Self::MetadataOnly,
        }
    }
}

impl From<OfferDecisionV1> for FileOfferDecision {
    fn from(d: OfferDecisionV1) -> Self {
        match d {
            OfferDecisionV1::Accept => Self::Accept,
            OfferDecisionV1::Reject => Self::Reject,
            OfferDecisionV1::Defer => Self::Defer,
            OfferDecisionV1::MetadataOnly => Self::MetadataOnly,
        }
    }
}

/// §44 on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOfferV1 {
    pub transfer_id: TransferId,
    pub blob_id: BlobId,
    pub size: u64,
    pub chunk_size: u32,
    pub algorithm: AlgorithmV1,
    pub nonce: [u8; 12],
    pub media_type: Option<MediaType>,
    pub manifest_id: Option<ManifestId>,
    pub encrypted_metadata: Vec<u8>,
    pub expiry_millis: Option<u64>,
}

impl From<&FileOffer> for FileOfferV1 {
    fn from(o: &FileOffer) -> Self {
        Self {
            transfer_id: o.transfer_id,
            blob_id: o.blob.blob_id,
            size: o.blob.size.bytes(),
            chunk_size: o.blob.chunking.chunk_size,
            algorithm: match o.blob.encryption.algorithm {
                EncryptionAlgorithm::ChaCha20Poly1305 => AlgorithmV1::ChaCha20Poly1305,
                EncryptionAlgorithm::ChaCha20Poly1305Chunked => {
                    AlgorithmV1::ChaCha20Poly1305Chunked
                }
            },
            nonce: o.blob.encryption.nonce,
            media_type: o.blob.media_type,
            manifest_id: o.blob.manifest_id,
            encrypted_metadata: o.encrypted_metadata.clone(),
            expiry_millis: o.expiry_millis,
        }
    }
}

impl FileOfferV1 {
    /// Wire → domain, re-validating everything the peer said (§9 "do not
    /// trust remote-declared size blindly").
    pub fn into_domain(self, limits: &WireLimits) -> Result<FileOffer, WireError> {
        self.validate(limits)?;
        let size = BlobSize::parse(self.size).map_err(|_| WireError::Invalid {
            what: "offer size",
            why: "rejected by the domain size rules",
        })?;
        Ok(FileOffer {
            transfer_id: self.transfer_id,
            blob: BlobDescriptor {
                blob_id: self.blob_id,
                size,
                chunking: ChunkingDescriptor {
                    chunk_size: self.chunk_size,
                },
                encryption: EncryptionDescriptor {
                    algorithm: match self.algorithm {
                        AlgorithmV1::ChaCha20Poly1305 => EncryptionAlgorithm::ChaCha20Poly1305,
                        AlgorithmV1::ChaCha20Poly1305Chunked => {
                            EncryptionAlgorithm::ChaCha20Poly1305Chunked
                        }
                    },
                    nonce: self.nonce,
                },
                media_type: self.media_type,
                manifest_id: self.manifest_id,
            },
            encrypted_metadata: self.encrypted_metadata,
            expiry_millis: self.expiry_millis,
        })
    }

    fn validate(&self, limits: &WireLimits) -> Result<(), WireError> {
        if self.size > limits.manifest.max_file_bytes {
            return Err(WireError::TooLarge {
                what: "offered file size",
                actual: self.size,
                max: limits.manifest.max_file_bytes,
            });
        }
        if self.chunk_size == 0 {
            return Err(WireError::Invalid {
                what: "offer chunk size",
                why: "zero",
            });
        }
        if self.encrypted_metadata.len() > limits.max_metadata_bytes {
            return Err(WireError::TooLarge {
                what: "offer metadata",
                actual: self.encrypted_metadata.len() as u64,
                max: limits.max_metadata_bytes as u64,
            });
        }
        Ok(())
    }
}

/// §132 "missing chunks": which `[start, end)` runs the receiver wants.
/// The same shape serves the first request, a resume (§31) and a retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRequestV1 {
    pub transfer_id: TransferId,
    pub ranges: Vec<(u32, u32)>,
}

/// §99 range acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkAckV1 {
    pub transfer_id: TransferId,
    pub received_ranges: Vec<(u32, u32)>,
}

/// §100: completion says *which* acknowledgement it is, so a sender can
/// apply [`crate::transfer_control::allows_transfer_completion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferCompleteV1 {
    pub transfer_id: TransferId,
    pub blob_id: BlobId,
    pub ack: TransferAckKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelV1 {
    pub transfer_id: TransferId,
    /// §101: the one real choice — keep staged chunks for a later resume.
    pub keep_partial: bool,
    pub reason: Option<String>,
}

/// §132's verbs, one variant each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileControlV1 {
    /// §134: sent by the receiver (and answered by the sender).
    Capabilities(FilesCapabilities),
    Offer(FileOfferV1),
    OfferReply {
        transfer_id: TransferId,
        decision: OfferDecisionV1,
    },
    ManifestRequest {
        transfer_id: TransferId,
    },
    ChunkRequest(ChunkRequestV1),
    ChunkAck(ChunkAckV1),
    Pause {
        transfer_id: TransferId,
    },
    /// §31: carries what is still missing, so nothing restarts.
    Resume(ChunkRequestV1),
    Cancel(CancelV1),
    Complete(TransferCompleteV1),
    /// Pull (appended, never inserted, so older peers refuse it cleanly):
    /// "I want this blob." The holder answers with an ordinary `Offer`
    /// for it, or with [`FileControlV1::BlobUnavailable`].
    BlobRequest {
        blob_id: BlobId,
    },
    /// The holder cannot or will not serve the requested blob. Says
    /// nothing about *why* (a refusal must not reveal whether a blob
    /// exists).
    BlobUnavailable {
        blob_id: BlobId,
    },
}

impl FileControlV1 {
    pub fn transfer_id(&self) -> Option<TransferId> {
        match self {
            Self::Capabilities(_) | Self::BlobRequest { .. } | Self::BlobUnavailable { .. } => None,
            Self::Offer(o) => Some(o.transfer_id),
            Self::OfferReply { transfer_id, .. }
            | Self::ManifestRequest { transfer_id }
            | Self::Pause { transfer_id } => Some(*transfer_id),
            Self::ChunkRequest(r) | Self::Resume(r) => Some(r.transfer_id),
            Self::ChunkAck(a) => Some(a.transfer_id),
            Self::Cancel(c) => Some(c.transfer_id),
            Self::Complete(c) => Some(c.transfer_id),
        }
    }

    fn validate(&self, limits: &WireLimits) -> Result<(), WireError> {
        match self {
            Self::Offer(o) => o.validate(limits),
            Self::ChunkRequest(r) | Self::Resume(r) => validate_ranges(&r.ranges, limits),
            Self::ChunkAck(a) => validate_ranges(&a.received_ranges, limits),
            Self::Cancel(c) => match &c.reason {
                Some(r) if r.len() > MAX_REASON_BYTES => Err(WireError::TooLarge {
                    what: "cancel reason",
                    actual: r.len() as u64,
                    max: MAX_REASON_BYTES as u64,
                }),
                _ => Ok(()),
            },
            Self::Capabilities(c) => {
                if c.max_chunk_size == 0 || c.parallelism == 0 || c.max_object_size == 0 {
                    return Err(WireError::Invalid {
                        what: "capabilities",
                        why: "a limit is zero",
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn validate_ranges(ranges: &[(u32, u32)], limits: &WireLimits) -> Result<(), WireError> {
    if ranges.len() > limits.max_ranges {
        return Err(WireError::TooLarge {
            what: "range count",
            actual: ranges.len() as u64,
            max: limits.max_ranges as u64,
        });
    }
    // Strictly increasing, non-empty, non-overlapping: the only form
    // `ResumeBitmap::missing_ranges` ever produces, so anything else is
    // not from an honest peer.
    let mut floor = 0u32;
    for &(start, end) in ranges {
        if start >= end || start < floor {
            return Err(WireError::Invalid {
                what: "chunk ranges",
                why: "empty, reversed, overlapping or unordered",
            });
        }
        floor = end;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// §135/§137 control framing
// ---------------------------------------------------------------------

/// `[major: u16 BE][postcard body]`.
pub fn encode_control(msg: &FileControlV1) -> Result<Vec<u8>, WireError> {
    let body = postcard::to_allocvec(msg).map_err(|_| WireError::Malformed)?;
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&FILES_PROTOCOL_MAJOR.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Length and version are checked before any deserialization; every
/// decoded field is validated after it.
pub fn decode_control(frame: &[u8], limits: &WireLimits) -> Result<FileControlV1, WireError> {
    if frame.len() > limits.max_control_bytes {
        return Err(WireError::TooLarge {
            what: "control frame",
            actual: frame.len() as u64,
            max: limits.max_control_bytes as u64,
        });
    }
    let (major, body) = split_version(frame)?;
    check_major(major)?;
    let msg: FileControlV1 = postcard::from_bytes(body).map_err(|_| WireError::Malformed)?;
    msg.validate(limits)?;
    Ok(msg)
}

fn split_version(frame: &[u8]) -> Result<(u16, &[u8]), WireError> {
    match frame {
        [a, b, rest @ ..] => Ok((u16::from_be_bytes([*a, *b]), rest)),
        _ => Err(WireError::Truncated),
    }
}

fn check_major(peer_major: u16) -> Result<(), WireError> {
    if peer_major == FILES_PROTOCOL_MAJOR {
        Ok(())
    } else {
        Err(WireError::UnsupportedVersion { peer_major })
    }
}

// ---------------------------------------------------------------------
// Manifest frame
// ---------------------------------------------------------------------

/// `[major u16][transfer_id 16][postcard BlobManifest]`.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestFrameV1 {
    pub transfer_id: TransferId,
    pub manifest: BlobManifest,
}

pub fn encode_manifest(frame: &ManifestFrameV1) -> Result<Vec<u8>, WireError> {
    let body = postcard::to_allocvec(&frame.manifest).map_err(|_| WireError::Malformed)?;
    let mut out = Vec::with_capacity(2 + 16 + body.len());
    out.extend_from_slice(&FILES_PROTOCOL_MAJOR.to_be_bytes());
    out.extend_from_slice(frame.transfer_id.as_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Bounded by [`ManifestLimits::max_manifest_bytes`], then checked for
/// internal consistency with [`validate_manifest_shape`].
pub fn decode_manifest(bytes: &[u8], limits: &WireLimits) -> Result<ManifestFrameV1, WireError> {
    let max = bounded_len(
        limits.manifest.max_manifest_bytes,
        u64::MAX,
        "manifest limit",
    )?;
    if bytes.len() > max.saturating_add(18) {
        return Err(WireError::TooLarge {
            what: "manifest frame",
            actual: bytes.len() as u64,
            max: limits.manifest.max_manifest_bytes,
        });
    }
    let (major, rest) = split_version(bytes)?;
    check_major(major)?;
    let Some((id_bytes, body)) = rest.split_first_chunk::<16>() else {
        return Err(WireError::Truncated);
    };
    let transfer_id = TransferId::from_raw_bytes(*id_bytes);
    let manifest: BlobManifest = postcard::from_bytes(body).map_err(|_| WireError::Malformed)?;
    validate_manifest_shape(&manifest, &limits.manifest)?;
    Ok(ManifestFrameV1 {
        transfer_id,
        manifest,
    })
}

/// Which transfer a manifest frame is for, without decoding the (possibly
/// large) manifest — so a caller can look up its context first.
pub fn peek_manifest_transfer_id(frame: &[u8]) -> Result<TransferId, WireError> {
    let (_, rest) = split_version(frame)?;
    rest.first_chunk::<16>()
        .map(|b| TransferId::from_raw_bytes(*b))
        .ok_or(WireError::Truncated)
}

/// Which transfer a chunk frame is for: the first 16 bytes of its header.
pub fn peek_chunk_transfer_id(frame: &[u8]) -> Result<TransferId, WireError> {
    frame
        .first_chunk::<16>()
        .map(|b| TransferId::from_raw_bytes(*b))
        .ok_or(WireError::Truncated)
}

/// A manifest from a peer is a claim. Before anything is allocated or
/// written against it: counts and sizes within local limits, chunks
/// numbered in order, contiguous, never larger than the declared chunk
/// size, and summing to exactly the declared total. (Authenticity — that
/// it matches the offered `BlobId` — is
/// [`crate::source_trust::verify_manifest_authenticity`]'s job.)
pub fn validate_manifest_shape(
    manifest: &BlobManifest,
    limits: &ManifestLimits,
) -> Result<(), WireError> {
    let invalid = |why| WireError::Invalid {
        what: "manifest",
        why,
    };
    if manifest.total_size > limits.max_file_bytes {
        return Err(WireError::TooLarge {
            what: "manifest total size",
            actual: manifest.total_size,
            max: limits.max_file_bytes,
        });
    }
    if manifest.chunks.len() as u64 > u64::from(limits.max_chunks) {
        return Err(WireError::TooLarge {
            what: "manifest chunk count",
            actual: manifest.chunks.len() as u64,
            max: u64::from(limits.max_chunks),
        });
    }
    if manifest.chunk_size == 0 {
        return Err(invalid("chunk size is zero"));
    }
    let mut offset = 0u64;
    for (position, chunk) in manifest.chunks.iter().enumerate() {
        if chunk.index as usize != position {
            return Err(invalid("chunks are not numbered in order"));
        }
        if chunk.offset != offset {
            return Err(invalid("chunks are not contiguous"));
        }
        if chunk.size == 0 || chunk.size > manifest.chunk_size {
            return Err(invalid("a chunk is empty or over the declared chunk size"));
        }
        offset = offset
            .checked_add(u64::from(chunk.size))
            .ok_or_else(|| invalid("chunk sizes overflow"))?;
    }
    if offset != manifest.total_size {
        return Err(invalid("chunk sizes do not sum to the total size"));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// §138 chunk frame
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeaderV1 {
    pub transfer_id: TransferId,
    pub blob_id: BlobId,
    pub chunk_index: u32,
    pub size: u32,
    pub hash: ChunkHash,
}

impl ChunkHeaderV1 {
    /// For a chunk the sender is about to ship.
    pub fn for_chunk(transfer_id: TransferId, blob_id: BlobId, d: &ChunkDescriptor) -> Self {
        Self {
            transfer_id,
            blob_id,
            chunk_index: d.index,
            size: d.size,
            hash: d.hash,
        }
    }

    /// Fixed layout, big-endian, no serializer.
    pub fn to_bytes(&self) -> [u8; CHUNK_HEADER_LEN] {
        let mut out = [0u8; CHUNK_HEADER_LEN];
        out[0..16].copy_from_slice(self.transfer_id.as_bytes());
        out[16..48].copy_from_slice(self.blob_id.as_bytes());
        out[48..52].copy_from_slice(&self.chunk_index.to_be_bytes());
        out[52..56].copy_from_slice(&self.size.to_be_bytes());
        out[56..88].copy_from_slice(self.hash.as_bytes());
        out
    }

    /// Every bit pattern parses; *meaning* is [`ChunkHeaderV1::validate`].
    pub fn from_bytes(b: &[u8; CHUNK_HEADER_LEN]) -> Self {
        let arr16 = |s: &[u8]| <[u8; 16]>::try_from(s).expect("fixed slice");
        let arr32 = |s: &[u8]| <[u8; 32]>::try_from(s).expect("fixed slice");
        let word = |s: &[u8]| u32::from_be_bytes(<[u8; 4]>::try_from(s).expect("fixed slice"));
        Self {
            transfer_id: TransferId::from_raw_bytes(arr16(&b[0..16])),
            blob_id: BlobId::from_raw_digest(arr32(&b[16..48])),
            chunk_index: word(&b[48..52]),
            size: word(&b[52..56]),
            hash: ChunkHash::from_raw_digest(arr32(&b[56..88])),
        }
    }

    /// §138 "Validate before allocating": a header is acceptable only if
    /// it names this blob, an index inside the manifest, the size and
    /// hash the manifest gives for that index, and a size within the
    /// negotiated ceiling. Returns the manifest's own descriptor.
    pub fn validate<'m>(
        &self,
        manifest: &'m BlobManifest,
        max_chunk_size: u32,
    ) -> Result<&'m ChunkDescriptor, WireError> {
        if self.blob_id != manifest.blob_id {
            return Err(WireError::WrongBlob);
        }
        let chunks = manifest.chunks.len() as u64;
        let out_of_range = WireError::ChunkIndexOutOfRange {
            index: self.chunk_index,
            chunks,
        };
        let index = usize::try_from(self.chunk_index).map_err(|_| out_of_range)?;
        let Some(descriptor) = manifest.chunks.get(index) else {
            return Err(out_of_range);
        };
        if self.size > max_chunk_size {
            return Err(WireError::TooLarge {
                what: "chunk size",
                actual: u64::from(self.size),
                max: u64::from(max_chunk_size),
            });
        }
        if self.size != descriptor.size {
            return Err(WireError::HeaderDisagreesWithManifest("size"));
        }
        if self.hash != descriptor.hash {
            return Err(WireError::HeaderDisagreesWithManifest("hash"));
        }
        Ok(descriptor)
    }
}

/// Header, then the bytes. The body length is the header's, and a body
/// of any other length is a caller bug caught here rather than on the
/// peer.
pub fn write_chunk_frame<W: Write>(
    out: &mut W,
    header: &ChunkHeaderV1,
    body: &[u8],
) -> Result<(), WireError> {
    if u32::try_from(body.len()) != Ok(header.size) {
        return Err(WireError::HeaderDisagreesWithManifest("body length"));
    }
    out.write_all(&header.to_bytes())
        .and_then(|()| out.write_all(body))
        .map_err(|_| WireError::Io)
}

/// Reads one chunk frame. The header is read into a stack array and
/// validated against `manifest` first; only then is `size` bytes (now
/// known to be ≤ `max_chunk_size`) reserved for the body, with
/// `try_reserve_exact` so an allocator refusal is an error, not an abort.
///
/// The body's *hash* is deliberately not recomputed here: the writer
/// ([`crate::blob_io::BlobWriter::write_chunk`]) verifies it before the
/// bytes touch storage, and hashing twice is pure overhead.
pub fn read_chunk_frame<R: Read>(
    input: &mut R,
    expected_transfer: TransferId,
    manifest: &BlobManifest,
    max_chunk_size: u32,
) -> Result<(ChunkHeaderV1, Vec<u8>), WireError> {
    let mut raw = [0u8; CHUNK_HEADER_LEN];
    read_exact_or_truncated(input, &mut raw)?;
    let header = ChunkHeaderV1::from_bytes(&raw);
    if header.transfer_id != expected_transfer {
        return Err(WireError::WrongTransfer);
    }
    header.validate(manifest, max_chunk_size)?;
    let len = bounded_len(
        u64::from(header.size),
        u64::from(max_chunk_size),
        "chunk size",
    )?;
    let mut body = Vec::new();
    body.try_reserve_exact(len)
        .map_err(|_| WireError::AllocationRefused(len as u64))?;
    body.resize(len, 0);
    read_exact_or_truncated(input, &mut body)?;
    Ok((header, body))
}

fn read_exact_or_truncated<R: Read>(input: &mut R, buf: &mut [u8]) -> Result<(), WireError> {
    input.read_exact(buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            WireError::Truncated
        } else {
            WireError::Io
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;
    use crate::manifest::build_manifest;

    fn manifest_of(len: usize, chunk: u32) -> (Vec<u8>, BlobManifest) {
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let m = build_manifest(&data, chunk, &ManifestLimits::default()).unwrap();
        (data, m)
    }

    fn limits() -> WireLimits {
        WireLimits::default()
    }

    // ---- §139 ----

    #[test]
    fn bounded_len_checks_the_limit_before_converting() {
        assert_eq!(bounded_len(10, 10, "x"), Ok(10));
        assert!(matches!(
            bounded_len(11, 10, "x"),
            Err(WireError::TooLarge { actual: 11, .. })
        ));
        assert!(bounded_len(u64::MAX, u64::MAX - 1, "x").is_err());
    }

    #[test]
    fn wire_len_u32_refuses_what_does_not_fit() {
        assert_eq!(wire_len_u32(7, "x"), Ok(7));
        if usize::BITS > 32 {
            assert!(wire_len_u32(usize::MAX, "x").is_err());
        }
    }

    // ---- §133/§134 ----

    #[test]
    fn negotiation_takes_the_smaller_limits_and_the_common_features() {
        let sender = FilesCapabilities::desktop_default();
        let receiver = FilesCapabilities {
            max_chunk_size: 64 * 1024,
            parallelism: 2,
            max_object_size: 1 << 20,
            features: FilesFeatures::BASELINE.union(FilesFeatures::PARALLEL),
        };
        let n = negotiate(&sender, &receiver).unwrap();
        assert_eq!(n.chunk_size_ceiling, 64 * 1024);
        assert_eq!(n.parallelism, 2);
        assert_eq!(n.max_object_size, 1 << 20);
        assert!(!n.can_resume(), "receiver never offered resume");
        assert!(n.admits(1 << 20, 64 * 1024));
        assert!(!n.admits((1 << 20) + 1, 64 * 1024));
        assert!(!n.admits(10, 64 * 1024 + 1));
    }

    #[test]
    fn parallelism_is_one_unless_both_sides_do_it() {
        let mut a = FilesCapabilities::desktop_default();
        let mut b = FilesCapabilities::desktop_default();
        b.features = FilesFeatures::BASELINE;
        assert_eq!(negotiate(&a, &b).unwrap().parallelism, 1);
        a.features = FilesFeatures::BASELINE;
        assert_eq!(negotiate(&a, &b).unwrap().parallelism, 1);
    }

    #[test]
    fn negotiation_refuses_zero_limits_and_a_missing_baseline() {
        let good = FilesCapabilities::desktop_default();
        let mut zero = good;
        zero.parallelism = 0;
        assert_eq!(negotiate(&good, &zero), Err(NegotiationError::ZeroLimit));
        let mut no_cas = good;
        no_cas.features = FilesFeatures::CHUNKING.union(FilesFeatures::RESUME);
        assert_eq!(
            negotiate(&good, &no_cas),
            Err(NegotiationError::NoCommonBaseline)
        );
    }

    #[test]
    fn unknown_feature_bits_from_a_newer_peer_are_dropped_not_fatal() {
        let f = FilesFeatures::from_bits_truncate(u32::MAX);
        assert_eq!(f.bits(), 0b11_1111);
    }

    #[test]
    fn a_peers_advertisement_is_clamped_to_local_limits() {
        let mut c = FilesCapabilities::desktop_default();
        c.max_object_size = u64::MAX;
        let clamped = c.clamped_to(&limits());
        assert_eq!(clamped.max_object_size, limits().manifest.max_file_bytes);
    }

    // ---- §135/§137 control framing ----

    fn sample_requests() -> Vec<FileControlV1> {
        let t = TransferId::new();
        vec![
            FileControlV1::Capabilities(FilesCapabilities::desktop_default()),
            FileControlV1::OfferReply {
                transfer_id: t,
                decision: OfferDecisionV1::Accept,
            },
            FileControlV1::ManifestRequest { transfer_id: t },
            FileControlV1::ChunkRequest(ChunkRequestV1 {
                transfer_id: t,
                ranges: vec![(0, 3), (5, 9)],
            }),
            FileControlV1::ChunkAck(ChunkAckV1 {
                transfer_id: t,
                received_ranges: vec![(0, 4)],
            }),
            FileControlV1::Pause { transfer_id: t },
            FileControlV1::Resume(ChunkRequestV1 {
                transfer_id: t,
                ranges: vec![(3, 5)],
            }),
            FileControlV1::Cancel(CancelV1 {
                transfer_id: t,
                keep_partial: true,
                reason: Some("user".into()),
            }),
            FileControlV1::Complete(TransferCompleteV1 {
                transfer_id: t,
                blob_id: BlobId::from_ciphertext(b"x"),
                ack: TransferAckKind::BlobVerified,
            }),
            FileControlV1::BlobRequest {
                blob_id: BlobId::from_ciphertext(b"want"),
            },
            FileControlV1::BlobUnavailable {
                blob_id: BlobId::from_ciphertext(b"nope"),
            },
        ]
    }

    #[test]
    fn every_control_message_round_trips() {
        for msg in sample_requests() {
            let bytes = encode_control(&msg).unwrap();
            assert_eq!(&bytes[..2], &FILES_PROTOCOL_MAJOR.to_be_bytes());
            assert_eq!(decode_control(&bytes, &limits()).unwrap(), msg);
        }
    }

    #[test]
    fn a_foreign_major_version_is_refused_before_decoding() {
        let mut bytes = encode_control(&sample_requests()[0]).unwrap();
        bytes[0..2].copy_from_slice(&2u16.to_be_bytes());
        assert_eq!(
            decode_control(&bytes, &limits()),
            Err(WireError::UnsupportedVersion { peer_major: 2 })
        );
    }

    #[test]
    fn an_oversized_control_frame_is_refused_before_decoding() {
        let big = vec![0u8; limits().max_control_bytes + 1];
        assert!(matches!(
            decode_control(&big, &limits()),
            Err(WireError::TooLarge {
                what: "control frame",
                ..
            })
        ));
    }

    #[test]
    fn short_and_garbage_frames_are_errors_not_panics() {
        assert_eq!(decode_control(&[], &limits()), Err(WireError::Truncated));
        assert_eq!(decode_control(&[0], &limits()), Err(WireError::Truncated));
        let mut junk = FILES_PROTOCOL_MAJOR.to_be_bytes().to_vec();
        junk.extend_from_slice(&[0xFF; 40]);
        assert!(decode_control(&junk, &limits()).is_err());
    }

    #[test]
    fn range_lists_must_be_ordered_non_empty_and_bounded() {
        let t = TransferId::new();
        for bad in [
            vec![(3, 3)],
            vec![(5, 2)],
            vec![(0, 5), (4, 8)],
            vec![(6, 8), (0, 2)],
        ] {
            let msg = FileControlV1::ChunkRequest(ChunkRequestV1 {
                transfer_id: t,
                ranges: bad,
            });
            let bytes = encode_control(&msg).unwrap();
            assert!(matches!(
                decode_control(&bytes, &limits()),
                Err(WireError::Invalid { .. })
            ));
        }
        let tight = WireLimits {
            max_ranges: 1,
            ..limits()
        };
        let msg = FileControlV1::ChunkAck(ChunkAckV1 {
            transfer_id: t,
            received_ranges: vec![(0, 1), (2, 3)],
        });
        assert!(matches!(
            decode_control(&encode_control(&msg).unwrap(), &tight),
            Err(WireError::TooLarge { .. })
        ));
    }

    #[test]
    fn an_overlong_cancel_reason_is_refused() {
        let msg = FileControlV1::Cancel(CancelV1 {
            transfer_id: TransferId::new(),
            keep_partial: false,
            reason: Some("x".repeat(MAX_REASON_BYTES + 1)),
        });
        assert!(decode_control(&encode_control(&msg).unwrap(), &limits()).is_err());
    }

    #[test]
    fn transfer_id_is_reported_for_every_message_but_capabilities() {
        for msg in sample_requests() {
            let is_caps = matches!(
                msg,
                FileControlV1::Capabilities(_)
                    | FileControlV1::BlobRequest { .. }
                    | FileControlV1::BlobUnavailable { .. }
            );
            assert_eq!(msg.transfer_id().is_none(), is_caps);
        }
    }

    // ---- §136 offer ----

    fn offer() -> FileOffer {
        FileOffer {
            transfer_id: TransferId::new(),
            blob: BlobDescriptor {
                blob_id: BlobId::from_ciphertext(b"ct"),
                size: BlobSize::parse(1234).unwrap(),
                chunking: ChunkingDescriptor { chunk_size: 512 },
                encryption: EncryptionDescriptor {
                    algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                    nonce: [9; 12],
                },
                media_type: None,
                manifest_id: Some(ManifestId::new()),
            },
            encrypted_metadata: vec![1, 2, 3],
            expiry_millis: Some(77),
        }
    }

    #[test]
    fn an_offer_survives_domain_to_wire_to_domain() {
        let o = offer();
        let wire = FileControlV1::Offer(FileOfferV1::from(&o));
        let decoded = decode_control(&encode_control(&wire).unwrap(), &limits()).unwrap();
        let FileControlV1::Offer(w) = decoded else {
            panic!("wrong variant")
        };
        assert_eq!(w.into_domain(&limits()).unwrap(), o);
    }

    #[test]
    fn an_offer_over_local_limits_never_becomes_a_domain_offer() {
        let mut w = FileOfferV1::from(&offer());
        w.size = limits().manifest.max_file_bytes + 1;
        assert!(w.into_domain(&limits()).is_err());
        let mut w = FileOfferV1::from(&offer());
        w.chunk_size = 0;
        assert!(w.into_domain(&limits()).is_err());
        let mut w = FileOfferV1::from(&offer());
        w.encrypted_metadata = vec![0; limits().max_metadata_bytes + 1];
        assert!(w.into_domain(&limits()).is_err());
    }

    // ---- manifest frame ----

    #[test]
    fn a_manifest_round_trips_through_its_own_frame() {
        let (_, m) = manifest_of(10_000, 1024);
        let frame = ManifestFrameV1 {
            transfer_id: TransferId::new(),
            manifest: m,
        };
        let bytes = encode_manifest(&frame).unwrap();
        assert_eq!(decode_manifest(&bytes, &limits()).unwrap(), frame);
    }

    #[test]
    fn a_truncated_manifest_frame_is_truncated_not_a_panic() {
        assert_eq!(
            decode_manifest(&[0, 1], &limits()),
            Err(WireError::Truncated)
        );
        assert_eq!(
            decode_manifest(&[0, 1, 9, 9], &limits()),
            Err(WireError::Truncated)
        );
    }

    #[test]
    fn manifest_shape_rejects_every_kind_of_lie() {
        let (_, good) = manifest_of(4096, 1024);
        let l = ManifestLimits::default();
        assert!(validate_manifest_shape(&good, &l).is_ok());

        let mut m = good.clone();
        m.chunks[1].index = 7;
        assert!(validate_manifest_shape(&m, &l).is_err(), "numbering");

        let mut m = good.clone();
        m.chunks[2].offset += 1;
        assert!(validate_manifest_shape(&m, &l).is_err(), "contiguity");

        let mut m = good.clone();
        m.chunks[0].size = 2048;
        assert!(validate_manifest_shape(&m, &l).is_err(), "over chunk_size");

        let mut m = good.clone();
        m.chunks[3].size = 0;
        assert!(validate_manifest_shape(&m, &l).is_err(), "empty chunk");

        let mut m = good.clone();
        m.total_size += 1;
        assert!(validate_manifest_shape(&m, &l).is_err(), "sum mismatch");

        let mut m = good.clone();
        m.chunk_size = 0;
        assert!(validate_manifest_shape(&m, &l).is_err(), "zero chunk size");

        let tight = ManifestLimits { max_chunks: 2, ..l };
        assert!(
            validate_manifest_shape(&good, &tight).is_err(),
            "count limit"
        );
        let tight = ManifestLimits {
            max_file_bytes: 10,
            ..l
        };
        assert!(
            validate_manifest_shape(&good, &tight).is_err(),
            "size limit"
        );
    }

    // ---- §138 chunk frame ----

    #[test]
    fn chunk_header_layout_is_fixed_and_round_trips() {
        let (_, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let h = ChunkHeaderV1::for_chunk(t, m.blob_id, &m.chunks[2]);
        let raw = h.to_bytes();
        assert_eq!(raw.len(), CHUNK_HEADER_LEN);
        assert_eq!(&raw[48..52], &2u32.to_be_bytes());
        assert_eq!(ChunkHeaderV1::from_bytes(&raw), h);
    }

    #[test]
    fn a_frame_round_trips_and_the_body_matches_the_manifest() {
        let (data, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let d = &m.chunks[1];
        let body = &data[d.offset as usize..(d.offset + u64::from(d.size)) as usize];
        let mut wire = Vec::new();
        write_chunk_frame(&mut wire, &ChunkHeaderV1::for_chunk(t, m.blob_id, d), body).unwrap();
        let (h, got) = read_chunk_frame(&mut wire.as_slice(), t, &m, 1024).unwrap();
        assert_eq!(h.chunk_index, 1);
        assert_eq!(got, body);
        assert!(crate::verify::verify_chunk(&got, d));
    }

    fn frame_with(h: &ChunkHeaderV1, body: &[u8]) -> Vec<u8> {
        let mut v = h.to_bytes().to_vec();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn a_header_is_validated_before_any_body_is_allocated() {
        let (_, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let good = ChunkHeaderV1::for_chunk(t, m.blob_id, &m.chunks[0]);

        // Header claims a huge body but the stream has none: refused on
        // size, never reaching the allocation.
        let mut huge = good;
        huge.size = u32::MAX;
        assert!(matches!(
            read_chunk_frame(&mut frame_with(&huge, &[]).as_slice(), t, &m, 1024),
            Err(WireError::TooLarge { .. })
        ));

        let mut wrong_blob = good;
        wrong_blob.blob_id = BlobId::from_ciphertext(b"other");
        assert_eq!(
            read_chunk_frame(&mut frame_with(&wrong_blob, &[]).as_slice(), t, &m, 1024)
                .unwrap_err(),
            WireError::WrongBlob
        );

        assert_eq!(
            read_chunk_frame(
                &mut frame_with(&good, &[]).as_slice(),
                TransferId::new(),
                &m,
                1024
            )
            .unwrap_err(),
            WireError::WrongTransfer
        );

        let mut far = good;
        far.chunk_index = u32::MAX;
        assert!(matches!(
            read_chunk_frame(&mut frame_with(&far, &[]).as_slice(), t, &m, 1024),
            Err(WireError::ChunkIndexOutOfRange { .. })
        ));

        let mut lying_size = good;
        lying_size.size -= 1;
        assert_eq!(
            read_chunk_frame(&mut frame_with(&lying_size, &[]).as_slice(), t, &m, 1024)
                .unwrap_err(),
            WireError::HeaderDisagreesWithManifest("size")
        );

        let mut lying_hash = good;
        lying_hash.hash = ChunkHash::from_ciphertext_chunk(b"nope");
        assert_eq!(
            read_chunk_frame(&mut frame_with(&lying_hash, &[]).as_slice(), t, &m, 1024)
                .unwrap_err(),
            WireError::HeaderDisagreesWithManifest("hash")
        );
    }

    #[test]
    fn the_negotiated_ceiling_can_be_lower_than_the_manifests_chunk_size() {
        let (_, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let h = ChunkHeaderV1::for_chunk(t, m.blob_id, &m.chunks[0]);
        assert!(matches!(
            read_chunk_frame(&mut frame_with(&h, &[]).as_slice(), t, &m, 512),
            Err(WireError::TooLarge { .. })
        ));
    }

    #[test]
    fn a_short_header_or_short_body_is_truncated() {
        let (data, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let h = ChunkHeaderV1::for_chunk(t, m.blob_id, &m.chunks[0]);
        assert_eq!(
            read_chunk_frame(&mut [0u8; 10].as_slice(), t, &m, 1024).unwrap_err(),
            WireError::Truncated
        );
        let half = &data[..m.chunks[0].size as usize / 2];
        assert_eq!(
            read_chunk_frame(&mut frame_with(&h, half).as_slice(), t, &m, 1024).unwrap_err(),
            WireError::Truncated
        );
    }

    #[test]
    fn writing_a_body_of_the_wrong_length_is_refused_locally() {
        let (_, m) = manifest_of(3000, 1024);
        let h = ChunkHeaderV1::for_chunk(TransferId::new(), m.blob_id, &m.chunks[0]);
        let mut sink = Vec::new();
        assert!(write_chunk_frame(&mut sink, &h, &[1, 2, 3]).is_err());
        assert!(sink.is_empty(), "nothing was written");
    }

    #[test]
    fn a_corrupt_body_passes_the_frame_but_not_chunk_verification() {
        // The frame layer checks structure; hashing is the writer's job.
        let (data, m) = manifest_of(3000, 1024);
        let t = TransferId::new();
        let d = &m.chunks[0];
        let mut body = data[..d.size as usize].to_vec();
        body[0] ^= 0xFF;
        let mut wire = Vec::new();
        write_chunk_frame(&mut wire, &ChunkHeaderV1::for_chunk(t, m.blob_id, d), &body).unwrap();
        let (_, got) = read_chunk_frame(&mut wire.as_slice(), t, &m, 1024).unwrap();
        assert!(!crate::verify::verify_chunk(&got, d));
    }
}
