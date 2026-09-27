//! §28 "Transfer Journal".
//!
//! §28's own instruction is as much about what NOT to build as what
//! to: "high-frequency operational state belongs in a transfer
//! journal/projection... do not append a permanent event for every
//! packet. Part 04 event log stores meaningful semantic transitions."
//! [`TransferJournal`] is that separate, non-durable home — nothing in
//! this module ever constructs a [`crate::events::FileEvent`] or
//! touches `siar-event-log`, on purpose. It exists alongside
//! [`crate::transfer_record::TransferRecord`] (which DOES eventually
//! back onto durable events, via each domain's own state-machine
//! transitions) rather than folded into it, so the high-frequency
//! fields below can be mutated as often as every chunk arrives without
//! that ever being mistaken for something a caller should be recording
//! to the event log.

use crate::resume::ResumeBitmap;

/// §28's own four named examples, each field-for-field:
/// - `chunk_bitmap` → reuses [`ResumeBitmap`] rather than a second
///   bitset type, since it's already exactly this shape (§29).
/// - `bytes_verified` → running total, updated by
///   [`TransferJournal::record_chunk_verified`].
/// - `active_path` → deliberately a bare `Option<String>` label, not a
///   real transport/routing type: this crate has no dependency on
///   `siar-transport`/`siar-routing-policy` (see `lib.rs`'s own "no
///   transport/routing integration" scope note), so a real caller
///   supplies whatever it uses to name a path (a `TransportKind`'s
///   `Debug` output, a relay id, anything) rather than this crate
///   inventing a path-identity type it has no other use for.
/// - `retry_count` → bumped by [`TransferJournal::record_retry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferJournal {
    pub chunk_bitmap: ResumeBitmap,
    pub bytes_verified: u64,
    pub active_path: Option<String>,
    pub retry_count: u32,
}

impl TransferJournal {
    pub fn new(chunk_count: usize) -> Self {
        Self {
            chunk_bitmap: ResumeBitmap::new(chunk_count),
            bytes_verified: 0,
            active_path: None,
            retry_count: 0,
        }
    }

    /// Marks `chunk_index` received in the bitmap and adds
    /// `chunk_len_bytes` to the running verified-bytes total — the one
    /// place both fields move together, so a caller can't update the
    /// bitmap while forgetting the byte count (or vice versa) and
    /// leave the two silently out of sync.
    pub fn record_chunk_verified(&mut self, chunk_index: u32, chunk_len_bytes: u64) {
        self.chunk_bitmap.mark_received(chunk_index);
        self.bytes_verified += chunk_len_bytes;
    }

    pub fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    pub fn set_active_path(&mut self, path: impl Into<String>) {
        self.active_path = Some(path.into());
    }

    pub fn is_complete(&self) -> bool {
        self.chunk_bitmap.is_complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_journal_has_no_verified_bytes_no_retries_and_no_active_path() {
        let journal = TransferJournal::new(4);
        assert_eq!(journal.bytes_verified, 0);
        assert_eq!(journal.retry_count, 0);
        assert_eq!(journal.active_path, None);
        assert!(!journal.is_complete());
    }

    #[test]
    fn recording_a_verified_chunk_moves_the_bitmap_and_the_byte_count_together() {
        let mut journal = TransferJournal::new(2);
        journal.record_chunk_verified(0, 1_000);
        assert!(journal.chunk_bitmap.is_received(0));
        assert_eq!(journal.bytes_verified, 1_000);
        journal.record_chunk_verified(1, 500);
        assert!(journal.is_complete());
        assert_eq!(journal.bytes_verified, 1_500);
    }

    #[test]
    fn retries_and_active_path_are_independent_of_chunk_progress() {
        let mut journal = TransferJournal::new(1);
        journal.record_retry();
        journal.record_retry();
        journal.set_active_path("iroh-direct");
        assert_eq!(journal.retry_count, 2);
        assert_eq!(journal.active_path.as_deref(), Some("iroh-direct"));
        assert!(!journal.is_complete());
    }
}
