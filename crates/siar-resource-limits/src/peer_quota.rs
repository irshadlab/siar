//! §29 "Per-Peer Quotas", §30 "Trust-Aware Quotas".
//!
//! §28's "Hierarchical Accounting" (runtime → extension → peer →
//! operation) and §31-32's extension-side quotas are deliberately not
//! attempted this pass — §28's third level needs an `OperationId`-like
//! type nothing in this workspace defines yet, and folding it into a
//! shallow stand-in here would be guessing at a shape rather than
//! building the real thing. This module covers the one level (§29's
//! per-peer quota) the spec gives a complete, concrete field list for.

use crate::admission::{AdmissionResult, DeferredReason, DropReason};
use crate::token_bucket::TokenBucket;
use serde::{Deserialize, Serialize};

/// §30, verbatim variant list, in the spec's own low-to-high order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TrustClass {
    Unknown,
    Known,
    Verified,
    Organization,
    Authority,
    LocalOwnDevice,
}

/// §29, verbatim field list ("Recommended limits"). `max_requests_per_sec`
/// deliberately isn't enforced by a counter on this struct — see
/// [`PeerQuota::request_rate_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerQuota {
    pub max_active_streams: u32,
    pub max_pending_messages: u32,
    pub max_inbound_bytes: u64,
    pub max_staged_files: u32,
    pub max_dtn_relay_bytes: u64,
    pub max_requests_per_sec: u64,
}

impl PeerQuota {
    /// §29's own closing line: "Unknown peers get smaller defaults."
    /// No concrete numbers are given anywhere in Part 08's text for
    /// any trust tier — every value below is this module's own
    /// reasoned choice (doubling-ish growth per tier, documented
    /// inline), not a transcribed spec default.
    ///
    /// §30's hard rule this function exists specifically to satisfy:
    /// "Never make trusted peers unlimited." Even
    /// [`TrustClass::LocalOwnDevice`] gets a large-but-finite quota,
    /// never `u64::MAX`/`u32::MAX` — checked by this module's own test
    /// rather than left as an unstated assumption.
    pub fn for_trust_class(trust: TrustClass) -> Self {
        match trust {
            TrustClass::Unknown => Self {
                max_active_streams: 4,
                max_pending_messages: 32,
                max_inbound_bytes: 4 * 1024 * 1024,
                max_staged_files: 2,
                max_dtn_relay_bytes: 1024 * 1024,
                max_requests_per_sec: 5,
            },
            TrustClass::Known => Self {
                max_active_streams: 16,
                max_pending_messages: 128,
                max_inbound_bytes: 32 * 1024 * 1024,
                max_staged_files: 8,
                max_dtn_relay_bytes: 8 * 1024 * 1024,
                max_requests_per_sec: 20,
            },
            TrustClass::Verified => Self {
                max_active_streams: 32,
                max_pending_messages: 512,
                max_inbound_bytes: 128 * 1024 * 1024,
                max_staged_files: 32,
                max_dtn_relay_bytes: 32 * 1024 * 1024,
                max_requests_per_sec: 50,
            },
            TrustClass::Organization => Self {
                max_active_streams: 64,
                max_pending_messages: 2048,
                max_inbound_bytes: 512 * 1024 * 1024,
                max_staged_files: 128,
                max_dtn_relay_bytes: 128 * 1024 * 1024,
                max_requests_per_sec: 100,
            },
            TrustClass::Authority => Self {
                max_active_streams: 128,
                max_pending_messages: 8192,
                max_inbound_bytes: 1024 * 1024 * 1024,
                max_staged_files: 256,
                max_dtn_relay_bytes: 512 * 1024 * 1024,
                max_requests_per_sec: 200,
            },
            // Finite, not unlimited — §30's explicit constraint.
            TrustClass::LocalOwnDevice => Self {
                max_active_streams: 512,
                max_pending_messages: 65536,
                max_inbound_bytes: 8 * 1024 * 1024 * 1024,
                max_staged_files: 1024,
                max_dtn_relay_bytes: 2 * 1024 * 1024 * 1024,
                max_requests_per_sec: 1000,
            },
        }
    }

