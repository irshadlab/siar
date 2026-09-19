//! §5 "Blob Concept" through §8 "Blob Identity Layers", §14 "Chunk
//! Identity", §19 "Per-Blob Key".

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// §5, §6, §7: "Encrypt first, then content-address the ciphertext" —
/// this crate's own recommended choice between the two options §7
/// lays out, so [`BlobId::from_ciphertext`] is the only constructor,
/// deliberately named to make that choice visible at every call site
/// rather than a bare `BlobId::new(bytes)` that doesn't say which kind
/// of bytes it expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobId([u8; 32]);

impl BlobId {
    pub fn from_ciphertext(ciphertext: &[u8]) -> Self {
        Self(*blake3::hash(ciphertext).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// §14: computed the same way as [`BlobId`] — a content hash of one
/// chunk's own ciphertext bytes, so [`crate::verify::verify_chunk`] can
/// check a chunk against its manifest entry before the whole blob has
/// arrived (§14's own stated benefit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkHash([u8; 32]);

impl ChunkHash {
    pub fn from_ciphertext_chunk(chunk: &[u8]) -> Self {
        Self(*blake3::hash(chunk).as_bytes())
    }
}

/// §8: "A message may reference a logical attachment record. The
/// actual storage identity is the encrypted blob." A `LogicalAttachmentId`
/// is the human/application-facing handle (one per "the file the user
/// attached"); [`BlobId`] is the storage-facing one (one per distinct
/// ciphertext — two logical attachments could, in principle, resolve
/// to the same `BlobId` if their encrypted bytes happened to match,
/// though that's rare given per-blob random keys, §19). Kept as a
/// UUID, not a content hash — unlike a `BlobId`, a logical attachment
/// has no "content" of its own to hash; it's a reference, generated
/// once when a file is attached to a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct LogicalAttachmentId(Uuid);

impl LogicalAttachmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for LogicalAttachmentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ManifestId(Uuid);

impl ManifestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ManifestId {
    fn default() -> Self {
        Self::new()
    }
}

/// §26 "Transfer State Machine" (`transfer_state.rs`) tracks *a*
/// transfer's state, but nothing before this identified *which*
/// transfer — a real gap found while wiring §34's file events
/// (`events.rs`): a [`ManifestId`] identifies the chunked content, not
/// a directed offer of it, and the same manifest can legitimately be
/// offered as more than one transfer (the same file sent to two
/// different conversations is one blob, one manifest, two transfers).
/// Same UUID-newtype shape as [`ManifestId`]/[`LogicalAttachmentId`]
/// above, for the same reason: a transfer has no content of its own to
/// hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TransferId(Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TransferId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// §19: "not stored in public blob metadata." A plain `[u8; 32]`
/// wrapper, not `zeroize`-wrapped here — this crate defines the type
/// shape only; it doesn't generate, store, or hold a live key anywhere
/// (no encryption is actually performed in this crate — see its own
/// top doc comment), so there's no in-memory secret for this crate
/// itself to zeroize. A real caller that generates and holds one of
/// these — most naturally `siar-crypto`, which already has the
/// zeroize-on-drop discipline this would need (see
/// `siar_crypto::identity::DeviceIdentity`'s own `Drop` impl) — is
/// where that responsibility belongs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BlobEncryptionKey(pub [u8; 32]);

impl std::fmt::Debug for BlobEncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlobEncryptionKey(..)") // never print key material, even in Debug
    }
}
