//! §108 "Rate Limiting", §110 "Resource Policy", §111 "Battery
//! Awareness", §112 "Memory Pressure", §113 "Thermal Pressure".
//!
//! §107 "Multiple Sources" has its real type in
//! [`crate::transfer_control::SourceFailureTracker`], not here — see
//! that type's own doc comment.
//!
//! §109 "Abuse Protection" is proven, not just mapped in prose, by
//! `tests/abuse_protection_matrix.rs` — one test per named threat,
//! each against the real mitigation named below rather than a
//! seventh, redundant type. Huge fake size / millions of chunks →
//! [`crate::limits::ManifestLimits`]'s own size and chunk-count
//! ceilings, enforced inside [`crate::manifest::build_manifest`] before
//! any hashing happens; never-completing transfers →
//! [`crate::source_integrity::source_mutated`]/[`crate::file_offer::FileOffer::is_expired`];
//! hash mismatch spam →
//! [`crate::transfer_control::on_corrupt_chunk`]'s own escalating
//! penalty; storage exhaustion →
//! [`crate::storage_lifecycle::StoragePressure`]; offer spam → this
//! module's own [`RateLimits`] below.

use serde::{Deserialize, Serialize};

/// §108's own four named per-peer limits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimits {
    pub max_offers_per_sec: f64,
    pub max_active_transfers: u32,
    pub max_bytes_per_sec: u64,
    pub max_stored_partial_bytes: u64,
}

/// §108: "unknown peers get stricter limits" — the same two-tier shape
/// this workspace's sibling `siar-dtn-bundle` crate already uses for
/// storage quota (`admission::PeerQuotaPolicy`, see `docs/06`),
/// applied here to rate instead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    pub unknown_peer: RateLimits,
    pub known_peer: RateLimits,
}

impl RateLimitPolicy {
    pub fn limits_for(&self, is_known_peer: bool) -> RateLimits {
        if is_known_peer {
            self.known_peer
        } else {
            self.unknown_peer
        }
    }
}

/// §110's own struct, transcribed field-for-field from the spec's own
/// literal Rust snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileResourcePolicy {
    pub max_parallel_transfers: usize,
    pub max_parallel_chunks: usize,
    pub max_buffer_bytes: usize,
    pub max_staging_bytes: u64,
    pub max_file_size: u64,
}

/// §111's own four named knobs. "Part 13 will deepen this" — Part 13
/// (Battery-Aware Scheduling) doesn't exist yet in this workspace (see
/// `ROADMAP.md`'s own cross-spec build-order analysis, where it's
/// actually the single highest-priority spec after Tier 0); this is
/// the minimal, honest set of knobs this crate can expose today
/// without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryAwareAdjustments {
    pub pause_background_bulk: bool,
    pub reduce_concurrency: bool,
    pub prefer_wifi: bool,
    pub avoid_expensive_discovery: bool,
}

/// §112's own four named actions. "Do not lose verified progress" is
/// structural, not a fifth field: none of these four actions touch
/// [`crate::transfer_journal::TransferJournal::bytes_verified`] or its
/// chunk bitmap at all — shrinking a buffer pool or pausing background
/// transfers has nothing to say about data that already passed
/// verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryPressureAdjustments {
    pub shrink_buffer_pool: bool,
    pub reduce_parallel_chunks: bool,
    pub release_previews: bool,
    pub pause_background_transfers: bool,
}

/// §113's own two named actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThermalPressureAdjustments {
    pub reduce_concurrency: bool,
    pub defer_noncritical_bulk: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_peers_get_the_stricter_configured_limits() {
        let policy = RateLimitPolicy {
            unknown_peer: RateLimits {
                max_offers_per_sec: 1.0,
                max_active_transfers: 1,
                max_bytes_per_sec: 10_000,
                max_stored_partial_bytes: 50_000,
            },
            known_peer: RateLimits {
                max_offers_per_sec: 10.0,
                max_active_transfers: 5,
                max_bytes_per_sec: 1_000_000,
                max_stored_partial_bytes: 5_000_000,
            },
        };
        assert_eq!(policy.limits_for(false).max_active_transfers, 1);
        assert_eq!(policy.limits_for(true).max_active_transfers, 5);
    }
}
