//! §23 "Message Attachment Reference", §24 "File-Only Transfer".
//!
//! §23's own code block names an `EncryptedOrVisibleMetadata` field
//! without spelling out its shape — read here as a real choice between
//! two states a caller can be in, not a single flexible blob: either
//! the metadata has already been sealed
//! ([`crate::metadata_encryption::encrypt_file_metadata`]) and only
//! ciphertext is on hand, or it hasn't been sealed yet and the plain
//! [`FileMetadata`] itself is on hand. Modeling this as an enum rather
//! than "ciphertext bytes, always" means a caller building an
//! [`AttachmentReference`] before encryption (e.g. while assembling a
//! local, not-yet-sent attachment) doesn't have to round-trip through
//! encrypt-then-hold-ciphertext just to satisfy the type.

use serde::{Deserialize, Serialize};

use crate::descriptor::{BlobDescriptor, FileMetadata};

/// §23's own `EncryptedOrVisibleMetadata` — see this module's own top
/// doc comment for why it's an enum rather than a single opaque blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AttachmentMetadata {
    /// Not yet sealed — the real [`FileMetadata`] value.
    Visible(FileMetadata),
    /// Already sealed via
    /// [`crate::metadata_encryption::encrypt_file_metadata`] — nonce-
    /// prepended ciphertext, opaque to anything without the blob's own
    /// key.
    Encrypted(Vec<u8>),
}

/// §23, field-for-field: "Messaging stores a reference. ... The
/// message does not contain the file bytes." Nothing in this struct's
/// own shape can carry file content — only a [`BlobDescriptor`]
/// (content-addressed, never the bytes themselves) and this crate's
/// own [`AttachmentMetadata`] choice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentReference {
    pub blob: BlobDescriptor,
    pub metadata: AttachmentMetadata,
}

impl AttachmentReference {
    pub fn visible(blob: BlobDescriptor, metadata: FileMetadata) -> Self {
        Self {
            blob,
            metadata: AttachmentMetadata::Visible(metadata),
        }
    }

    pub fn encrypted(blob: BlobDescriptor, ciphertext: Vec<u8>) -> Self {
        Self {
            blob,
            metadata: AttachmentMetadata::Encrypted(ciphertext),
        }
    }
}

// §24 "File-Only Transfer": "A file-sharing app can use FileOffer/
// FileAccept/Transfer/Blob without creating a conversation. This is a
// core reusability requirement." Not a type of its own to build — a
// structural property to preserve — so there is no `FileOnlyTransfer`
// struct here. What actually makes the requirement true is that
// nothing in this crate (this module included) has any dependency on
// a conversation/message concept: `AttachmentReference` above is built
// entirely from [`BlobDescriptor`]/[`AttachmentMetadata`], and every
// other real type a transfer needs —
// [`crate::transfer_state::TransferState`],
// [`crate::transfer_record::TransferRecord`],
// [`crate::transfer_journal::TransferJournal`] — depends on
// `siar-domain` (for [`siar_domain::DeviceId`] alone) and nothing that
// implies a `ConversationId`/`MessageId` exists anywhere. This crate's
// own `Cargo.toml` has never depended on `siar-messaging`, which is
// the actual, checkable form of "without creating a conversation":
// a `cargo tree -i siar-messaging` from this crate's own manifest
// finds nothing, because there is nothing to find.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor};
    use crate::ids::BlobId;

    fn sample_blob() -> BlobDescriptor {
        BlobDescriptor {
            blob_id: BlobId::from_ciphertext(b"some ciphertext"),
            size: siar_domain::BlobSize::parse(15).unwrap(),
            chunking: ChunkingDescriptor {
                chunk_size: 64 * 1024,
            },
            encryption: EncryptionDescriptor {
                algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                nonce: [0u8; 12],
            },
            media_type: None,
            manifest_id: None,
        }
    }

    #[test]
    fn a_visible_reference_carries_the_real_metadata() {
        let metadata = FileMetadata {
            display_name: None,
            media_type: None,
            logical_size: 15,
            created_at_millis: None,
        };
        let reference = AttachmentReference::visible(sample_blob(), metadata.clone());
        assert_eq!(reference.metadata, AttachmentMetadata::Visible(metadata));
    }

    #[test]
    fn an_encrypted_reference_carries_only_ciphertext() {
        let ciphertext = vec![1, 2, 3, 4];
        let reference = AttachmentReference::encrypted(sample_blob(), ciphertext.clone());
        assert_eq!(
            reference.metadata,
            AttachmentMetadata::Encrypted(ciphertext)
        );
    }

    #[test]
    fn round_trips_through_postcard_like_every_other_wire_type_here() {
        let reference = AttachmentReference::encrypted(sample_blob(), vec![9, 9, 9]);
        let bytes = postcard::to_allocvec(&reference).unwrap();
        let decoded: AttachmentReference = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, reference);
    }
}
