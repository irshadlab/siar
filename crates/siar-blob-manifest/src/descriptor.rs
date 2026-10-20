//! §9 "Blob Descriptor", §10 "File Metadata", §18 "Encryption Model",
//! §21 "Encryption Metadata".

use serde::{Deserialize, Serialize};
use siar_domain::{BlobSize, MediaType};

use crate::ids::{BlobId, ManifestId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkingDescriptor {
    pub chunk_size: u32,
}

/// §18/§21. "Use authenticated encryption" — `ChaCha20Poly1305` is
/// this workspace's already-established choice (`siar-crypto`'s
/// message encryption uses it; see that crate's `Cargo.toml`), named
/// as the one variant here rather than an open-ended algorithm
/// identifier, since introducing a second AEAD into this codebase
/// isn't this crate's call to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionAlgorithm {
    /// Whole-blob AEAD, `nonce ‖ ciphertext ‖ tag` (format v1).
    ChaCha20Poly1305,
    /// Chunk-wise AEAD (format v2, [`crate::chunked_encryption`]); the
    /// descriptor's `nonce` is unused (the nonce prefix is in the blob header).
    ChaCha20Poly1305Chunked,
}

/// §18/§21: real encrypt/decrypt now lives in [`crate::encryption`] —
/// see that module's own doc comment for the full reasoning. One fresh
/// random nonce per whole-blob AEAD operation (not a per-chunk scheme):
/// §7's "encrypt first, then content-address the ciphertext" already
/// means encryption happens once, over the entire plaintext, before
/// [`crate::chunking`]/[`crate::manifest`] ever chunk the *resulting
/// ciphertext* buffer by byte offset — so there was never a need for
/// §20 "Chunk Nonces"' per-chunk derivation this field's own doc
/// comment used to speculate about before [`crate::encryption`] was
/// actually implemented. Corrected here rather than left as a stale
/// guess once the real implementation resolved the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionDescriptor {
    pub algorithm: EncryptionAlgorithm,
    pub nonce: [u8; 12],
}

/// §9, verbatim field-for-field, reusing `siar_domain::MediaType`
/// (already real in this workspace, used by
/// `siar-messaging::MessageService::send_attachment`) rather than a
/// second media-type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobDescriptor {
    pub blob_id: BlobId,
    pub size: BlobSize,
    pub chunking: ChunkingDescriptor,
    pub encryption: EncryptionDescriptor,
    pub media_type: Option<MediaType>,
    pub manifest_id: Option<ManifestId>,
}

/// §9: "Do not trust remote-declared size blindly." Not enforced by
/// this struct itself (a plain data holder can't refuse to be
/// constructed with a lie) — enforcement is
/// [`crate::manifest::BlobManifest::total_size`] being recomputed from
/// the actual chunk descriptors, which a real receive path checks
/// against a remote-declared [`BlobDescriptor::size`] rather than
/// trusting it directly.
///
/// §10. `display_name: Option<FileName>` — [`FileName`] bounds the
/// string length (Part 01 §9's reasoning against unbounded strings,
/// applied here), unlike `String` directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMetadata {
    pub display_name: Option<FileName>,
    pub media_type: Option<MediaType>,
    pub logical_size: u64,
    pub created_at_millis: Option<u64>,
}

/// A bounded display name — 255 bytes, a conservative cross-filesystem
/// limit (matches ext4/NTFS/APFS's own 255-byte filename ceilings, so
/// a name that fits here is a name every target platform can also
/// actually write to disk).
///
/// §54 "Path Traversal Protection": "must remain just a display
/// string. Never construct storage paths directly from remote
/// filenames." This type enforces that by omission, not by stripping
/// characters — there is no `as_path`/`to_path_buf` method anywhere on
/// `FileName`, only [`FileName::as_str`] for display. The one
/// sanctioned route from a `FileName` to bytes actually written to
/// disk is [`crate::filename_safety::sanitize_for_export`] (§55), which
/// takes a `FileName` and returns a real, traversal-safe `String` —
/// never this type directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("file name is {len} bytes, over the 255-byte limit")]
pub struct FileNameTooLong {
    len: usize,
}

impl FileName {
    pub fn new(name: impl Into<String>) -> Result<Self, FileNameTooLong> {
        let name = name.into();
        if name.len() > 255 {
            return Err(FileNameTooLong { len: name.len() });
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
