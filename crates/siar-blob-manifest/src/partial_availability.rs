//! §32 "Partial Availability".
//!
//! §32: "receiver should be able to access a thumbnail/preview/first
//! playable media segment before full object completion where
//! application allows. The blob store exposes safe partial-read
//! status." This crate has no blob store (see `lib.rs`'s own "no local
//! store" scope note) — what it can give a future store is the pure
//! computation a "safe partial-read status" actually needs: given
//! which chunks have arrived ([`crate::resume::ResumeBitmap`]) and the
//! manifest describing their byte layout
//! ([`crate::manifest::BlobManifest`]), how many bytes from the START
//! of the file are safely readable right now.
//!
//! Deliberately "from the start" only, not "any chunk that happens to
//! be present": a thumbnail/preview/progressive-media reader needs a
//! contiguous prefix to do anything useful with (you can't decode a
//! JPEG's header from chunk 7 alone) — a single received chunk in the
//! middle of an otherwise-empty transfer is real progress but not
//! *safely readable* progress in §32's own sense, so it isn't counted
//! here.

use crate::manifest::BlobManifest;
use crate::resume::ResumeBitmap;

/// The "safe partial-read status" §32 asks a blob store to expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialAvailability {
    /// How many bytes, counted from offset 0, are covered by an
    /// unbroken run of received chunks. A caller wanting to read the
    /// first N bytes of a file may do so safely if `leading_bytes >=
    /// N`.
    pub leading_bytes: u64,
    pub is_fully_available: bool,
}

/// Computes [`PartialAvailability`] from a manifest's own chunk layout
/// and a receiver's current [`ResumeBitmap`]. `manifest.chunks` is
/// assumed ordered by index starting at 0, matching
/// [`crate::manifest::build_manifest`]'s own construction — the same
/// assumption [`ResumeBitmap`] itself already makes about chunk
/// indices being dense from zero.
pub fn partial_availability(manifest: &BlobManifest, bitmap: &ResumeBitmap) -> PartialAvailability {
    if bitmap.is_complete() {
        return PartialAvailability {
            leading_bytes: manifest.total_size,
            is_fully_available: true,
        };
    }

    let mut leading_bytes = 0u64;
    for chunk in &manifest.chunks {
        if !bitmap.is_received(chunk.index) {
            break;
        }
        leading_bytes += chunk.size as u64;
    }

    PartialAvailability {
        leading_bytes,
        is_fully_available: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;

    #[test]
    fn nothing_received_means_nothing_leading_is_available() {
        let ciphertext = vec![0u8; 300];
        let manifest =
            crate::manifest::build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let bitmap = ResumeBitmap::new(3);
        let status = partial_availability(&manifest, &bitmap);
        assert_eq!(status.leading_bytes, 0);
        assert!(!status.is_fully_available);
    }

    #[test]
    fn a_contiguous_prefix_counts_but_a_gap_stops_it() {
        let ciphertext = vec![0u8; 300]; // 3 chunks of 100 bytes each
        let manifest =
            crate::manifest::build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let mut bitmap = ResumeBitmap::new(3);
        bitmap.mark_received(0);
        // chunk 1 still missing — chunk 2 arriving out of order must
        // NOT count toward the leading prefix, since it isn't safely
        // readable without chunk 1.
        bitmap.mark_received(2);
        let status = partial_availability(&manifest, &bitmap);
        assert_eq!(status.leading_bytes, 100);
        assert!(!status.is_fully_available);
    }

    #[test]
    fn a_full_bitmap_reports_the_whole_file_available() {
        let ciphertext = vec![0u8; 250];
        let manifest =
            crate::manifest::build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap();
        let mut bitmap = ResumeBitmap::new(3);
        bitmap.mark_received(0);
        bitmap.mark_received(1);
        bitmap.mark_received(2);
        let status = partial_availability(&manifest, &bitmap);
        assert_eq!(status.leading_bytes, 250);
        assert!(status.is_fully_available);
    }
}