    /// §53's `TokenBucket` is the real enforcement mechanism for
    /// `max_requests_per_sec` — a rate, not a one-shot pool, the same
    /// reasoning `admission::admit`'s own doc comment already gives
    /// for not re-checking `bandwidth_class` against a counter. Burst
    /// capacity is set equal to the per-second rate itself: enough to
    /// absorb one full second of legitimate traffic arriving at once,
    /// without licensing sustained bursting beyond the configured rate.
    /// §173 "peer quota <= global quota". The per-trust table above is
    /// device-independent (a `LocalOwnDevice` peer may hold 512 streams),
    /// but an embedded node's *entire* stream budget is 16. Clamp each
    /// dimension that has a global counterpart to that global limit; this
    /// can only tighten, never raise, a quota.
    pub fn clamped_to(&self, cfg: &crate::profile::ResourceConfig) -> Self {
        Self {
            max_active_streams: self.max_active_streams.min(cfg.network.max_streams),
            max_dtn_relay_bytes: self.max_dtn_relay_bytes.min(cfg.storage.relay_quota_bytes),
            ..*self
        }
    }

    pub fn request_rate_bucket(&self, now_millis: u64) -> TokenBucket {
        TokenBucket::new(
            self.max_requests_per_sec,
            self.max_requests_per_sec,
            now_millis,
        )
    }
}

/// Live outstanding usage for one peer, mirroring [`PeerQuota`]'s
/// field names 1:1 (minus the rate dimension, which
/// [`PeerQuota::request_rate_bucket`] tracks separately) so charging
/// against a quota is a direct field-by-field comparison rather than
/// a lookup through an indirection layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerUsageCounters {
    pub active_streams: u32,
    pub pending_messages: u32,
    pub inbound_bytes: u64,
    pub staged_files: u32,
    pub dtn_relay_bytes: u64,
}

/// What's being requested against a peer's quota — the delta
/// [`PeerUsageCounters::try_charge`] would add if admitted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerUsageDelta {
    pub active_streams: u32,
    pub pending_messages: u32,
    pub inbound_bytes: u64,
    pub staged_files: u32,
    pub dtn_relay_bytes: u64,
}

impl PeerUsageCounters {
    /// Admits `delta` against `quota` if every dimension would stay
    /// within its own max after charging — all five checked
    /// independently, so (for example) a peer maxed out on
    /// `pending_messages` can still be rejected/deferred even if every
    /// other dimension has room, matching §29's framing of these as
    /// separate, independently-enforced limits rather than one
    /// combined score.
    ///
    /// `durable` follows this crate's established §21-22 split
    /// (`admission::admit`, `queue::BoundedPriorityQueue::enqueue`):
    /// work that's worth retrying later gets [`AdmissionResult::Deferred`],
    /// disposable work gets [`AdmissionResult::Dropped`].
    pub fn try_charge(
        &mut self,
        quota: &PeerQuota,
        delta: PeerUsageDelta,
        durable: bool,
    ) -> AdmissionResult {
        let fits = self.active_streams.saturating_add(delta.active_streams)
            <= quota.max_active_streams
            && self.pending_messages.saturating_add(delta.pending_messages)
                <= quota.max_pending_messages
            && self.inbound_bytes.saturating_add(delta.inbound_bytes) <= quota.max_inbound_bytes
            && self.staged_files.saturating_add(delta.staged_files) <= quota.max_staged_files
            && self.dtn_relay_bytes.saturating_add(delta.dtn_relay_bytes)
                <= quota.max_dtn_relay_bytes;

        if !fits {
            return if durable {
                AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
            } else {
                AdmissionResult::Dropped(DropReason::Stale)
            };
        }

        self.active_streams += delta.active_streams;
        self.pending_messages += delta.pending_messages;
        self.inbound_bytes += delta.inbound_bytes;
        self.staged_files += delta.staged_files;
        self.dtn_relay_bytes += delta.dtn_relay_bytes;
        AdmissionResult::Accepted
    }

