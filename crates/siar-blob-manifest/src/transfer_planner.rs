//! §35 "Transfer Planner", §36 "Transport Independence".
//!
//! §35's own diagram names six responsibilities. Three of them —
//! **route request generation** and, by extension, the transfer engine
//! itself actually driving a transport — are explicitly Part 03's own
//! job ("It consumes Part 03 routing decisions"), and this crate has
//! never depended on `siar-routing-policy` (see this module's own §36
//! note below for why that's a feature, not a gap). What this module
//! gives Part 03 is [`RouteRequirements`] — the sizing data a router
//! needs to make its own decision — rather than reaching for
//! `siar-routing-policy`'s own types and creating a dependency this
//! crate has never needed. The other three responsibilities —
//! **destination resolution** (trivial: a transfer has exactly one
//! peer, [`siar_domain::DeviceId`]), **chunk scheduling**, and
//! **concurrency limits/retry planning** — are real decisions this
//! module makes.

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;

use crate::manifest::BlobManifest;
use crate::resume::ResumeBitmap;

/// §35's own "route request generation," scoped to what this crate can
/// honestly provide without depending on `siar-routing-policy`: the
/// two numbers a router needs to reason about a transfer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteRequirements {
    pub total_size: u64,
    pub chunk_count: u32,
}

pub fn route_requirements(manifest: &BlobManifest) -> RouteRequirements {
    RouteRequirements {
        total_size: manifest.total_size,
        chunk_count: manifest.chunks.len() as u32,
    }
}

/// §35's own "retry planning."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_base_millis: u64,
}

impl RetryPolicy {
    /// Exponential backoff, doubling per attempt from
    /// `backoff_base_millis` — the same shape §35's own "retry
    /// planning" implies without spelling out a formula. `attempt` is
    /// 0-based (the first retry after an initial failure is attempt
    /// 0).
    pub fn backoff_millis(&self, attempt: u32) -> u64 {
        self.backoff_base_millis
            .saturating_mul(1u64 << attempt.min(32))
    }
}

/// §35's own "chunk scheduling" and "concurrency limits," plus
/// destination resolution and the retry policy above, gathered into
/// one real plan a transfer engine (not built in this crate — see
/// `lib.rs`'s own scope note) would actually execute against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPlan {
    pub destination: DeviceId,
    /// Chunk indices in the order they'll be sent — NOT always `0..n`;
    /// see [`resume_chunk_schedule`] for the resume-aware ordering a
    /// real transfer engine should prefer once a transfer has already
    /// started.
    pub chunk_schedule: Vec<u32>,
    pub concurrency_limit: u32,
    pub retry_policy: RetryPolicy,
}

/// A brand-new transfer's plan: every chunk, in ascending order —
/// there's no resume state yet to prioritize against.
pub fn plan_new_transfer(
    destination: DeviceId,
    manifest: &BlobManifest,
    concurrency_limit: u32,
    retry_policy: RetryPolicy,
) -> TransferPlan {
    TransferPlan {
        destination,
        chunk_schedule: (0..manifest.chunks.len() as u32).collect(),
        concurrency_limit,
        retry_policy,
    }
}

/// A resuming transfer's plan: missing chunks first, in ascending
/// order — the real "chunk scheduling" decision §35 asks for once a
/// transfer has partial progress, so a receiver's own
/// [`crate::partial_availability::partial_availability`] leading-bytes
/// count grows as fast as possible rather than waiting on whichever
/// chunk happens to be scheduled last.
pub fn resume_chunk_schedule(bitmap: &ResumeBitmap) -> Vec<u32> {
    (0..bitmap.chunk_count() as u32)
        .filter(|&i| !bitmap.is_received(i))
        .collect()
}

// §36 "Transport Independence": "Transfer engine must not contain
// Iroh-specific send()/Bluetooth-specific write(); instead, a
// Transport/Session abstraction." Like §24, a structural property, not
// a type: this crate has never depended on `siar-transport`,
// `siar-transport-ble`, or any other transport crate, and
// [`TransferPlan`] above carries nothing transport-specific — a
// `DeviceId`, a chunk order, a concurrency number, a retry policy. A
// `cargo tree -i siar-transport` from this crate's own manifest finds
// nothing, for the same reason `cargo tree -i siar-messaging` finds
// nothing for §24: there was never a dependency to remove.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::ManifestLimits;

    fn sample_manifest() -> BlobManifest {
        let ciphertext = vec![0u8; 300];
        crate::manifest::build_manifest(&ciphertext, 100, &ManifestLimits::default()).unwrap()
    }

    #[test]
    fn route_requirements_reflect_the_manifest_s_own_size_and_chunk_count() {
        let manifest = sample_manifest();
        let requirements = route_requirements(&manifest);
        assert_eq!(requirements.total_size, 300);
        assert_eq!(requirements.chunk_count, 3);
    }

    #[test]
    fn backoff_doubles_per_attempt_from_the_base() {
        let policy = RetryPolicy {
            max_attempts: 5,
            backoff_base_millis: 100,
        };
        assert_eq!(policy.backoff_millis(0), 100);
        assert_eq!(policy.backoff_millis(1), 200);
        assert_eq!(policy.backoff_millis(2), 400);
    }

    #[test]
    fn a_new_transfer_schedules_every_chunk_in_order() {
        let manifest = sample_manifest();
        let plan = plan_new_transfer(
            DeviceId::new(),
            &manifest,
            4,
            RetryPolicy {
                max_attempts: 3,
                backoff_base_millis: 50,
            },
        );
        assert_eq!(plan.chunk_schedule, vec![0, 1, 2]);
        assert_eq!(plan.concurrency_limit, 4);
    }

    #[test]
    fn a_resuming_transfer_schedules_only_the_missing_chunks() {
        let mut bitmap = ResumeBitmap::new(5);
        bitmap.mark_received(0);
        bitmap.mark_received(1);
        bitmap.mark_received(4);
        assert_eq!(resume_chunk_schedule(&bitmap), vec![2, 3]);
    }

    #[test]
    fn a_fully_received_bitmap_schedules_nothing_further() {
        let mut bitmap = ResumeBitmap::new(2);
        bitmap.mark_received(0);
        bitmap.mark_received(1);
        assert!(resume_chunk_schedule(&bitmap).is_empty());
    }
}
