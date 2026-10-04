//! §113 messaging, §114 file, §115 DTN, §116 routing, §117 capability,
//! §118 call, §119 group, §120 sync limits.
//!
//! One typed struct per feature, each with the spec's named fields and
//! `check_*` functions that return *which* limit was hit and by how much
//! ([`LimitViolation`]), so callers can reject precisely and the UI/log
//! can say why. [`FeatureLimits::for_profile`] derives every value from
//! the profile's [`ResourceConfig`] so they cannot contradict it, and
//! [`FeatureLimits::validate`] cross-checks the whole set (a file offer
//! at the maximum size must still fit the maximum manifest; the worst-
//! case chunk buffers of all active transfers must fit the Files memory
//! budget; the largest DTN bundle must fit the relay quota; …).
//!
//! The numbers are this crate's own reasoned defaults (the spec lists
//! field names only). Anything tunable per product should be edited here
//! and re-validated, not hand-set in callers.

use crate::budget::Subsystem;
use crate::negotiation_limits::NegotiationLimits;
use crate::policy_layers::HardLimits;
use crate::profile::{ResourceConfig, ResourceProfile};
use crate::routing_limits::RoutingLimits;

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{field}: {value} exceeds limit {max}")]
pub struct LimitViolation {
    pub field: &'static str,
    pub value: u64,
    pub max: u64,
}

