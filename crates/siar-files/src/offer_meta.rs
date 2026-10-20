//! What an offer carries besides the blob's description: the blob key,
//! so the receiver can actually open what it is sent, and an optional
//! display name.
//!
//! **Confidentiality.** The field on the wire is called `encrypted_metadata`
//! and this crate relies on the *channel* for that: offers travel only to
//! an authenticated peer over the end-to-end encrypted transport (QUIC
//! between paired iroh endpoints), never through a relay or DTN custodian
//! that is not the recipient. Putting the key in an offer is therefore as
//! private as the connection it rides. A transport that cannot promise
//! that (a store-and-forward path through third parties) must wrap the
//! key per recipient instead — see `siar_blob_manifest::key_wrapping` —
//! and must not use this carriage.
//!
//! **Trust.** Everything here is peer-supplied: the bytes are bounded by
//! the wire limits before they get here, parsing failures yield "no
//! metadata" rather than an error (the blob can still be accepted and
//! its bytes verified), and the name is only ever a *display* name —
//! [`ReceivedFile::export_name`] is the only string that may reach a
//! filesystem.

use serde::{Deserialize, Serialize};
use siar_blob_manifest::descriptor::FileName;
use siar_blob_manifest::filename_safety::sanitize_for_export;
use siar_blob_manifest::ids::{BlobEncryptionKey, BlobId, TransferId};
use siar_blob_manifest::key_wrapping::WrappedKey;
use siar_domain::{AttachmentReference, BlobSize, DeviceId, MediaType};

use crate::FilesError;

const VERSION: u8 = 1;
const VERSION_WRAPPED: u8 = 2;

#[derive(Serialize, Deserialize)]
struct Wire {
    version: u8,
    key: [u8; 32],
    file_name: Option<FileName>,
}

/// Version 2: the key is not in the offer at all, only a copy wrapped for
/// this one recipient (`siar_blob_manifest::key_wrapping`). Safe to carry
/// over paths that are not end-to-end private, because only the intended
/// recipient's pairwise secret opens it.
#[derive(Serialize, Deserialize)]
struct WireWrapped {
    version: u8,
    wrapped: Vec<u8>,
    file_name: Option<FileName>,
}

/// What an offer's metadata field turned out to hold.
pub(crate) enum Decoded {
    /// The raw key (version 1).
    Key(OfferMetadata),
    /// A key wrapped for one recipient (version 2); needs that
    /// recipient's pairwise secret to open.
    Wrapped {
        wrapped: WrappedKey,
        file_name: Option<FileName>,
    },
}

/// Parsed, validated offer metadata.
#[derive(Clone)]
pub(crate) struct OfferMetadata {
    pub(crate) key: BlobEncryptionKey,
    pub(crate) file_name: Option<FileName>,
}

pub(crate) fn encode(
    key: &BlobEncryptionKey,
    file_name: Option<&str>,
) -> Result<Vec<u8>, FilesError> {
    let file_name = file_name
        .map(FileName::new)
        .transpose()
        .map_err(|_| FilesError::FileNameTooLong)?;
    let w = Wire {
        version: VERSION,
        key: key.0,
        file_name,
    };
    postcard::to_allocvec(&w).map_err(|_| FilesError::FileNameTooLong)
}

/// Metadata whose key is wrapped for one recipient.
pub(crate) fn encode_wrapped(
    wrapped: &WrappedKey,
    file_name: Option<&str>,
) -> Result<Vec<u8>, FilesError> {
    let file_name = file_name
        .map(FileName::new)
        .transpose()
        .map_err(|_| FilesError::FileNameTooLong)?;
    postcard::to_allocvec(&WireWrapped {
        version: VERSION_WRAPPED,
        wrapped: wrapped.to_bytes(),
        file_name,
    })
    .map_err(|_| FilesError::FileNameTooLong)
}

fn clean_name(n: Option<FileName>) -> Option<FileName> {
    n.filter(|n| n.as_str().len() <= 255)
}

/// `None` for anything that is not well-formed v1 or v2 metadata. The
/// first byte is the version, so each is tried only for its own tag.
pub(crate) fn decode_any(bytes: &[u8]) -> Option<Decoded> {
    match *bytes.first()? {
        VERSION => {
            let w: Wire = postcard::from_bytes(bytes).ok()?;
            Some(Decoded::Key(OfferMetadata {
                key: BlobEncryptionKey(w.key),
                file_name: clean_name(w.file_name),
            }))
        }
        VERSION_WRAPPED => {
            let w: WireWrapped = postcard::from_bytes(bytes).ok()?;
            Some(Decoded::Wrapped {
                wrapped: WrappedKey::from_bytes(&w.wrapped)?,
                file_name: clean_name(w.file_name),
            })
        }
        _ => None,
    }
}

/// `None` for anything that is not a well-formed v1 metadata blob.
#[cfg(test)]
pub(crate) fn decode(bytes: &[u8]) -> Option<OfferMetadata> {
    match decode_any(bytes)? {
        Decoded::Key(m) => Some(m),
        Decoded::Wrapped { .. } => None,
    }
}

/// A file that arrived and verified, with everything needed to open it.
/// The key lives only here: the service does not keep it. An application
/// that wants the file later must keep [`ReceivedFile::reference`] (the
/// same shape a message carries) — dropping this without doing so means
/// the stored ciphertext can never be opened.
pub struct ReceivedFile {
    pub transfer_id: TransferId,
    pub from: DeviceId,
    pub blob_id: BlobId,
    /// Size of the stored (encrypted) blob.
    pub stored_size: u64,
    pub media_type: Option<MediaType>,
    pub(crate) file_name: Option<FileName>,
    pub(crate) key: BlobEncryptionKey,
}