    /// Gives back previously-charged usage (a stream closed, staged
    /// files were committed/discarded, ...) — saturating, so a
    /// double-release can't underflow into a huge wraparound count.
    pub fn release(&mut self, delta: PeerUsageDelta) {
        self.active_streams = self.active_streams.saturating_sub(delta.active_streams);
        self.pending_messages = self.pending_messages.saturating_sub(delta.pending_messages);
        self.inbound_bytes = self.inbound_bytes.saturating_sub(delta.inbound_bytes);
        self.staged_files = self.staged_files.saturating_sub(delta.staged_files);
        self.dtn_relay_bytes = self.dtn_relay_bytes.saturating_sub(delta.dtn_relay_bytes);
    }
}

#[cfg(test)]
mod clamp_tests {
    use super::*;
    use crate::profile::{ResourceConfig, ResourceProfile};

    const TRUSTS: [TrustClass; 6] = [
        TrustClass::Unknown,
        TrustClass::Known,
        TrustClass::Verified,
        TrustClass::Organization,
        TrustClass::Authority,
        TrustClass::LocalOwnDevice,
    ];

    #[test]
    fn unclamped_quotas_can_exceed_a_small_device_which_is_why_clamping_exists() {
        let emb = ResourceConfig::for_profile(ResourceProfile::Embedded);
        assert!(
            PeerQuota::for_trust_class(TrustClass::LocalOwnDevice).max_active_streams
                > emb.network.max_streams
        );
    }

