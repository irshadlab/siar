//! §12-16 device resource profiles, §141 resource config, §142 config
//! validation.
//!
//! Every number below is this crate's own reasoned default — the spec
//! names the profiles and their *character* (embedded: small and low
//! concurrency; mobile-low: 4 GB RAM, battery sensitive; desktop: more of
//! everything "without becoming unbounded"; server: high concurrency with
//! *stricter* per-peer/tenant quotas) but gives no figures except the
//! 256 MiB memory split in §10, which `MobileNormal` reproduces.
//! `validate` is the safety net: every profile must pass it (tested).

use crate::budget::Subsystem;
use crate::policy_layers::HardLimits;
use crate::queue::QueueCapacities;

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceProfile {
    Embedded,
    MobileLow,
    MobileNormal,
    Desktop,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryLimits {
    pub global_bytes: u64,
    /// Guarantees per subsystem, in [`Subsystem::ALL`] order.
    pub sub_budgets: [u64; Subsystem::COUNT],
    /// How far one subsystem may exceed its guarantee (§11).
    pub max_borrow_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageLimits {
    pub quota_bytes: u64,
    pub critical_reserve_bytes: u64,
    pub relay_quota_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkLimits {
    pub max_sessions: u32,
    pub max_sessions_per_peer: u32,
    pub max_streams: u32,
    pub max_streams_per_peer: u32,
    pub max_transfers: u32,
    pub max_transfers_per_peer: u32,
    pub bandwidth_bytes_per_sec: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerLimits {
    pub max_tracked_peers: u32,
    /// Unknown peers admitted concurrently (§129).
    pub max_unknown_peers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueLimits {
    pub capacities: QueueCapacities,
    /// Largest item a queue slot may hold.
    pub max_item_bytes: u64,
    /// Total memory all queues together may pin.
    pub memory_budget_bytes: u64,
}

impl QueueLimits {
    fn total_slots(&self) -> u64 {
        let c = &self.capacities;
        [
            c.critical,
            c.control,
            c.interactive,
            c.normal,
            c.bulk,
            c.background,
        ]
        .iter()
        .map(|&n| n as u64)
        .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceConfig {
    pub profile: ResourceProfile,
    pub memory: MemoryLimits,
    pub storage: StorageLimits,
    pub network: NetworkLimits,
    pub peers: PeerLimits,
    pub queues: QueueLimits,
    /// §143 "data saver": background transfers are throttled hard.
    pub data_saver: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} must be non-zero")]
    Zero(&'static str),
    #[error("sub-budgets sum to {sum}, over the global memory budget {global}")]
    SubBudgetsExceedGlobal { sum: u64, global: u64 },
    #[error("critical storage reserve {reserve} exceeds storage quota {quota}")]
    CriticalReserveExceedsQuota { reserve: u64, quota: u64 },
    #[error("relay quota {relay} exceeds storage quota {quota}")]
    RelayQuotaExceedsQuota { relay: u64, quota: u64 },
    #[error("per-peer {what} ({per_peer}) exceeds global ({global})")]
    PerPeerExceedsGlobal {
        what: &'static str,
        per_peer: u32,
        global: u32,
    },
    #[error("transfers ({transfers}) exceed streams ({streams}); every transfer needs a stream")]
    TransfersExceedStreams { transfers: u32, streams: u32 },
    #[error("queues could pin {needed} bytes, over the queue memory budget {budget}")]
    QueuesExceedMemory { needed: u64, budget: u64 },
    #[error("bulk queue alone could pin {needed} bytes, over the queue memory budget {budget}")]
    BulkQueueExceedsMemory { needed: u64, budget: u64 },
    #[error("queue memory budget {budget} exceeds the global memory budget {global}")]
    QueueBudgetExceedsGlobal { budget: u64, global: u64 },
    #[error("{what} ({value}) exceeds hard ceiling ({ceiling})")]
    ExceedsHardCeiling {
        what: &'static str,
        value: u64,
        ceiling: u64,
    },
}

/// Splits `global` by the §10 example proportions
/// (files 3/8, calls 1/4, messaging, dtn and "other" 1/8 each); any
/// rounding remainder goes to Files so the parts sum exactly.
pub fn split_memory(global: u64) -> [u64; Subsystem::COUNT] {
    let eighth = global / 8;
    let mut parts = [0u64; Subsystem::COUNT];
    parts[Subsystem::Messaging as usize] = eighth;
    parts[Subsystem::Dtn as usize] = eighth;
    parts[Subsystem::Extensions as usize] = eighth;
    parts[Subsystem::Calls as usize] = global / 4;
    parts[Subsystem::Files as usize] = global - 3 * eighth - global / 4;
    parts
}

impl HardLimits {
    /// Ceilings sit at 2× the profile's defaults: room for an admin to
    /// raise a quota (§143), never room for unbounded growth.
    pub fn for_profile(p: ResourceProfile) -> Self {
        let d = ResourceConfig::for_profile(p);
        let small = p == ResourceProfile::Embedded;
        Self {
            max_frame_bytes: if small { 256 * KIB } else { MIB },
            max_event_bytes: if small { 64 * KIB } else { 256 * KIB },
            max_manifest_bytes: if small { MIB } else { 16 * MIB },
            max_concurrent_streams: u64::from(d.network.max_streams) * 2,
            max_memory_buffer_bytes: d.memory.global_bytes * 2,
            max_dtn_storage_bytes: d.storage.relay_quota_bytes * 2,
            max_pending_outbox_ops: if small { 1_000 } else { 100_000 },
        }
    }
}

impl ResourceConfig {
    pub fn for_profile(p: ResourceProfile) -> Self {
        // (mem MiB, storage, reserve, relay, sessions/peer, streams/peer,
        //  transfers/peer, tracked peers, unknown peers, queue caps, item, queue mem MiB)
        use ResourceProfile::*;
        let caps = |c, k, i, n, b, g| QueueCapacities {
            critical: c,
            control: k,
            interactive: i,
            normal: n,
            bulk: b,
            background: g,
        };
        let (
            mem_mib,
            quota,
            reserve,
            relay,
            sess,
            sess_pp,
            streams,
            streams_pp,
            xfers,
            xfers_pp,
            tracked,
            unknown,
            qcaps,
            item,
            qmem_mib,
        ) = match p {
            Embedded => (
                32,
                256 * MIB,
                32 * MIB,
                64 * MIB,
                8,
                2,
                16,
                4,
                2,
                1,
                64,
                4,
                caps(8, 8, 32, 32, 8, 4),
                16 * KIB,
                4,
            ),
            MobileLow => (
                128,
                2 * GIB,
                128 * MIB,
                256 * MIB,
                16,
                2,
                32,
                8,
                2,
                1,
                256,
                8,
                caps(16, 16, 128, 128, 32, 8),
                64 * KIB,
                24,
            ),
            MobileNormal => (
                256,
                8 * GIB,
                512 * MIB,
                GIB,
                32,
                3,
                64,
                16,
                4,
                2,
                512,
                16,
                QueueCapacities::conservative_default(),
                64 * KIB,
                64,
            ),
            Desktop => (
                512,
                64 * GIB,
                2 * GIB,
                8 * GIB,
                128,
                4,
                256,
                32,
                8,
                3,
                2_048,
                32,
                caps(64, 64, 512, 512, 128, 32),
                64 * KIB,
                96,
            ),
            Server => (
                4_096,
                1_024 * GIB,
                16 * GIB,
                512 * GIB,
                10_000,
                8,
                40_000,
                16,
                1_024,
                4,
                100_000,
                256,
                caps(256, 256, 2_048, 2_048, 512, 128),
                64 * KIB,
                512,
            ),
        };
        let global = mem_mib * MIB;
        Self {
            profile: p,
            memory: MemoryLimits {
                global_bytes: global,
                sub_budgets: split_memory(global),
                max_borrow_bytes: global / 4,
            },
            storage: StorageLimits {
                quota_bytes: quota,
                critical_reserve_bytes: reserve,
                relay_quota_bytes: relay,
            },
            network: NetworkLimits {
                max_sessions: sess,
                max_sessions_per_peer: sess_pp,
                max_streams: streams,
                max_streams_per_peer: streams_pp,
                max_transfers: xfers,
                max_transfers_per_peer: xfers_pp,
                bandwidth_bytes_per_sec: None,
            },
            peers: PeerLimits {
                max_tracked_peers: tracked,
                max_unknown_peers: unknown,
            },
            queues: QueueLimits {
                capacities: qcaps,
                max_item_bytes: item,
                memory_budget_bytes: qmem_mib * MIB,
            },
            data_saver: false,
        }
    }

    /// Transfers actually allowed right now: data saver cuts parallelism
    /// to a quarter (never below one, so transfers still progress).
    pub fn effective_max_transfers(&self) -> u32 {
        if self.data_saver {
            (self.network.max_transfers / 4).max(1)
        } else {
            self.network.max_transfers
        }
    }

    /// §142. Returns *every* violation, not just the first, so a bad
    /// config file can be fixed in one pass.
    pub fn validate(&self, hard: &HardLimits) -> Result<(), Vec<ConfigError>> {
        let mut e = Vec::new();
        let n = &self.network;
        for (name, v) in [
            ("memory.global_bytes", self.memory.global_bytes),
            ("storage.quota_bytes", self.storage.quota_bytes),
            ("network.max_sessions", u64::from(n.max_sessions)),
            (
                "network.max_sessions_per_peer",
                u64::from(n.max_sessions_per_peer),
            ),
            ("network.max_streams", u64::from(n.max_streams)),
            (
                "network.max_streams_per_peer",
                u64::from(n.max_streams_per_peer),
            ),
            ("network.max_transfers", u64::from(n.max_transfers)),
            (
                "network.max_transfers_per_peer",
                u64::from(n.max_transfers_per_peer),
            ),
            (
                "peers.max_tracked_peers",
                u64::from(self.peers.max_tracked_peers),
            ),
            ("queues.max_item_bytes", self.queues.max_item_bytes),
        ] {
            if v == 0 {
                e.push(ConfigError::Zero(name));
            }
        }

        let sum = self
            .memory
            .sub_budgets
            .iter()
            .fold(0u64, |a, &b| a.saturating_add(b));
        if sum > self.memory.global_bytes {
            e.push(ConfigError::SubBudgetsExceedGlobal {
                sum,
                global: self.memory.global_bytes,
            });
        }
        if self.storage.critical_reserve_bytes > self.storage.quota_bytes {
            e.push(ConfigError::CriticalReserveExceedsQuota {
                reserve: self.storage.critical_reserve_bytes,
                quota: self.storage.quota_bytes,
            });
        }
        if self.storage.relay_quota_bytes > self.storage.quota_bytes {
            e.push(ConfigError::RelayQuotaExceedsQuota {
                relay: self.storage.relay_quota_bytes,
                quota: self.storage.quota_bytes,
            });
        }
        for (what, per_peer, global) in [
            ("sessions", n.max_sessions_per_peer, n.max_sessions),
            ("streams", n.max_streams_per_peer, n.max_streams),
            ("transfers", n.max_transfers_per_peer, n.max_transfers),
        ] {
            if per_peer > global {
                e.push(ConfigError::PerPeerExceedsGlobal {
                    what,
                    per_peer,
                    global,
                });
            }
        }
        if n.max_transfers > n.max_streams {
            e.push(ConfigError::TransfersExceedStreams {
                transfers: n.max_transfers,
                streams: n.max_streams,
            });
        }
        let q = &self.queues;
        let needed = q.total_slots().saturating_mul(q.max_item_bytes);
        if needed > q.memory_budget_bytes {
            e.push(ConfigError::QueuesExceedMemory {
                needed,
                budget: q.memory_budget_bytes,
            });
        }
        let bulk = (q.capacities.bulk as u64).saturating_mul(q.max_item_bytes);
        if bulk > q.memory_budget_bytes {
            e.push(ConfigError::BulkQueueExceedsMemory {
                needed: bulk,
                budget: q.memory_budget_bytes,
            });
        }
        if q.memory_budget_bytes > self.memory.global_bytes {
            e.push(ConfigError::QueueBudgetExceedsGlobal {
                budget: q.memory_budget_bytes,
                global: self.memory.global_bytes,
            });
        }
        for (what, value, ceiling) in [
            (
                "network.max_streams",
                u64::from(n.max_streams),
                hard.max_concurrent_streams,
            ),
            (
                "memory.global_bytes",
                self.memory.global_bytes,
                hard.max_memory_buffer_bytes,
            ),
            (
                "storage.relay_quota_bytes",
                self.storage.relay_quota_bytes,
                hard.max_dtn_storage_bytes,
            ),
        ] {
            if value > ceiling {
                e.push(ConfigError::ExceedsHardCeiling {
                    what,
                    value,
                    ceiling,
                });
            }
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
    fn every_profile_validates_against_its_own_hard_limits() {
        for p in ALL {
            let c = ResourceConfig::for_profile(p);
            assert_eq!(c.validate(&HardLimits::for_profile(p)), Ok(()), "{p:?}");
        }
    }

    #[test]
    fn mobile_normal_reproduces_the_spec_memory_split() {
        let c = ResourceConfig::for_profile(ResourceProfile::MobileNormal);
        let s = c.memory.sub_budgets;
        assert_eq!(c.memory.global_bytes, 256 * MIB);
        assert_eq!(s[Subsystem::Files as usize], 96 * MIB);
        assert_eq!(s[Subsystem::Calls as usize], 64 * MIB);
        assert_eq!(s[Subsystem::Messaging as usize], 32 * MIB);
        assert_eq!(s[Subsystem::Dtn as usize], 32 * MIB);
        assert_eq!(s[Subsystem::Extensions as usize], 32 * MIB);
    }

    #[test]
    fn split_always_sums_exactly() {
        for g in [0, 1, 7, 100, 1_000_003, u64::MAX / 2] {
            assert_eq!(split_memory(g).iter().sum::<u64>(), g);
        }
    }

    #[test]
    fn profiles_scale_monotonically_and_server_is_stricter_per_peer() {
        let c: Vec<_> = ALL
            .iter()
            .map(|p| ResourceConfig::for_profile(*p))
            .collect();
        for w in c.windows(2) {
            assert!(w[0].memory.global_bytes <= w[1].memory.global_bytes);
            assert!(w[0].network.max_streams <= w[1].network.max_streams);
            assert!(w[0].storage.relay_quota_bytes <= w[1].storage.relay_quota_bytes);
        }
        // §16: per-peer share of the *global* limit shrinks on the server.
        let share = |c: &ResourceConfig| {
            f64::from(c.network.max_transfers_per_peer) / f64::from(c.network.max_transfers)
        };
        assert!(share(&c[4]) < share(&c[2]));
    }

    #[test]
    fn spec_142_examples_are_rejected_together() {
        let p = ResourceProfile::MobileNormal;
        let mut c = ResourceConfig::for_profile(p);
        c.storage.critical_reserve_bytes = c.storage.quota_bytes + 1;
        c.network.max_streams_per_peer = c.network.max_streams + 1;
        c.queues.capacities.bulk = 1_000_000;
        let errs = c.validate(&HardLimits::for_profile(p)).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| matches!(e, ConfigError::CriticalReserveExceedsQuota { .. })));
        assert!(errs.iter().any(|e| matches!(
            e,
            ConfigError::PerPeerExceedsGlobal {
                what: "streams",
                ..
            }
        )));
        assert!(errs
            .iter()
            .any(|e| matches!(e, ConfigError::BulkQueueExceedsMemory { .. })));
    }

    #[test]
    fn zero_limits_overcommitted_budgets_and_hard_ceiling_are_caught() {
        let p = ResourceProfile::Desktop;
        let hard = HardLimits::for_profile(p);
        let mut c = ResourceConfig::for_profile(p);
        c.network.max_sessions = 0;
        c.memory.sub_budgets[0] = c.memory.global_bytes;
        c.network.max_streams = (hard.max_concurrent_streams + 1) as u32;
        let errs = c.validate(&hard).unwrap_err();
        assert!(errs.contains(&ConfigError::Zero("network.max_sessions")));
        assert!(errs
            .iter()
            .any(|e| matches!(e, ConfigError::SubBudgetsExceedGlobal { .. })));
        assert!(errs.iter().any(|e| matches!(
            e,
            ConfigError::ExceedsHardCeiling {
                what: "network.max_streams",
                ..
            }
        )));
    }

    #[test]
    fn data_saver_quarters_transfers_but_keeps_one() {
        let mut c = ResourceConfig::for_profile(ResourceProfile::Desktop);
        assert_eq!(c.effective_max_transfers(), 8);
        c.data_saver = true;
        assert_eq!(c.effective_max_transfers(), 2);
        c.network.max_transfers = 2;
        assert_eq!(c.effective_max_transfers(), 1);
    }
}
