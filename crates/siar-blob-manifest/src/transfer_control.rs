//! §98 "Transfer ACK", §99 "Chunk ACK", §100 "Completion ACK", §101
//! "Transfer Cancellation", §102 "Pause", §103 "Deferred Transfer",
//! §104 "Retry", §105 "Corrupt Chunk", §106 "Corrupt Manifest".

use serde::{Deserialize, Serialize};

use crate::resume::ResumeBitmap;

/// §98's own four named states, verbatim — "these are different
/// states," not four names for one idea. A sender that only tracked
/// one boolean "acked" flag would be unable to represent "the
/// transport delivered the bytes" separately from "the application
/// actually accepted them," which is exactly the confusion this
/// section exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferAckKind {
    TransportAck,
    ChunkAccepted,
    BlobVerified,
    UserAccepted,
}

/// §100: "Only then should sender mark transfer complete" — "then"
/// meaning specifically `BlobVerified`, not any of the other three
/// real-but-insufficient ACK kinds. A sender that marked a transfer
/// complete on `TransportAck` alone would be declaring success before
/// the receiver had even checked the bytes.
pub fn allows_transfer_completion(ack: TransferAckKind) -> bool {
    matches!(ack, TransferAckKind::BlobVerified)
}

/// §99: "chunk index verified... or use range/bitmap
/// acknowledgements... avoid ACK per tiny frame if chunk is already
/// large." Reuses [`ResumeBitmap`]'s own range machinery rather than a
/// second bitmap type — a chunk ACK and a resume request are the same
/// underlying fact (which chunks does the receiver have) viewed from
/// opposite sides of a transfer, so [`ChunkAck::from_bitmap`] mirrors
/// [`crate::resume::ResumeRequest::from_bitmap`] exactly, but reports
/// the *received* ranges instead of the missing ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkAck {
    pub received_ranges: Vec<(u32, u32)>,
}

impl ChunkAck {
    pub fn from_bitmap(bitmap: &ResumeBitmap) -> Self {
        Self {
            received_ranges: bitmap.received_ranges(),
        }
    }
}

/// §101's own four actions are all unconditional the moment a
/// cancellation happens (stop scheduling, cancel active streams,
/// persist `Cancelled` — none of these has a caller-facing choice, so
/// none gets a field) — the ONE real decision point is "user may
/// optionally keep partial data for resume," which is exactly
/// [`StagingRetentionPolicy`] below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StagingRetentionPolicy {
    DiscardImmediately,
    KeepForResume,
}

// §102 "Pause": contrasted with cancellation, not given a type of its
// own here — "pause retains: journal, partial chunks, metadata" is
// already exactly what [`crate::transfer_journal::TransferJournal`]
// holds (chunk bitmap, bytes verified, retry count) plus the manifest
// itself. Pausing a transfer, in this crate's own terms, means simply
// NOT discarding the `TransferJournal`/manifest — the same data
// [`StagingRetentionPolicy::KeepForResume`] governs for a genuine
// cancellation. The two sections share that one real mechanism; §102
// doesn't need a second one.

/// §103's own five named reasons, verbatim. "This integrates with Part
/// 03 routing policy" — cross-referenced, not depended on: this crate
/// has no `siar-routing-policy` dependency, so this is the vocabulary
/// a real integration would use, not a call into that crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeferredReason {
    WaitingForWifi,
    WaitingForPeer,
    WaitingForStorage,
    WaitingForApproval,
    WaitingForBattery,
}

// §104 "Retry": already satisfied, not new work —
// [`crate::transfer_planner::RetryPolicy`]/
// [`crate::transfer_planner::RetryPolicy::backoff_millis`] already
// operate at "transfer/session level," per attempt, never per raw
// network packet (this crate has no packet-level concept at all to
// retry). "Transport handles lower-level retries" — correctly outside
// this crate's own scope.

/// §105: real, escalating state — not a bare "discard and retry"
/// bool. `failure_count` is the running total across every corrupt
/// chunk seen so far for this transfer (a caller re-supplies its own
/// prior count in), and `should_penalize_source` only flips once that
/// running total crosses a caller-supplied threshold — "repeated
/// corruption may indicate malicious/broken source," not the first
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorruptChunkAction {
    pub failure_count: u32,
    pub should_request_retransmission: bool,
    pub should_penalize_source: bool,
}

pub fn on_corrupt_chunk(prior_failure_count: u32, penalize_threshold: u32) -> CorruptChunkAction {
    let failure_count = prior_failure_count + 1;
    CorruptChunkAction {
        failure_count,
        should_request_retransmission: true,
        should_penalize_source: failure_count >= penalize_threshold,
    }
}

// §106 "Corrupt Manifest": "reject entire transfer offer... do not
// attempt 'best effort' reconstruction." Resolved by construction, not
// a new check — [`crate::manifest::build_manifest`]/
// [`crate::verify::verify_complete_blob`]/
// [`crate::source_trust::verify_manifest_authenticity`] are already
// all-or-nothing; this crate has never had a partial-reconstruction
// code path to disable, so there was nothing to remove.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_blob_verified_allows_marking_a_transfer_complete() {
        assert!(allows_transfer_completion(TransferAckKind::BlobVerified));
        for ack in [
            TransferAckKind::TransportAck,
            TransferAckKind::ChunkAccepted,
            TransferAckKind::UserAccepted,
        ] {
            assert!(
                !allows_transfer_completion(ack),
                "{ack:?} should not be enough"
            );
        }
    }

    #[test]
    fn a_chunk_ack_reports_the_bitmap_s_own_received_ranges() {
        let mut bitmap = ResumeBitmap::new(10);
        for i in [0, 1, 2, 5, 6, 9] {
            bitmap.mark_received(i);
        }
        let ack = ChunkAck::from_bitmap(&bitmap);
        assert_eq!(ack.received_ranges, vec![(0, 3), (5, 7), (9, 10)]);
    }

    #[test]
    fn the_first_corrupt_chunk_requests_retransmission_but_does_not_penalize() {
        let action = on_corrupt_chunk(0, 3);
        assert_eq!(action.failure_count, 1);
        assert!(action.should_request_retransmission);
        assert!(!action.should_penalize_source);
    }

    #[test]
    fn repeated_corruption_crosses_the_threshold_and_penalizes_the_source() {
        let action = on_corrupt_chunk(2, 3);
        assert_eq!(action.failure_count, 3);
        assert!(action.should_penalize_source);
    }
}
