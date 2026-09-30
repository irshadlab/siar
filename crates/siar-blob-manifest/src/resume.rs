//! §29 "Resume Bitmap", §30 "Range-Based Resume", §31 "Resume
//! Protocol".

use serde::{Deserialize, Serialize};

use crate::ids::ManifestId;

/// §29: which chunks (by index) have been durably received, tracked as
/// a real bit-packed bitset (`Vec<u64>`, 64 chunks per word) rather
/// than re-deriving completeness from what's on disk every time. §96
/// "Chunk Bitmap Optimization": "for thousands of chunks, storing one
/// SQL row per chunk may be expensive... compressed bitmap/roaring
/// bitmap/bitset blob" — this crate has no SQL row per chunk to begin
/// with (there's no store at all — see `lib.rs`'s own scope note), but
/// the same "don't spend a whole byte, let alone a whole row, per
/// chunk" principle applies to this struct's own in-memory shape: a
/// `Vec<bool>` costs a full byte per chunk in every mainstream Rust
/// representation, where a packed `u64` word costs one bit — 64 times
/// less memory for the exact same information, which matters
/// specifically at the "thousands of chunks" scale §96 names. §96's
/// own "benchmark before choosing" is honored in spirit, not in a
/// literal microbenchmark: a hand-rolled bitset needs no new
/// dependency and the win is a property of the representation itself
/// (bits vs. bytes), not something that could go either way depending
/// on measurement — there was nothing to benchmark that a napkin
/// calculation didn't already settle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeBitmap {
    words: Vec<u64>,
    chunk_count: usize,
}

impl ResumeBitmap {
    pub fn new(chunk_count: usize) -> Self {
        Self {
            words: vec![0u64; chunk_count.div_ceil(64)],
            chunk_count,
        }
    }

    pub fn mark_received(&mut self, chunk_index: u32) {
        let index = chunk_index as usize;
        if index < self.chunk_count {
            self.words[index / 64] |= 1u64 << (index % 64);
        }
    }

    pub fn is_received(&self, chunk_index: u32) -> bool {
        let index = chunk_index as usize;
        if index >= self.chunk_count {
            return false;
        }
        (self.words[index / 64] >> (index % 64)) & 1 == 1
    }

    /// Word-at-a-time, not one [`ResumeBitmap::is_received`] call per
    /// chunk — exactly the bulk-operation speedup that's the other
    /// half of §96's own motivation alongside the memory savings above.
    /// Every full 64-chunk word must be all-ones, and the final
    /// partial word (if `chunk_count` isn't a multiple of 64) is
    /// checked against a mask covering only its real bits — the unused
    /// high bits of the last word are always zero (`mark_received`
    /// only ever sets bits below `chunk_count`), so they're never
    /// mistaken for "received."
    pub fn is_complete(&self) -> bool {
        if self.chunk_count == 0 {
            return true;
        }
        let full_words = self.chunk_count / 64;
        if self.words[..full_words].iter().any(|&w| w != u64::MAX) {
            return false;
        }
        let remaining_bits = self.chunk_count % 64;
        if remaining_bits == 0 {
            return true;
        }
        let mask = (1u64 << remaining_bits) - 1;
        self.words[full_words] & mask == mask
    }

    /// A real popcount sum, not a per-chunk loop — safe to sum every
    /// word's `count_ones()` unmasked because [`ResumeBitmap::mark_received`]
    /// can never set a padding bit past `chunk_count` in the first
    /// place (it's guarded by the same bounds check
    /// [`ResumeBitmap::is_received`] uses), so there are never any
    /// stray 1-bits to accidentally count.
    pub fn received_count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_count
    }

    /// §30: contiguous runs of missing chunk indices, as `[start, end)`
    /// half-open ranges — what a resume request actually asks a peer
    /// for ("send me chunks 12 through 47"), rather than one request
    /// per missing chunk.
    pub fn missing_ranges(&self) -> Vec<(u32, u32)> {
        self.ranges_where(false)
    }

    /// §99 "Chunk ACK": the complement of [`ResumeBitmap::missing_ranges`]
    /// — contiguous runs of chunks the receiver DOES have, in the same
    /// `[start, end)` half-open form, for acknowledging received
    /// ranges rather than requesting missing ones.
    pub fn received_ranges(&self) -> Vec<(u32, u32)> {
        self.ranges_where(true)
    }

    /// Shared scan behind both [`ResumeBitmap::missing_ranges`] and
    /// [`ResumeBitmap::received_ranges`] — one pass over this bitmap's
    /// own chunk indices, parameterized by which boolean value counts
    /// as "inside a run," rather than two independent copies of the
    /// same loop that could drift out of sync with each other. Still a
    /// bit-at-a-time scan (via [`ResumeBitmap::is_received`]) rather
    /// than a word-level run-finder — run boundaries can fall anywhere
    /// within a word, so a genuinely faster version would need real
    /// bit-twiddling (leading/trailing-zero counting per word) for a
    /// win this method's actual callers (built from real transfers,
    /// not synthetic thousand-chunk benchmarks) don't yet need.
    fn ranges_where(&self, target: bool) -> Vec<(u32, u32)> {
        let mut ranges = Vec::new();
        let mut run_start: Option<u32> = None;
        for index in 0..self.chunk_count as u32 {
            if self.is_received(index) == target {
                if run_start.is_none() {
                    run_start = Some(index);
                }
            } else if let Some(start) = run_start.take() {
                ranges.push((start, index));
            }
        }
        if let Some(start) = run_start {
            ranges.push((start, self.chunk_count as u32));
        }
        ranges
    }
}

