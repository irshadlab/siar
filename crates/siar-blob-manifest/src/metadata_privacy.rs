//! §22 "Metadata Privacy".
//!
//! §22 names five example fields (file name, MIME type, dimensions,
//! duration, creation time) and asks an implementation to decide, per
//! field, which of three exposure classes it belongs to — then says
//! "default should minimize exposure." This module makes that decision
//! a real, checkable value ([`MetadataSensitivity`]) rather than a
//! policy that only exists in prose, and pins down this crate's own
//! default classification for every field [`crate::descriptor::FileMetadata`]
//! actually has today.
//!
//! Two of §22's five named fields — dimensions, duration — have no
//! home in [`crate::descriptor::FileMetadata`] yet (that struct covers
//! `display_name`/`media_type`/`logical_size`/`created_at_millis`
//! only); named here as an honest gap rather than silently classifying
//! a field this crate doesn't carry.

use crate::descriptor::FileMetadata;

/// §22's own three exposure classes, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSensitivity {
    /// Visible to transport/routing infrastructure — never anything a
    /// peer or relay would treat as sensitive on its own (e.g. a raw
    /// byte count needed to plan a transfer).
    PublicTransport,
    /// Encrypted at the application layer — readable only by the
    /// intended recipient(s), same protection class as the blob's own
    /// content. [`crate::metadata_encryption::encrypt_file_metadata`]
    /// is the real mechanism this class maps onto.
    EncryptedApplication,
    /// Never leaves the local device at all — not sent in any form,
    /// encrypted or otherwise.
    LocalOnly,
}

/// One of [`FileMetadata`]'s own fields, named so a caller can ask
/// "what's the default classification for the name field" without
/// stringly-typed field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataField {
    DisplayName,
    MediaType,
    LogicalSize,
    CreatedAtMillis,
}

/// §22's own "default should minimize exposure" made concrete for
/// every field this crate actually has:
///
/// - `display_name` → [`MetadataSensitivity::EncryptedApplication`] —
///   a real filename can leak content ("bank-statement-2026.pdf"); the
///   §22 worked example lists it first among sensitive fields.
/// - `media_type` → [`MetadataSensitivity::EncryptedApplication`] — a
///   MIME type is coarser than a filename but still describes content
///   ("this is a video," "this is a PDF"); minimizing exposure means
///   not handing that to transport infrastructure by default either.
/// - `logical_size` → [`MetadataSensitivity::PublicTransport`] — a
///   byte count is exactly the kind of value routing/transport
///   legitimately needs to plan a transfer (chunk counts, bandwidth
///   estimates — see Part 03's own `DeliveryRequirements`), and on its
///   own reveals far less than a name or type does.
/// - `created_at_millis` → [`MetadataSensitivity::EncryptedApplication`]
///   — a creation timestamp can correlate with other metadata (when a
///   photo was taken, when a document was authored) in ways transport
///   infrastructure has no legitimate need for.
///
/// Not a caller-overridable policy — a fixed default, matching §22's
/// own instruction to have one rather than leaving every field's
/// classification to be decided ad hoc at each call site.
pub fn default_sensitivity(field: MetadataField) -> MetadataSensitivity {
    use MetadataField as F;
    use MetadataSensitivity as S;
    match field {
        F::DisplayName => S::EncryptedApplication,
        F::MediaType => S::EncryptedApplication,
        F::LogicalSize => S::PublicTransport,
        F::CreatedAtMillis => S::EncryptedApplication,
    }
}

/// Splits a [`FileMetadata`] into what may be sent as public transport
/// metadata (today: only [`FileMetadata::logical_size`], as a bare
/// `u64`) versus what must go through
/// [`crate::metadata_encryption::encrypt_file_metadata`] instead. A
/// real caller building a transport-layer transfer offer should use
/// this rather than re-deriving the same split ad hoc — the one place
/// this crate's own §22 policy is enforced, not just documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicTransportMetadata {
    pub logical_size: u64,
}

pub fn public_transport_view(metadata: &FileMetadata) -> PublicTransportMetadata {
    PublicTransportMetadata {
        logical_size: metadata.logical_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_logical_size_defaults_to_public_transport() {
        assert_eq!(
            default_sensitivity(MetadataField::LogicalSize),
            MetadataSensitivity::PublicTransport
        );
        for field in [
            MetadataField::DisplayName,
            MetadataField::MediaType,
            MetadataField::CreatedAtMillis,
        ] {
            assert_eq!(
                default_sensitivity(field),
                MetadataSensitivity::EncryptedApplication,
                "{field:?} should default to minimizing exposure"
            );
        }
    }

    #[test]
    fn the_public_transport_view_carries_nothing_but_the_size() {
        let metadata = FileMetadata {
            display_name: Some(crate::descriptor::FileName::new("secret-plans.pdf").unwrap()),
            media_type: Some(siar_domain::MediaType::ImageJpeg),
            logical_size: 12_345,
            created_at_millis: Some(1_700_000_000_000),
        };
        let view = public_transport_view(&metadata);
        assert_eq!(view.logical_size, 12_345);
        // No API on PublicTransportMetadata exposes anything else —
        // the type itself is the enforcement, not a runtime check.
    }
}