    #[test]
    fn clamped_quota_never_exceeds_the_global_limits_and_never_loosens() {
        for p in [
            ResourceProfile::Embedded,
            ResourceProfile::MobileLow,
            ResourceProfile::MobileNormal,
            ResourceProfile::Desktop,
            ResourceProfile::Server,
        ] {
            let cfg = ResourceConfig::for_profile(p);
            for t in TRUSTS {
                let raw = PeerQuota::for_trust_class(t);
                let c = raw.clamped_to(&cfg);
                assert!(
                    c.max_active_streams <= cfg.network.max_streams,
                    "{p:?}/{t:?}"
                );
                assert!(
                    c.max_dtn_relay_bytes <= cfg.storage.relay_quota_bytes,
                    "{p:?}/{t:?}"
                );
                assert!(
                    c.max_active_streams <= raw.max_active_streams
                        && c.max_dtn_relay_bytes <= raw.max_dtn_relay_bytes
                );
                assert_eq!(
                    (
                        c.max_pending_messages,
                        c.max_inbound_bytes,
                        c.max_staged_files,
                        c.max_requests_per_sec
                    ),
                    (
                        raw.max_pending_messages,
                        raw.max_inbound_bytes,
                        raw.max_staged_files,
                        raw.max_requests_per_sec
                    )
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trust_tier_including_local_own_device_is_finite_not_unlimited() {
        // §30's hard rule, checked directly rather than assumed.
        for trust in [
            TrustClass::Unknown,
            TrustClass::Known,
            TrustClass::Verified,
            TrustClass::Organization,
            TrustClass::Authority,
            TrustClass::LocalOwnDevice,
        ] {
            let quota = PeerQuota::for_trust_class(trust);
            assert_ne!(quota.max_active_streams, u32::MAX);
            assert_ne!(quota.max_pending_messages, u32::MAX);
            assert_ne!(quota.max_inbound_bytes, u64::MAX);
            assert_ne!(quota.max_staged_files, u32::MAX);
            assert_ne!(quota.max_dtn_relay_bytes, u64::MAX);
            assert_ne!(quota.max_requests_per_sec, u64::MAX);
        }
    }

    #[test]
    fn higher_trust_strictly_increases_every_quota_dimension() {
        // §30: "Higher trust can increase quotas" — checked across the
        // full ordered tier list, not just Unknown vs LocalOwnDevice.
        let tiers = [
            TrustClass::Unknown,
            TrustClass::Known,
            TrustClass::Verified,
            TrustClass::Organization,
            TrustClass::Authority,
            TrustClass::LocalOwnDevice,
        ];
        for pair in tiers.windows(2) {
            let lower = PeerQuota::for_trust_class(pair[0]);
            let higher = PeerQuota::for_trust_class(pair[1]);
            assert!(higher.max_active_streams > lower.max_active_streams);
            assert!(higher.max_pending_messages > lower.max_pending_messages);
            assert!(higher.max_inbound_bytes > lower.max_inbound_bytes);
            assert!(higher.max_staged_files > lower.max_staged_files);
            assert!(higher.max_dtn_relay_bytes > lower.max_dtn_relay_bytes);
            assert!(higher.max_requests_per_sec > lower.max_requests_per_sec);
        }
    }

    #[test]
    fn unknown_peer_gets_the_smallest_defaults() {
        // §29's own closing line, checked directly against the other
        // five tiers rather than just the adjacent one.
        let unknown = PeerQuota::for_trust_class(TrustClass::Unknown);
        for trust in [
            TrustClass::Known,
            TrustClass::Verified,
            TrustClass::Organization,
            TrustClass::Authority,
            TrustClass::LocalOwnDevice,
        ] {
            assert!(
                PeerQuota::for_trust_class(trust).max_active_streams > unknown.max_active_streams
            );
        }
    }

    #[test]
    fn charge_within_quota_succeeds_and_updates_counters() {
        let quota = PeerQuota::for_trust_class(TrustClass::Known);
        let mut usage = PeerUsageCounters::default();
        let delta = PeerUsageDelta {
            active_streams: 1,
            ..Default::default()
        };

        assert_eq!(
            usage.try_charge(&quota, delta, true),
            AdmissionResult::Accepted
        );
        assert_eq!(usage.active_streams, 1);
    }

    #[test]
    fn durable_charge_past_quota_defers_without_mutating_usage() {
        let quota = PeerQuota::for_trust_class(TrustClass::Unknown); // max_active_streams: 4
        let mut usage = PeerUsageCounters {
            active_streams: 4,
            ..Default::default()
        };
        let delta = PeerUsageDelta {
            active_streams: 1,
            ..Default::default()
        };

        let result = usage.try_charge(&quota, delta, true);
        assert_eq!(
            result,
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
        // Rejected charge must not have partially applied.
        assert_eq!(usage.active_streams, 4);
    }

    #[test]
    fn non_durable_charge_past_quota_drops_instead_of_deferring() {
        let quota = PeerQuota::for_trust_class(TrustClass::Unknown);
        let mut usage = PeerUsageCounters {
            active_streams: 4,
            ..Default::default()
        };
        let delta = PeerUsageDelta {
            active_streams: 1,
            ..Default::default()
        };

        let result = usage.try_charge(&quota, delta, false);
        assert_eq!(result, AdmissionResult::Dropped(DropReason::Stale));
    }

    #[test]
    fn each_quota_dimension_is_enforced_independently() {
        // A peer maxed out on pending_messages must be blocked even
        // though every other dimension has plenty of room.
        let quota = PeerQuota::for_trust_class(TrustClass::Known); // max_pending_messages: 128
        let mut usage = PeerUsageCounters {
            pending_messages: 128,
            ..Default::default()
        };
        let delta = PeerUsageDelta {
            pending_messages: 1,
            ..Default::default()
        };

        assert_eq!(
            usage.try_charge(&quota, delta, true),
            AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
        );
    }

    #[test]
    fn release_gives_back_charged_capacity() {
        let quota = PeerQuota::for_trust_class(TrustClass::Known);
        let mut usage = PeerUsageCounters::default();
        let delta = PeerUsageDelta {
            staged_files: 3,
            ..Default::default()
        };

        usage.try_charge(&quota, delta, true);
        assert_eq!(usage.staged_files, 3);
        usage.release(delta);
        assert_eq!(usage.staged_files, 0);
    }

    #[test]
    fn request_rate_bucket_starts_full_at_the_configured_rate() {
        let quota = PeerQuota::for_trust_class(TrustClass::Known); // 20 req/s
        let mut bucket = quota.request_rate_bucket(0);
        // A full second's worth of requests should be admittable at once.
        assert!(bucket.try_consume(20, 0));
        assert!(!bucket.try_consume(1, 0));
    }
}
