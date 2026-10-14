//! §151 "GC Mark-and-Sweep Option", §152 "Orphan Detection", §153
//! "Partial Blob Retention" (with §150's recount folded into the sweep).
//!
//! ## The sweep (§151)
//!
//! ```text
//! for every stored blob:
//!   references?  yes ──▶ clear any grace mark; repair a drifted cached count
//!   no ─▶ pinned / mid-transfer / cache / relay? ──▶ leave alone
//!         no ─▶ first time seen unreferenced? ──▶ start the grace clock
//!               grace elapsed & still unreferenced? ──▶ delete
//! ```
//!
//! Three properties are the point of doing it as a periodic sweep rather
//! than "delete when a counter hits zero":
//!
//! - **Nothing is deleted the moment it becomes unreferenced.** The
//!   first sweep that sees it only *starts a grace period*
//!   ([`GcPolicy::grace_period_millis`]); a reference added in the
//!   meantime — a message re-shared, an undo — clears the mark and the
//!   blob lives. This is also what absorbs the one race a sweep cannot
//!   close by itself (a reference added between the check and the
//!   delete): the check is repeated immediately before deleting, and
//!   anything that slips through the remaining instant would have to
//!   have been unreferenced for the *whole* grace period first.
//! - **The references are the truth, the count is a cache (§150).**
//!   Every blob visited has its [`crate::schema_rows::BlobRow::reference_count`] compared
//!   with [`ReferenceStore::count_for`]; a mismatch is *repaired* and
//!   counted in [`GcReport::counts_repaired`], never trusted.
//! - **Age alone never deletes anything.** [`is_gc_eligible`] is the one
//!   rule, and it ignores creation time entirely — only "how long
//!   unreferenced".
//!
//! What the sweep deliberately does **not** touch: [`BlobClassification::
//! Cache`] (cache eviction, §65, owns those — an unreferenced cache copy
//! is normal, not garbage) and [`BlobClassification::Relay`] (a relay
//! holds data for *someone else* until its own [`crate::expiry::
//! RelayExpiry`]). Pinned blobs and blobs with an in-progress transfer
//! are skipped and any stale grace mark is cleared, so protection
//! restarts the clock rather than leaving a nearly-expired one behind.
//!
//! Run [`crate::fs_store::FsBlobStore::reconcile_metadata`] first so
//! every file on disk has a row for the sweep to visit; a file with no
//! row is invisible to it.
//!
//! ## Orphaned and partial staging (§152, §153)
//!
//! A crash can leave staging data whose transfer no longer exists.
//! [`sweep_staging`] separates three cases, each with its own typed
//! expiry ([`crate::expiry::TransferExpiry`]):
//!
//! | staging | kept? |
//! |---|---|
//! | a writer for it is open in this process | always |
//! | its transfer is known and **not** past its `TransferExpiry` | yes — kept for resume (§153) |
//! | its transfer is known and **is** past its `TransferExpiry` | removed |
//! | no transfer knows it (orphan) | removed once older than the orphan grace |
//!
//! "User cancel" is the caller removing the transfer from the map (so
//! it falls into the orphan row) or calling
//! [`crate::fs_store::FsBlobStore::discard_staging`] directly.
//! "Storage pressure" is [`reclaim_staging`]: oldest orphans first, then
//! oldest resumable, never an active one, stopping the moment enough
//! space is freed.

use std::collections::{HashMap, HashSet};

use crate::blob_references::ReferenceSet;
use crate::expiry::TransferExpiry;
use crate::fs_store::FsBlobStore;
use crate::ids::BlobId;
use crate::metadata_store::BlobMetadataStore;
use crate::reference_store::ReferenceStore;
use crate::storage_lifecycle::{is_gc_eligible, BlobClassification, GcPolicy};
use crate::store_error::BlobError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcReport {
    pub scanned: u64,
    pub referenced: u64,
    /// Cached `reference_count`s that disagreed with the real
    /// references and were corrected (§150).
    pub counts_repaired: u32,
    /// Unreferenced blobs first seen this sweep: grace clock started.
    pub grace_started: u32,
    /// Unreferenced, but the grace period has not yet elapsed.
    pub deferred_in_grace: u32,
    pub deleted: u32,
    pub bytes_freed: u64,
    pub protected_pinned: u32,
    pub protected_active: u32,
    /// `Cache` and `Relay` blobs, owned by other policies.
    pub skipped_by_class: u32,
    /// References naming a blob this store does not hold — normal for
    /// an attachment whose download hasn't happened; informational.
    pub references_without_blob: u32,
}

