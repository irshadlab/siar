//! §18 "File Transfer Recovery", §19 "Chunk Commit Ordering", §20
//! "Chunk State Ambiguity".
//!
//! §19's own commit pipeline — write staging bytes → flush → verify
//! hash/auth → persist chunk-complete → ACK chunk — has more stages
//! than `siar_blob_manifest::resume::ResumeBitmap` tracks: that type
//! (already real, from an earlier pass of this workspace's Part 05
//! crate) is a binary received/not-received bitset, which is exactly
//! right for *what to request from a peer* but cannot represent §20's
//! actual hazard — "bytes were written but the completion bit was
//! never committed." A binary bitmap has no way to distinguish "safely
//! verified" from "written but never confirmed," so this module adds
//! the finer per-chunk state §19-20 need, as a genuine complement to
//! `ResumeBitmap` rather than a competing replacement for it: this
//! module answers "for a chunk marked received, is it actually safe to
//! trust," `ResumeBitmap` still answers "which chunks are missing at
//! all."

use serde::{Deserialize, Serialize};
use siar_blob_manifest::ids::BlobId;
use std::collections::HashMap;

/// A local addition — no `TransferId` exists anywhere in
/// `siar-blob-manifest` yet (checked directly; §18 names the concept
/// but nothing upstream defines it), so this crate defines the
/// minimal opaque id it needs rather than either blocking on an
/// upstream addition or silently reusing [`BlobId`] for a distinct
/// concept (the same blob could in principle be the subject of more
/// than one transfer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferId(uuid::Uuid);

impl TransferId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub const fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn new_for_tests(n: u16) -> Self {
        Self(uuid::Uuid::from_u128(n as u128 + 1))
    }
}

impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

/// §19's own commit-ordering stages, restricted to the ones that
/// actually need to be durably distinguishable for recovery to reason
/// about (the "write bytes"/"flush" steps aren't states in their own
/// right — they're preconditions for reaching
/// [`ChunkRecoveryState::WrittenUnverified`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChunkRecoveryState {
    /// Bytes are on disk (flushed), but hash/auth verification has not
    /// been durably confirmed. §20's exact hazard state — a crash
    /// landing here must never be read as "this chunk is fine."
    WrittenUnverified,
    /// Hash/auth verified and that fact is durably persisted.
    Verified,
    /// The peer has been (or is being) told this chunk is confirmed.
    Acked,
}

pub fn can_transition(from: ChunkRecoveryState, to: ChunkRecoveryState) -> bool {
    use ChunkRecoveryState::*;
    matches!(
        (from, to),
        (WrittenUnverified, Verified) | (Verified, Acked)
    )
}

/// §20's own instruction ("treat chunk as unverified, reverify on
/// restart. Do not assume") turned into an exhaustive decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkRecoveryAction {
    /// No record at all, or only `WrittenUnverified` — §20's rule
    /// applies to both identically: never assume, always recheck the
    /// actual bytes against the chunk's expected hash before trusting
    /// them.
    Reverify,
    /// Verification was durably confirmed but the ACK may not have
    /// gone out (or arrived) before the crash — safe to (re)send.
    ResendAck,
    NothingToDo,
}

/// §20's exact reconciliation rule for one chunk's persisted state (or
/// lack of one).
pub fn reconcile_chunk_on_restart(persisted: Option<ChunkRecoveryState>) -> ChunkRecoveryAction {
    match persisted {
        None | Some(ChunkRecoveryState::WrittenUnverified) => ChunkRecoveryAction::Reverify,
        Some(ChunkRecoveryState::Verified) => ChunkRecoveryAction::ResendAck,
        Some(ChunkRecoveryState::Acked) => ChunkRecoveryAction::NothingToDo,
    }
}

/// §18's own restart sequence ("load transfer, validate staging,
/// recompute/verify uncertain chunks") applied across a whole
/// transfer's chunks at once — the per-chunk decision from
/// [`reconcile_chunk_on_restart`], batched.
#[derive(Debug, Clone, Default)]
pub struct ChunkRecoveryMap {
    transfer: Option<(TransferId, BlobId)>,
    states: HashMap<u32, ChunkRecoveryState>,
}

impl ChunkRecoveryMap {
    pub fn new(transfer: TransferId, blob: BlobId) -> Self {
        Self {
            transfer: Some((transfer, blob)),
            states: HashMap::new(),
        }
    }

    pub fn record_state(&mut self, chunk_index: u32, state: ChunkRecoveryState) {
        self.states.insert(chunk_index, state);
    }

    pub fn state_of(&self, chunk_index: u32) -> Option<ChunkRecoveryState> {
        self.states.get(&chunk_index).copied()
    }

    /// The `(TransferId, BlobId)` this map was created for, if any —
    /// a default-constructed map (via `Default`) has none.
    pub fn transfer(&self) -> Option<(TransferId, BlobId)> {
        self.transfer
    }

    /// The restart plan for chunks `0..total_chunks` — every index,
    /// not just the ones with a recorded state, since an index with no
    /// record at all is itself one of [`reconcile_chunk_on_restart`]'s
    /// two `Reverify` cases.
    pub fn restart_actions(&self, total_chunks: u32) -> Vec<(u32, ChunkRecoveryAction)> {
        (0..total_chunks)
            .map(|i| (i, reconcile_chunk_on_restart(self.state_of(i))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_ordering_is_linear() {
        use ChunkRecoveryState::*;
        assert!(can_transition(WrittenUnverified, Verified));
        assert!(can_transition(Verified, Acked));
        assert!(!can_transition(WrittenUnverified, Acked));
    }

    #[test]
    fn no_record_at_all_is_reverified_not_assumed_missing_or_fine() {
        assert_eq!(
            reconcile_chunk_on_restart(None),
            ChunkRecoveryAction::Reverify
        );
    }

    #[test]
    fn written_but_unverified_is_20s_exact_hazard_case_reverify() {
        let action = reconcile_chunk_on_restart(Some(ChunkRecoveryState::WrittenUnverified));
        assert_eq!(action, ChunkRecoveryAction::Reverify);
    }

    #[test]
    fn verified_but_possibly_unacked_resends_the_ack() {
        let action = reconcile_chunk_on_restart(Some(ChunkRecoveryState::Verified));
        assert_eq!(action, ChunkRecoveryAction::ResendAck);
    }

    #[test]
    fn fully_acked_needs_nothing_further() {
        let action = reconcile_chunk_on_restart(Some(ChunkRecoveryState::Acked));
        assert_eq!(action, ChunkRecoveryAction::NothingToDo);
    }

    #[test]
    fn transfer_accessor_returns_what_the_map_was_constructed_with() {
        let transfer_id = TransferId::new();
        let blob_id = BlobId::from_ciphertext(b"x");
        let map = ChunkRecoveryMap::new(transfer_id, blob_id);
        assert_eq!(map.transfer(), Some((transfer_id, blob_id)));
    }

    #[test]
    fn restart_plan_covers_every_chunk_including_ones_with_no_record() {
        let mut map = ChunkRecoveryMap::new(TransferId::new(), BlobId::from_ciphertext(b"x"));
        map.record_state(0, ChunkRecoveryState::Acked);
        map.record_state(1, ChunkRecoveryState::WrittenUnverified);
        // chunk 2 has no record at all.

        let plan = map.restart_actions(3);
        assert_eq!(
            plan,
            vec![
                (0, ChunkRecoveryAction::NothingToDo),
                (1, ChunkRecoveryAction::Reverify),
                (2, ChunkRecoveryAction::Reverify),
            ]
        );
    }
}