fn check(field: &'static str, value: u64, max: u64) -> Result<(), LimitViolation> {
    if value > max {
        Err(LimitViolation { field, value, max })
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------- §113

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagingLimits {
    pub max_message_bytes: u64,
    pub max_attachments: u64,
    pub max_pending_outbound: u64,
    pub max_inbound_per_sec_per_peer: u64,
    pub max_unresolved_remote_events: u64,
}

impl MessagingLimits {
    pub fn check_message(&self, bytes: u64, attachments: u64) -> Result<(), LimitViolation> {
        check("messaging.max_message_bytes", bytes, self.max_message_bytes)?;
        check(
            "messaging.max_attachments",
            attachments,
            self.max_attachments,
        )
    }
    /// `current` is the count *before* adding one.
    pub fn check_can_queue_outbound(&self, current: u64) -> Result<(), LimitViolation> {
        check(
            "messaging.max_pending_outbound",
            current.saturating_add(1),
            self.max_pending_outbound,
        )
    }
    pub fn check_can_hold_unresolved(&self, current: u64) -> Result<(), LimitViolation> {
        check(
            "messaging.max_unresolved_remote_events",
            current.saturating_add(1),
            self.max_unresolved_remote_events,
        )
    }
}

// ------------------------------------------------------------- §114

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileLimits {
    pub max_file_size: u64,
    pub max_chunk_size: u64,
    pub max_parallel_chunks: u64,
    pub max_active_transfers: u64,
    pub max_staging_bytes: u64,
    pub max_manifest_entries: u64,
}

impl FileLimits {
    /// Entries a manifest for this file/chunk size needs. `None` for a
    /// zero chunk size (malformed).
    pub fn manifest_entries_needed(file_size: u64, chunk_size: u64) -> Option<u64> {
        (chunk_size != 0).then(|| file_size.div_ceil(chunk_size))
    }

    /// Validate an incoming offer *from its declared numbers*, before
    /// anything is allocated or read. The manifest size is derived from
    /// file size and chunk size, so a tiny chunk size cannot be used to
    /// smuggle in an enormous manifest.
    pub fn check_offer(&self, file_size: u64, chunk_size: u64) -> Result<u64, LimitViolation> {
        check("files.max_file_size", file_size, self.max_file_size)?;
        check("files.max_chunk_size", chunk_size, self.max_chunk_size)?;
        let entries =
            Self::manifest_entries_needed(file_size, chunk_size).ok_or(LimitViolation {
                field: "files.chunk_size_is_zero",
                value: 0,
                max: 1,
            })?;
        check(
            "files.max_manifest_entries",
            entries,
            self.max_manifest_entries,
        )?;
        Ok(entries)
    }

    pub fn check_can_start_transfer(&self, active: u64) -> Result<(), LimitViolation> {
        check(
            "files.max_active_transfers",
            active.saturating_add(1),
            self.max_active_transfers,
        )
    }

    pub fn check_staging(&self, staged: u64, additional: u64) -> Result<(), LimitViolation> {
        check(
            "files.max_staging_bytes",
            staged.saturating_add(additional),
            self.max_staging_bytes,
        )
    }

    /// Worst-case bytes of chunk buffers across every allowed transfer.
    pub fn worst_case_chunk_memory(&self) -> u128 {
        u128::from(self.max_active_transfers)
            * u128::from(self.max_parallel_chunks)
            * u128::from(self.max_chunk_size)
    }
}

// ------------------------------------------------------------- §115

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DtnLimits {
    pub max_bundle_bytes: u64,
    pub max_relay_bytes: u64,
    pub max_bundles: u64,
    pub max_replication_budget: u64,
    pub max_hop_limit: u64,
    pub max_peer_intake_per_sec: u64,
}

impl DtnLimits {
    /// Header-level checks on a bundle's *claimed* numbers.
    pub fn check_bundle(
        &self,
        bytes: u64,
        replication_budget: u64,
        hop_limit: u64,
    ) -> Result<(), LimitViolation> {
        check("dtn.max_bundle_bytes", bytes, self.max_bundle_bytes)?;
        check(
            "dtn.max_replication_budget",
            replication_budget,
            self.max_replication_budget,
        )?;
        check("dtn.max_hop_limit", hop_limit, self.max_hop_limit)
    }

    pub fn check_relay_room(
        &self,
        bundles: u64,
        bytes: u64,
        adding_bytes: u64,
    ) -> Result<(), LimitViolation> {
        check(
            "dtn.max_bundles",
            bundles.saturating_add(1),
            self.max_bundles,
        )?;
        check(
            "dtn.max_relay_bytes",
            bytes.saturating_add(adding_bytes),
            self.max_relay_bytes,
        )
    }
}

// ------------------------------------------------------------- §116

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingPolicyLimits {
    pub max_candidates_per_destination: u64,
    pub max_concurrent_path_acquisitions: u64,
    pub max_retry_timers: u64,
    pub max_discovery_escalations: u64,
}

impl RoutingPolicyLimits {
    pub fn check_candidates(&self, n: u64) -> Result<(), LimitViolation> {
        check(
            "routing.max_candidates_per_destination",
            n,
            self.max_candidates_per_destination,
        )
    }
    pub fn check_escalation(&self, level: u64) -> Result<(), LimitViolation> {
        check(
            "routing.max_discovery_escalations",
            level,
            self.max_discovery_escalations,
        )
    }
    /// The runtime counters that enforce these ([`RoutingBackpressure`]).
    ///
    /// [`RoutingBackpressure`]: crate::routing_limits::RoutingBackpressure
    pub fn to_backpressure(&self) -> RoutingLimits {
        RoutingLimits {
            concurrent_connects: self.max_concurrent_path_acquisitions,
            discovery_jobs: self.max_discovery_escalations,
            retry_timers: self.max_retry_timers,
        }
    }
}

// ------------------------------------------------------------- §117

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityPolicyLimits {
    pub max_capabilities: u64,
    pub max_extension_count: u64,
    pub max_parameter_bytes: u64,
    pub max_dynamic_updates_per_sec: u64,
}

impl CapabilityPolicyLimits {
    pub fn check_extension_count(&self, n: u64) -> Result<(), LimitViolation> {
        check(
            "capability.max_extension_count",
            n,
            self.max_extension_count,
        )
    }
    /// Offer-shape limits for [`NegotiationGate`].
    ///
    /// [`NegotiationGate`]: crate::negotiation_limits::NegotiationGate
    pub fn to_negotiation(&self, base: NegotiationLimits) -> NegotiationLimits {
        let each = usize::try_from(self.max_parameter_bytes).unwrap_or(usize::MAX);
        NegotiationLimits {
            max_capabilities: usize::try_from(self.max_capabilities).unwrap_or(usize::MAX),
            max_param_bytes_each: each,
            max_param_bytes_total: base.max_param_bytes_total.max(each),
            ..base
        }
    }
}

// ---------------------------------------------------- §118 / §119 / §120

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallLimits {
    pub max_active_calls: u64,
    pub max_video_tracks: u64,
    pub max_decoder_buffers: u64,
    pub max_encoder_workers: u64,
}

impl CallLimits {
    pub fn check_can_start_call(&self, active: u64) -> Result<(), LimitViolation> {
        check(
            "calls.max_active_calls",
            active.saturating_add(1),
            self.max_active_calls,
        )
    }
    pub fn check_video_tracks(&self, tracks: u64) -> Result<(), LimitViolation> {
        check("calls.max_video_tracks", tracks, self.max_video_tracks)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupLimits {
    pub max_group_members: u64,
    pub max_simultaneous_fanout: u64,
    pub max_update_backlog: u64,
}

impl GroupLimits {
    pub fn check_members(&self, n: u64) -> Result<(), LimitViolation> {
        check("groups.max_group_members", n, self.max_group_members)
    }
    pub fn check_backlog_room(&self, backlog: u64) -> Result<(), LimitViolation> {
        check(
            "groups.max_update_backlog",
            backlog.saturating_add(1),
            self.max_update_backlog,
        )
    }
    /// A message to `members` recipients is sent in waves of at most
    /// `max_simultaneous_fanout`.
    pub fn fanout_waves(&self, members: u64) -> u64 {
        members.div_ceil(self.max_simultaneous_fanout.max(1))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLimits {
    pub max_events_per_batch: u64,
    pub max_concurrent_stream_syncs: u64,
    pub max_history_backfill_bytes: u64,
}

impl SyncLimits {
    pub fn check_batch(&self, events: u64) -> Result<(), LimitViolation> {
        check(
            "sync.max_events_per_batch",
            events,
            self.max_events_per_batch,
        )
    }
    pub fn check_backfill(&self, bytes_so_far: u64, next: u64) -> Result<(), LimitViolation> {
        check(
            "sync.max_history_backfill_bytes",
            bytes_so_far.saturating_add(next),
            self.max_history_backfill_bytes,
        )
    }
}

// ---------------------------------------------------------- aggregate

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureLimits {
    pub messaging: MessagingLimits,
    pub files: FileLimits,
    pub dtn: DtnLimits,
    pub routing: RoutingPolicyLimits,
    pub capability: CapabilityPolicyLimits,
    pub calls: CallLimits,
    pub groups: GroupLimits,
    pub sync: SyncLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeatureLimitError {
    #[error("{0} must be non-zero")]
    Zero(&'static str),
    #[error("{field} ({value}) exceeds what the resource config allows ({config_max})")]
    ExceedsConfig {
        field: &'static str,
        value: u128,
        config_max: u128,
    },
    #[error("inconsistent: {0}")]
    Inconsistent(&'static str),
}

impl FeatureLimits {
    pub fn for_profile(p: ResourceProfile) -> Self {
        use ResourceProfile::*;
        let c = ResourceConfig::for_profile(p);
        // (chunk, parallel chunks, max file, groups members, fanout, calls)
        let (chunk, parallel, max_file, members, fanout, calls, video, dec, enc) = match p {
            Embedded => (64 * KIB, 2, 256 * MIB, 64, 4, 1, 1, 2, 1),
            MobileLow => (256 * KIB, 4, GIB, 128, 4, 1, 2, 4, 1),
            MobileNormal => (MIB, 8, 4 * GIB, 256, 8, 2, 4, 8, 2),
            Desktop => (MIB, 8, 16 * GIB, 1_024, 16, 4, 8, 16, 4),
            Server => (512 * KIB, 2, 16 * GIB, 5_000, 64, 64, 64, 128, 16),
        };
        let hard = HardLimits::for_profile(p);
        let manifest = max_file.div_ceil(chunk);
        let scale = |base: u64| -> u64 {
            let (n, d) = match p {
                Embedded => (1, 4),
                MobileLow => (1, 2),
                MobileNormal => (1, 1),
                Desktop => (2, 1),
                Server => (8, 1),
            };
            (base * n / d).max(1)
        };
        Self {
            messaging: MessagingLimits {
                max_message_bytes: 64 * KIB,
                max_attachments: 10,
                max_pending_outbound: scale(1_000),
                max_inbound_per_sec_per_peer: 5,
                max_unresolved_remote_events: scale(1_000),
            },
            files: FileLimits {
                max_file_size: max_file,
                max_chunk_size: chunk,
                max_parallel_chunks: parallel,
                max_active_transfers: u64::from(c.network.max_transfers),
                max_staging_bytes: c.storage.quota_bytes / 4,
                max_manifest_entries: manifest,
            },
            dtn: DtnLimits {
                // A bundle must fit one frame and the relay store.
                max_bundle_bytes: MIB
                    .min(c.storage.relay_quota_bytes)
                    .min(hard.max_frame_bytes),
                max_relay_bytes: c.storage.relay_quota_bytes,
                max_bundles: scale(10_000),
                max_replication_budget: 8,
                max_hop_limit: 16,
                max_peer_intake_per_sec: scale(20),
            },
            routing: RoutingPolicyLimits {
                max_candidates_per_destination: 8,
                max_concurrent_path_acquisitions: scale(8).min(u64::from(c.network.max_sessions)),
                max_retry_timers: scale(64),
                max_discovery_escalations: 3,
            },
            capability: CapabilityPolicyLimits {
                max_capabilities: 256,
                max_extension_count: 32,
                max_parameter_bytes: 256,
                max_dynamic_updates_per_sec: 10,
            },
            calls: CallLimits {
                max_active_calls: calls,
                max_video_tracks: video,
                max_decoder_buffers: dec,
                max_encoder_workers: enc,
            },
            groups: GroupLimits {
                max_group_members: members,
                max_simultaneous_fanout: fanout.min(u64::from(c.network.max_streams)),
                max_update_backlog: scale(1_000),
            },
            sync: SyncLimits {
                max_events_per_batch: 500,
                max_concurrent_stream_syncs: scale(2),
                max_history_backfill_bytes: 64 * MIB,
            },
        }
    }

    pub fn validate(
        &self,
        cfg: &ResourceConfig,
        hard: &HardLimits,
    ) -> Result<(), Vec<FeatureLimitError>> {
        use FeatureLimitError::*;
        let mut e = Vec::new();

        for (name, v) in [
            (
                "messaging.max_message_bytes",
                self.messaging.max_message_bytes,
            ),
            (
                "messaging.max_pending_outbound",
                self.messaging.max_pending_outbound,
            ),
            ("files.max_file_size", self.files.max_file_size),
            ("files.max_chunk_size", self.files.max_chunk_size),
            ("files.max_parallel_chunks", self.files.max_parallel_chunks),
            (
                "files.max_active_transfers",
                self.files.max_active_transfers,
            ),
            (
                "files.max_manifest_entries",
                self.files.max_manifest_entries,
            ),
            ("dtn.max_bundle_bytes", self.dtn.max_bundle_bytes),
            ("dtn.max_bundles", self.dtn.max_bundles),
            ("dtn.max_hop_limit", self.dtn.max_hop_limit),
            (
                "routing.max_candidates_per_destination",
                self.routing.max_candidates_per_destination,
            ),
            (
                "routing.max_concurrent_path_acquisitions",
                self.routing.max_concurrent_path_acquisitions,
            ),
            (
                "capability.max_capabilities",
                self.capability.max_capabilities,
            ),
            ("calls.max_active_calls", self.calls.max_active_calls),
            ("groups.max_group_members", self.groups.max_group_members),
            (
                "groups.max_simultaneous_fanout",
                self.groups.max_simultaneous_fanout,
            ),
            ("sync.max_events_per_batch", self.sync.max_events_per_batch),
            (
                "sync.max_concurrent_stream_syncs",
                self.sync.max_concurrent_stream_syncs,
            ),
        ] {
            if v == 0 {
                e.push(Zero(name));
            }
        }

        let over =
            |field: &'static str, value: u128, config_max: u128, e: &mut Vec<FeatureLimitError>| {
                if value > config_max {
                    e.push(ExceedsConfig {
                        field,
                        value,
                        config_max,
                    });
                }
            };
        over(
            "files.max_active_transfers",
            u128::from(self.files.max_active_transfers),
            u128::from(cfg.network.max_transfers),
            &mut e,
        );
        over(
            "files.max_staging_bytes",
            u128::from(self.files.max_staging_bytes),
            u128::from(
                cfg.storage
                    .quota_bytes
                    .saturating_sub(cfg.storage.critical_reserve_bytes),
            ),
            &mut e,
        );
        over(
            "files.worst_case_chunk_memory",
            self.files.worst_case_chunk_memory(),
            u128::from(cfg.memory.sub_budgets[Subsystem::Files as usize]),
            &mut e,
        );
        over(
            "dtn.max_relay_bytes",
            u128::from(self.dtn.max_relay_bytes),
            u128::from(cfg.storage.relay_quota_bytes),
            &mut e,
        );
        over(
            "routing.max_concurrent_path_acquisitions",
            u128::from(self.routing.max_concurrent_path_acquisitions),
            u128::from(cfg.network.max_sessions),
            &mut e,
        );
        over(
            "groups.max_simultaneous_fanout",
            u128::from(self.groups.max_simultaneous_fanout),
            u128::from(cfg.network.max_streams),
            &mut e,
        );
        over(
            "messaging.max_message_bytes",
            u128::from(self.messaging.max_message_bytes),
            u128::from(hard.max_event_bytes),
            &mut e,
        );
        over(
            "files.max_chunk_size",
            u128::from(self.files.max_chunk_size),
            u128::from(hard.max_frame_bytes),
            &mut e,
        );
        over(
            "dtn.max_bundle_bytes",
            u128::from(self.dtn.max_bundle_bytes),
            u128::from(hard.max_frame_bytes),
            &mut e,
        );
        over(
            "files.max_manifest_bytes_estimate",
            u128::from(self.files.max_manifest_entries) * 64,
            u128::from(hard.max_manifest_bytes),
            &mut e,
        );

        if self.dtn.max_bundle_bytes > self.dtn.max_relay_bytes {
            e.push(Inconsistent(
                "a single bundle can be larger than the whole relay store",
            ));
        }
        let needed = self
            .files
            .max_file_size
            .div_ceil(self.files.max_chunk_size.max(1));
        if needed > self.files.max_manifest_entries {
            e.push(Inconsistent(
                "the largest allowed file needs more manifest entries than allowed",
            ));
        }
        if self.files.max_parallel_chunks > 0
            && self.files.max_chunk_size > self.files.max_staging_bytes
        {
            e.push(Inconsistent(
                "a single chunk is larger than the staging area",
            ));
        }
        if e.is_empty() {
            Ok(())
        } else {
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ResourceProfile; 5] = [
        ResourceProfile::Embedded,
        ResourceProfile::MobileLow,
        ResourceProfile::MobileNormal,
        ResourceProfile::Desktop,
        ResourceProfile::Server,
    ];

    #[test]
    fn every_profile_derives_a_consistent_limit_set() {
        for p in ALL {
            let f = FeatureLimits::for_profile(p);
            let cfg = ResourceConfig::for_profile(p);
            assert_eq!(
                f.validate(&cfg, &HardLimits::for_profile(p)),
                Ok(()),
                "{p:?}"
            );
        }
    }

    #[test]
    fn validation_catches_contradictions() {
        let p = ResourceProfile::MobileNormal;
        let (cfg, hard) = (ResourceConfig::for_profile(p), HardLimits::for_profile(p));
        let mut f = FeatureLimits::for_profile(p);
        f.files.max_active_transfers = u64::from(cfg.network.max_transfers) + 1;
        f.files.max_parallel_chunks = 1_000_000;
        f.dtn.max_bundle_bytes = f.dtn.max_relay_bytes + 1;
        f.files.max_manifest_entries = 1;
        f.calls.max_active_calls = 0;
        let errs = f.validate(&cfg, &hard).unwrap_err();
        assert!(errs.contains(&FeatureLimitError::Zero("calls.max_active_calls")));
        assert!(errs.iter().any(|e| matches!(
            e,
            FeatureLimitError::ExceedsConfig {
                field: "files.worst_case_chunk_memory",
                ..
            }
        )));
        assert!(errs.iter().any(|e| matches!(
            e,
            FeatureLimitError::ExceedsConfig {
                field: "files.max_active_transfers",
                ..
            }
        )));
        assert!(errs.iter().any(|e| matches!(e, FeatureLimitError::Inconsistent(m) if m.contains("larger than the whole relay"))));
        assert!(errs.iter().any(
            |e| matches!(e, FeatureLimitError::Inconsistent(m) if m.contains("manifest entries"))
        ));
    }

    #[test]
    fn messaging_checks_name_the_limit_hit() {
        let m = FeatureLimits::for_profile(ResourceProfile::MobileNormal).messaging;
        assert!(m.check_message(64 * KIB, 10).is_ok());
        assert_eq!(
            m.check_message(64 * KIB + 1, 0).unwrap_err().field,
            "messaging.max_message_bytes"
        );
        assert_eq!(
            m.check_message(1, 11).unwrap_err(),
            LimitViolation {
                field: "messaging.max_attachments",
                value: 11,
                max: 10
            }
        );
        assert!(m
            .check_can_queue_outbound(m.max_pending_outbound - 1)
            .is_ok());
        assert!(m.check_can_queue_outbound(m.max_pending_outbound).is_err());
        assert!(
            m.check_can_hold_unresolved(u64::MAX).is_err(),
            "no overflow"
        );
    }

    #[test]
    fn a_tiny_chunk_size_cannot_smuggle_in_a_huge_manifest() {
        let f = FeatureLimits::for_profile(ResourceProfile::MobileNormal).files;
        assert_eq!(f.check_offer(4 * GIB, MIB), Ok(4_096));
        // 4 GiB in 1-byte chunks would be 4 billion manifest entries
        assert_eq!(
            f.check_offer(4 * GIB, 1).unwrap_err().field,
            "files.max_manifest_entries"
        );
        assert_eq!(
            f.check_offer(1, 0).unwrap_err().field,
            "files.chunk_size_is_zero"
        );
        assert_eq!(
            f.check_offer(4 * GIB + 1, MIB).unwrap_err().field,
            "files.max_file_size"
        );
        assert_eq!(
            f.check_offer(10, 2 * MIB).unwrap_err().field,
            "files.max_chunk_size"
        );
    }

    #[test]
    fn the_largest_allowed_file_always_fits_the_manifest() {
        for p in ALL {
            let f = FeatureLimits::for_profile(p).files;
            assert!(
                f.check_offer(f.max_file_size, f.max_chunk_size).is_ok(),
                "{p:?}"
            );
        }
    }

    #[test]
    fn file_staging_transfer_and_memory_math() {
        let f = FeatureLimits::for_profile(ResourceProfile::MobileNormal).files;
        assert!(f
            .check_can_start_transfer(f.max_active_transfers - 1)
            .is_ok());
        assert!(f.check_can_start_transfer(f.max_active_transfers).is_err());
        assert!(f.check_staging(f.max_staging_bytes, 0).is_ok());
        assert!(f.check_staging(f.max_staging_bytes, 1).is_err());
        assert!(f.check_staging(u64::MAX, 1).is_err());
        assert_eq!(f.worst_case_chunk_memory(), 4 * 8 * u128::from(MIB));
    }

    #[test]
    fn dtn_checks() {
        let d = FeatureLimits::for_profile(ResourceProfile::MobileNormal).dtn;
        assert!(d.check_bundle(MIB, 8, 16).is_ok());
        let emb = FeatureLimits::for_profile(ResourceProfile::Embedded).dtn;
        assert_eq!(
            emb.max_bundle_bytes,
            256 * KIB,
            "bounded by the hard frame limit"
        );
        assert_eq!(
            d.check_bundle(MIB + 1, 1, 1).unwrap_err().field,
            "dtn.max_bundle_bytes"
        );
        assert_eq!(
            d.check_bundle(1, 9, 1).unwrap_err().field,
            "dtn.max_replication_budget"
        );
        assert_eq!(
            d.check_bundle(1, 1, 17).unwrap_err().field,
            "dtn.max_hop_limit"
        );
        assert!(d.check_relay_room(0, d.max_relay_bytes - 10, 10).is_ok());
        assert_eq!(
            d.check_relay_room(0, d.max_relay_bytes - 10, 11)
                .unwrap_err()
                .field,
            "dtn.max_relay_bytes"
        );
        assert_eq!(
            d.check_relay_room(d.max_bundles, 0, 1).unwrap_err().field,
            "dtn.max_bundles"
        );
    }

    #[test]
    fn routing_and_capability_limits_feed_the_runtime_counters() {
        let f = FeatureLimits::for_profile(ResourceProfile::MobileNormal);
        let rb = f.routing.to_backpressure();
        assert_eq!(
            rb.concurrent_connects,
            f.routing.max_concurrent_path_acquisitions
        );
        assert_eq!(rb.retry_timers, f.routing.max_retry_timers);
        let n = f.capability.to_negotiation(NegotiationLimits::default());
        assert_eq!((n.max_capabilities, n.max_param_bytes_each), (256, 256));
        assert!(f.routing.check_candidates(8).is_ok() && f.routing.check_candidates(9).is_err());
        assert!(f.routing.check_escalation(4).is_err());
        assert!(f.capability.check_extension_count(33).is_err());
    }

    #[test]
    fn call_group_and_sync_limits() {
        let f = FeatureLimits::for_profile(ResourceProfile::MobileNormal);
        assert!(f.calls.check_can_start_call(1).is_ok());
        assert!(f.calls.check_can_start_call(2).is_err());
        assert!(f.calls.check_video_tracks(5).is_err());
        assert!(f.groups.check_members(256).is_ok() && f.groups.check_members(257).is_err());
        assert_eq!(f.groups.fanout_waves(256), 32);
        assert_eq!(f.groups.fanout_waves(0), 0);
        assert!(f.groups.check_backlog_room(1_000).is_err());
        assert!(f.sync.check_batch(500).is_ok() && f.sync.check_batch(501).is_err());
        assert!(f.sync.check_backfill(64 * MIB - 1, 1).is_ok());
        assert!(f.sync.check_backfill(64 * MIB, 1).is_err());
        assert!(f.sync.check_backfill(u64::MAX, 1).is_err());
    }

    #[test]
    fn limits_never_get_looser_for_smaller_devices() {
        let e = FeatureLimits::for_profile(ResourceProfile::Embedded);
        let d = FeatureLimits::for_profile(ResourceProfile::Desktop);
        assert!(e.messaging.max_pending_outbound <= d.messaging.max_pending_outbound);
        assert!(e.files.max_active_transfers <= d.files.max_active_transfers);
        assert!(e.dtn.max_relay_bytes <= d.dtn.max_relay_bytes);
        assert!(e.calls.max_active_calls <= d.calls.max_active_calls);
        assert!(e.groups.max_group_members <= d.groups.max_group_members);
    }
}