impl ReceivedFile {
    /// The sender's name for the file, for display only. Untrusted.
    pub fn display_name(&self) -> Option<&str> {
        self.file_name.as_ref().map(FileName::as_str)
    }

    /// A name that is safe to create on a filesystem: no separators, no
    /// control characters, never empty, never `.`/`..`.
    pub fn export_name(&self) -> String {
        match &self.file_name {
            Some(n) => sanitize_for_export(n),
            None => "unnamed_file".to_string(),
        }
    }

    /// The reference that opens this file with
    /// [`crate::FilesService::open_attachment`] /
    /// [`crate::FilesService::export_attachment`].
    pub fn reference(&self) -> Result<AttachmentReference, FilesError> {
        Ok(AttachmentReference {
            blob_hash: *self.blob_id.as_bytes(),
            encrypted_size: BlobSize::parse(self.stored_size)?,
            media_type: self.media_type.unwrap_or(MediaType::Other),
            attachment_key: self.key.0,
            thumbnail: None,
        })
    }
}

impl std::fmt::Debug for ReceivedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never prints the key.
        f.debug_struct("ReceivedFile")
            .field("transfer_id", &self.transfer_id)
            .field("blob_id", &self.blob_id)
            .field("display_name", &self.display_name())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> BlobEncryptionKey {
        BlobEncryptionKey([7; 32])
    }

    #[test]
    fn metadata_round_trips() {
        let b = encode(&key(), Some("report.pdf")).unwrap();
        let m = decode(&b).unwrap();
        assert_eq!(m.key, key());
        assert_eq!(m.file_name.unwrap().as_str(), "report.pdf");
        let b = encode(&key(), None).unwrap();
        assert!(decode(&b).unwrap().file_name.is_none());
    }

    #[test]
    fn a_name_over_255_bytes_is_refused_when_sending() {
        assert_eq!(
            encode(&key(), Some(&"x".repeat(256))).err(),
            Some(FilesError::FileNameTooLong)
        );
    }

    #[test]
    fn garbage_empty_and_foreign_versions_are_just_no_metadata() {
        assert!(decode(&[]).is_none());
        assert!(decode(&[0xFF; 50]).is_none());
        let mut b = encode(&key(), None).unwrap();
        b[0] = 9; // an unknown version
        assert!(decode(&b).is_none());
    }

    #[test]
    fn an_oversized_name_smuggled_in_is_dropped_but_the_key_survives() {
        let w = Wire {
            version: VERSION,
            key: [1; 32],
            file_name: Some(serde_json_free_long_name()),
        };
        let b = postcard::to_allocvec(&w).unwrap();
        let m = decode(&b).unwrap();
        assert!(m.file_name.is_none());
        assert_eq!(m.key.0, [1; 32]);
    }

    fn serde_json_free_long_name() -> FileName {
        // FileName::new refuses >255, so build one by decoding bytes that
        // a hostile peer could send.
        let mut raw = vec![0u8; 0];
        let s = "y".repeat(300);
        raw.extend_from_slice(&postcard::to_allocvec(&s).unwrap());
        postcard::from_bytes(&raw).unwrap()
    }

    #[test]
    fn wrapped_metadata_round_trips_and_carries_no_raw_key() {
        use siar_blob_manifest::key_wrapping::wrap_blob_key;
        let (blob, who) = (BlobId::from_ciphertext(b"b"), DeviceId::new());
        let w = wrap_blob_key(&key(), &[9; 32], blob, who).unwrap();
        let bytes = encode_wrapped(&w, Some("a.txt")).unwrap();
        assert_eq!(bytes[0], VERSION_WRAPPED);
        assert!(
            !bytes.windows(32).any(|win| win == key().0),
            "the raw key must not appear in a wrapped offer"
        );
        match decode_any(&bytes).unwrap() {
            Decoded::Wrapped { wrapped, file_name } => {
                assert_eq!(wrapped, w);
                assert_eq!(file_name.unwrap().as_str(), "a.txt");
            }
            Decoded::Key(_) => panic!("decoded as a raw key"),
        }
    }

    #[test]
    fn a_wrapped_blob_with_the_wrong_length_or_tag_is_no_metadata() {
        let bad = postcard::to_allocvec(&WireWrapped {
            version: VERSION_WRAPPED,
            wrapped: vec![0; 7],
            file_name: None,
        })
        .unwrap();
        assert!(decode_any(&bad).is_none());
        assert!(decode_any(&[7, 1, 2, 3]).is_none());
    }

    #[test]
    fn export_names_never_carry_a_path_and_debug_never_prints_the_key() {
        let f = ReceivedFile {
            transfer_id: TransferId::new(),
            from: DeviceId::new(),
            blob_id: BlobId::from_ciphertext(b"x"),
            stored_size: 10,
            media_type: None,
            file_name: Some(FileName::new("../../etc/passwd").unwrap()),
            key: key(),
        };
        let n = f.export_name();
        assert!(!n.contains('/') && !n.contains('\\'), "{n}");
        assert_eq!(
            f.display_name(),
            Some("../../etc/passwd"),
            "display is untouched"
        );
        let dbg = format!("{f:?}");
        assert!(
            !dbg.contains("[7, 7"),
            "the key must not appear in Debug: {dbg}"
        );
        let none = ReceivedFile {
            file_name: None,
            ..f
        };
        assert_eq!(none.export_name(), "unnamed_file");
    }
}