/// §31 "Resume Protocol": "Receiver sends: manifest known, missing
/// chunks: 3,4,9,10. Sender transfers only missing chunks. No full
/// restart." — the actual wire-shaped request that exchange implies.
/// Built from a receiver's own [`ResumeBitmap`] via
/// [`ResumeRequest::from_bitmap`], reusing §30's own range form
/// ([`ResumeBitmap::missing_ranges`]) rather than one entry per missing
/// chunk index, for the same reason that method already gives —
/// requesting "12 through 47" is one item, not 35.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeRequest {
    pub manifest_id: ManifestId,
    pub missing_ranges: Vec<(u32, u32)>,
}

impl ResumeRequest {
    /// A request with no missing ranges means "I already have
    /// everything" — a real, meaningful `ResumeRequest`, not an error
    /// case; a sender receiving one sends nothing further, which
    /// [`ResumeRequest::is_satisfied`] makes an explicit check instead
    /// of an implicit "empty vec happens to mean done."
    pub fn from_bitmap(manifest_id: ManifestId, bitmap: &ResumeBitmap) -> Self {
        Self {
            manifest_id,
            missing_ranges: bitmap.missing_ranges(),
        }
    }

    pub fn is_satisfied(&self) -> bool {
        self.missing_ranges.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_request_built_from_a_bitmap_matches_its_own_missing_ranges() {
        let manifest_id = ManifestId::new();
        let mut bitmap = ResumeBitmap::new(10);
        for i in [0, 1, 2, 5, 6, 9] {
            bitmap.mark_received(i);
        }
        let request = ResumeRequest::from_bitmap(manifest_id, &bitmap);
        assert_eq!(request.manifest_id, manifest_id);
        assert_eq!(request.missing_ranges, vec![(3, 5), (7, 9)]);
        assert!(!request.is_satisfied());
    }

    #[test]
    fn a_complete_bitmap_produces_a_satisfied_resume_request() {
        let manifest_id = ManifestId::new();
        let mut bitmap = ResumeBitmap::new(2);
        bitmap.mark_received(0);
        bitmap.mark_received(1);
        let request = ResumeRequest::from_bitmap(manifest_id, &bitmap);
        assert!(request.is_satisfied());
    }

    #[test]
    fn a_fresh_bitmap_is_entirely_missing() {
        let bitmap = ResumeBitmap::new(5);
        assert!(!bitmap.is_complete());
        assert_eq!(bitmap.missing_ranges(), vec![(0, 5)]);
    }

    #[test]
    fn received_ranges_is_the_exact_complement_of_missing_ranges() {
        let mut bitmap = ResumeBitmap::new(10);
        for i in [0, 1, 2, 5, 6, 9] {
            bitmap.mark_received(i);
        }
        assert_eq!(bitmap.received_ranges(), vec![(0, 3), (5, 7), (9, 10)]);
        assert_eq!(bitmap.missing_ranges(), vec![(3, 5), (7, 9)]);
    }

    #[test]
    fn a_fresh_bitmap_has_no_received_ranges_at_all() {
        let bitmap = ResumeBitmap::new(4);
        assert!(bitmap.received_ranges().is_empty());
    }

    #[test]
    fn marking_every_chunk_received_completes_the_bitmap() {
        let mut bitmap = ResumeBitmap::new(3);
        bitmap.mark_received(0);
        bitmap.mark_received(1);
        bitmap.mark_received(2);
        assert!(bitmap.is_complete());
        assert!(bitmap.missing_ranges().is_empty());
    }

    #[test]
    fn missing_ranges_groups_contiguous_gaps() {
        let mut bitmap = ResumeBitmap::new(10);
        for i in [0, 1, 2, 5, 6, 9] {
            bitmap.mark_received(i);
        }
        // received: 0,1,2,_,_,5,6,_,_,9 → missing: [3,5), [7,9)
        assert_eq!(bitmap.missing_ranges(), vec![(3, 5), (7, 9)]);
        assert_eq!(bitmap.received_count(), 6);
    }

    #[test]
    fn a_zero_chunk_bitmap_is_trivially_complete() {
        let bitmap = ResumeBitmap::new(0);
        assert!(bitmap.is_complete());
        assert_eq!(bitmap.received_count(), 0);
    }

    #[test]
    fn a_bitmap_spanning_multiple_64_bit_words_marks_and_reads_correctly_across_the_boundary() {
        // 130 chunks spans three words (64 + 64 + 2) — exercise a bit
        // right at a word boundary (63/64) and one in the final
        // partial word (129).
        let mut bitmap = ResumeBitmap::new(130);
        bitmap.mark_received(63);
        bitmap.mark_received(64);
        bitmap.mark_received(129);
        assert!(bitmap.is_received(63));
        assert!(bitmap.is_received(64));
        assert!(bitmap.is_received(129));
        assert!(!bitmap.is_received(62));
        assert!(!bitmap.is_received(65));
        assert!(!bitmap.is_received(128));
        assert_eq!(bitmap.received_count(), 3);
        assert!(!bitmap.is_complete());
    }

    #[test]
    fn a_bitmap_with_a_non_multiple_of_64_chunk_count_is_only_complete_when_every_real_bit_is_set()
    {
        // 65 chunks: one full word plus a single-bit final word. The
        // 63 unused high bits of that final word must never be
        // mistaken for "received."
        let mut bitmap = ResumeBitmap::new(65);
        for i in 0..64 {
            bitmap.mark_received(i);
        }
        assert!(!bitmap.is_complete(), "chunk 64 is still missing");
        bitmap.mark_received(64);
        assert!(bitmap.is_complete());
    }

    #[test]
    fn marking_an_out_of_range_chunk_index_is_a_harmless_no_op() {
        let mut bitmap = ResumeBitmap::new(4);
        bitmap.mark_received(999);
        assert_eq!(bitmap.received_count(), 0);
        assert!(!bitmap.is_received(999));
    }
}