/// See the module doc. `page` bounds how many rows are held at once.
pub fn sweep<M, R>(
    store: &FsBlobStore<M>,
    refs: &R,
    pinned: &HashSet<BlobId>,
    active_transfers: &HashSet<BlobId>,
    now_millis: u64,
    policy: &GcPolicy,
    page: usize,
) -> Result<GcReport, BlobError>
where
    M: BlobMetadataStore,
    R: ReferenceStore + ?Sized,
{
    let page = page.max(1);
    let meta = store.metadata();
    let empty = ReferenceSet::new();
    let mut report = GcReport::default();
    let mut cursor: Option<BlobId> = None;

    loop {
        let rows = meta.list_after(cursor, page)?;
        let Some(last) = rows.last() else { break };
        cursor = Some(last.blob_id);

        for mut row in rows {
            report.scanned += 1;
            let id = row.blob_id;
            let count = refs.count_for(id)?;

            // §150: the cached count is derived; correct it.
            if row.reference_count as u64 != count {
                row.reference_count = count as usize;
                meta.put(row)?;
                report.counts_repaired += 1;
            }

            if count > 0 {
                report.referenced += 1;
                if refs.unreferenced_since(id)?.is_some() {
                    refs.set_unreferenced_since(id, None)?;
                }
                continue;
            }

            if matches!(
                row.storage_class,
                BlobClassification::Cache | BlobClassification::Relay
            ) {
                report.skipped_by_class += 1;
                continue;
            }
            if pinned.contains(&id) {
                report.protected_pinned += 1;
                refs.set_unreferenced_since(id, None)?;
                continue;
            }
            if active_transfers.contains(&id) || store.is_active(id) {
                report.protected_active += 1;
                refs.set_unreferenced_since(id, None)?;
                continue;
            }

            let (since, newly_started) = match refs.unreferenced_since(id)? {
                Some(s) => (s, false),
                None => {
                    refs.set_unreferenced_since(id, Some(now_millis))?;
                    (now_millis, true)
                }
            };
            if is_gc_eligible(&empty, Some(since), now_millis, policy, false) {
                // Re-check at the last instant: a reference added
                // since the count above must win.
                if refs.count_for(id)? > 0 {
                    refs.set_unreferenced_since(id, None)?;
                    continue;
                }
                store.delete_sync(id)?;
                refs.set_unreferenced_since(id, None)?;
                report.deleted += 1;
                report.bytes_freed += row.size;
            } else if newly_started {
                report.grace_started += 1;
            } else {
                report.deferred_in_grace += 1;
            }
        }
    }

    // Informational cross-check in the other direction.
    let mut ref_cursor: Option<BlobId> = None;
    loop {
        let ids = refs.referenced_blob_ids(ref_cursor, page)?;
        let Some(last) = ids.last() else { break };
        ref_cursor = Some(*last);
        for id in ids {
            if meta.get(id)?.is_none() {
                report.references_without_blob += 1;
            }
        }
    }
    Ok(report)
}

/// §153's age-based half. See the module doc's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialRetentionPolicy {
    /// How long an *orphaned* staging (no transfer knows it) may sit
    /// before it is removed.
    pub orphan_grace_millis: u64,
}

impl Default for PartialRetentionPolicy {
    fn default() -> Self {
        Self {
            orphan_grace_millis: 24 * 3_600_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StagingSweepReport {
    pub kept_active: u32,
    pub kept_for_resume: u32,
    pub kept_orphan_in_grace: u32,
    pub removed_expired_transfer: u32,
    pub removed_orphan: u32,
    pub bytes_freed: u64,
}

pub fn sweep_staging<M: BlobMetadataStore>(
    store: &FsBlobStore<M>,
    resumable: &HashMap<BlobId, TransferExpiry>,
    now_millis: u64,
    policy: &PartialRetentionPolicy,
) -> Result<StagingSweepReport, BlobError> {
    let mut r = StagingSweepReport::default();
    for s in store.list_staging()? {
        if store.is_active(s.blob_id) {
            r.kept_active += 1;
            continue;
        }
        match resumable.get(&s.blob_id) {
            Some(expiry) if !expiry.is_expired(now_millis) => r.kept_for_resume += 1,
            Some(_) => {
                r.bytes_freed += store.discard_staging(s.blob_id)?;
                r.removed_expired_transfer += 1;
            }
            None => {
                let age = now_millis.saturating_sub(s.modified_millis);
                if age >= policy.orphan_grace_millis {
                    r.bytes_freed += store.discard_staging(s.blob_id)?;
                    r.removed_orphan += 1;
                } else {
                    r.kept_orphan_in_grace += 1;
                }
            }
        }
    }
    Ok(r)
}

/// §153's "storage pressure": free at least `bytes_needed` from
/// staging. Orphans go first (oldest first), then resumable transfers
/// (oldest first); an active writer is never touched. Returns
/// `(entries_removed, bytes_freed)`; if there is not enough to free,
/// frees what it legitimately can.
pub fn reclaim_staging<M: BlobMetadataStore>(
    store: &FsBlobStore<M>,
    resumable: &HashMap<BlobId, TransferExpiry>,
    bytes_needed: u64,
) -> Result<(u32, u64), BlobError> {
    let mut staging: Vec<_> = store
        .list_staging()?
        .into_iter()
        .filter(|s| !store.is_active(s.blob_id))
        .collect();
    // false (orphan) sorts before true (resumable); then oldest first.
    staging.sort_by_key(|s| (resumable.contains_key(&s.blob_id), s.modified_millis));
    let (mut removed, mut freed) = (0u32, 0u64);
    for s in staging {
        if freed >= bytes_needed {
            break;
        }
        freed += store.discard_staging(s.blob_id)?;
        removed += 1;
    }
    Ok((removed, freed))
}
