//! Nothing used to call [`TransferEngine::gc_sweep`]: GC existed and
//! never ran. This is the schedule — runtime-agnostic on purpose (no
//! timer, no executor, no thread): the host's own loop, or its battery
//! and thermal policy (Part 13), calls [`GcScheduler::tick`] whenever it
//! has a moment, and the scheduler decides whether a sweep is *due*.
//!
//! A sweep that fails does not count as having run, so it is retried on
//! the next tick instead of waiting out a full interval.

use std::collections::HashSet;
use std::sync::Mutex;

use siar_blob_manifest::gc::GcReport;
use siar_blob_manifest::ids::BlobId;
use siar_blob_manifest::metadata_store::BlobMetadataStore;
use siar_blob_manifest::reference_store::ReferenceStore;
use siar_blob_manifest::storage_lifecycle::GcPolicy;

use crate::transfer_engine::{EngineError, TransferEngine};

pub struct GcScheduler {
    interval_millis: u64,
    page: usize,
    last_success: Mutex<Option<u64>>,
}

impl GcScheduler {
    /// `page` bounds how many metadata rows a sweep holds at once.
    pub fn new(interval_millis: u64, page: usize) -> Self {
        Self {
            interval_millis,
            page: page.max(1),
            last_success: Mutex::new(None),
        }
    }

    /// Never run yet counts as due. A clock that went backwards is not
    /// "due" (it would otherwise sweep on every tick until time caught up).
    pub fn is_due(&self, now_millis: u64) -> bool {
        match *self.last_success.lock().expect("gc schedule") {
            None => true,
            Some(last) => now_millis.saturating_sub(last) >= self.interval_millis,
        }
    }

    /// Sweeps if due. `Ok(None)` means "not due".
    pub fn tick<M, R>(
        &self,
        engine: &TransferEngine<M, R>,
        pinned: &HashSet<BlobId>,
        now_millis: u64,
        policy: &GcPolicy,
    ) -> Result<Option<GcReport>, EngineError>
    where
        M: BlobMetadataStore,
        R: ReferenceStore + ?Sized,
    {
        if !self.is_due(now_millis) {
            return Ok(None);
        }
        let report = engine.gc_sweep(pinned, now_millis, policy, self.page)?;
        *self.last_success.lock().expect("gc schedule") = Some(now_millis);
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_when_never_run_then_only_after_the_interval() {
        let s = GcScheduler::new(1_000, 64);
        assert!(s.is_due(0));
        *s.last_success.lock().unwrap() = Some(5_000);
        assert!(!s.is_due(5_999));
        assert!(s.is_due(6_000));
    }

    #[test]
    fn a_backwards_clock_is_never_due() {
        let s = GcScheduler::new(1_000, 64);
        *s.last_success.lock().unwrap() = Some(9_000);
        assert!(!s.is_due(1));
    }

    #[test]
    fn page_is_at_least_one() {
        assert_eq!(GcScheduler::new(1, 0).page, 1);
    }
}
