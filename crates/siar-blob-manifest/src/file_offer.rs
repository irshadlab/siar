//! §44 "File Offer Protocol".
//!
//! `encrypted_metadata` is `Vec<u8>` rather than the spec's own
//! `Bytes` — this crate has never taken a dependency on the `bytes`
//! crate (every other ciphertext field here, e.g.
//! [`crate::attachment_reference::AttachmentMetadata::Encrypted`], is
//! already `Vec<u8>`), and `expiry` is `Option<u64>` milliseconds
//! rather than a `Timestamp` type, matching
//! [`crate::descriptor::FileMetadata::created_at_millis`]'s own
//! convention — the same two departures this crate has made
//! consistently everywhere a stub code block implied a type it
//! doesn't otherwise need.

use serde::{Deserialize, Serialize};

use crate::descriptor::BlobDescriptor;
use crate::ids::TransferId;

/// §44, field-for-field (see this module's own top doc comment for the
/// two departures).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileOffer {
    pub transfer_id: TransferId,
    pub blob: BlobDescriptor,
    pub encrypted_metadata: Vec<u8>,
    pub expiry_millis: Option<u64>,
}

impl FileOffer {
    pub fn is_expired(&self, now_millis: u64) -> bool {
        self.expiry_millis
            .is_some_and(|expiry| now_millis >= expiry)
    }
}

/// §44's own "Receiver decides" list, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOfferDecision {
    Accept,
    Reject,
    Defer,
    MetadataOnly,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{ChunkingDescriptor, EncryptionAlgorithm, EncryptionDescriptor};
    use crate::ids::BlobId;

    fn sample_offer(expiry_millis: Option<u64>) -> FileOffer {
        FileOffer {
            transfer_id: TransferId::new(),
            blob: BlobDescriptor {
                blob_id: BlobId::from_ciphertext(b"ciphertext"),
                size: siar_domain::BlobSize::parse(10).unwrap(),
                chunking: ChunkingDescriptor {
                    chunk_size: 64 * 1024,
                },
                encryption: EncryptionDescriptor {
                    algorithm: EncryptionAlgorithm::ChaCha20Poly1305,
                    nonce: [0u8; 12],
                },
                media_type: None,
                manifest_id: None,
            },
            encrypted_metadata: vec![1, 2, 3],
            expiry_millis,
        }
    }

    #[test]
    fn an_offer_with_no_expiry_never_expires() {
        let offer = sample_offer(None);
        assert!(!offer.is_expired(u64::MAX));
    }

    #[test]
    fn an_offer_expires_at_and_after_its_own_deadline_but_not_before() {
        let offer = sample_offer(Some(1_000));
        assert!(!offer.is_expired(999));
        assert!(offer.is_expired(1_000));
        assert!(offer.is_expired(1_001));
    }

    #[test]
    fn a_file_offer_round_trips_through_postcard() {
        let offer = sample_offer(Some(5_000));
        let bytes = postcard::to_allocvec(&offer).unwrap();
        let decoded: FileOffer = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, offer);
    }
}
