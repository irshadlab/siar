//! §41-43: how many chunks may be in flight at once.
//!
//! * §41 — parallelism is a *configurable limit* that rises only when
//!   resources justify it. [`effective_parallelism`] starts from what the
//!   path can carry and takes away for each constraint that is present
//!   (battery saver, memory pressure, metered link, busy CPU).
//! * §42 — per path: LAN/iroh high, Bluetooth one, DTN is bundle-level
//!   scheduling (one stream here; bundles are DTN's own unit).
//! * §43 — [`AdaptiveConcurrency`]: "start with deterministic limits",
//!   then additive-increase / multiplicative-decrease *inside* those
//!   limits, so adaptation can only ever lower what the deterministic
//!   rules allow or climb back up to it, never exceed it.

use crate::scheduling::PathKind;

/// §42's table. Chosen conservatively; a transport that knows better can
/// pass its own ceiling to [`effective_parallelism`] via `negotiated`.
pub const fn path_ceiling(path: PathKind) -> u16 {
    match path {
        PathKind::Lan => 8,
        PathKind::IrohRelay => 4,
        PathKind::WifiDirect | PathKind::WifiAware => 4,
        PathKind::Ble => 1,
        // Bundle-level scheduling: one bundle stream at a time.
        PathKind::Dtn => 1,
    }
}

/// What the device is under right now. The host fills this in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceContext {
    pub battery_saver: bool,
    pub memory_pressure: bool,
    pub metered: bool,
    pub cpu_constrained: bool,
}

/// The number of chunks to keep in flight: never below 1, never above
/// the path's ceiling or what the peers negotiated, and reduced — not
/// merely capped — by each active constraint.
pub fn effective_parallelism(path: PathKind, ctx: ResourceContext, negotiated: u16) -> u16 {
    let mut n = path_ceiling(path).min(negotiated.max(1));
    if ctx.battery_saver || ctx.memory_pressure {
        n = n.div_ceil(4); // 8 → 2, 4 → 1
    } else if ctx.metered || ctx.cpu_constrained {
        n = n.div_ceil(2); // 8 → 4, 4 → 2
    }
    n.max(1)
}

/// One observation window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub throughput_bytes_per_sec: u64,
    /// Lost or retransmitted chunks, per mille of those sent.
    pub loss_permille: u16,
    pub rtt_millis: u32,
    pub memory_pressure: bool,
    pub battery_saver: bool,
}

/// AIMD over concurrency, bounded by a ceiling the deterministic rules
/// computed. Pure state: feed it [`Sample`]s, read [`Self::limit`].
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveConcurrency {
    ceiling: u16,
    limit: u16,
    last_throughput: u64,
    last_rtt: u32,
    last_loss: u16,
}

/// A change in throughput smaller than this (per mille) is noise.
const IMPROVEMENT_PERMILLE: u64 = 50;
/// RTT growth beyond this factor (per mille over 1000) counts as queuing.
const RTT_GROWTH_PERMILLE: u64 = 250;
const LOSS_RISE_PERMILLE: u16 = 20;

impl AdaptiveConcurrency {
    /// Starts at 1 (cautious) and climbs while it pays.
    pub fn new(ceiling: u16) -> Self {
        Self {
            ceiling: ceiling.max(1),
            limit: 1,
            last_throughput: 0,
            last_rtt: 0,
            last_loss: 0,
        }
    }

    pub fn limit(&self) -> u16 {
        self.limit
    }

    pub fn ceiling(&self) -> u16 {
        self.ceiling
    }

    /// The deterministic ceiling changed (path switched, constraint
    /// appeared): re-clamp immediately.
    pub fn set_ceiling(&mut self, ceiling: u16) {
        self.ceiling = ceiling.max(1);
        self.limit = self.limit.min(self.ceiling);
    }

    pub fn on_sample(&mut self, s: Sample) -> u16 {
        // Pressure and battery saver override any measurement.
        if s.memory_pressure || s.battery_saver {
            self.limit = 1;
            self.remember(s);
            return self.limit;
        }
        let loss_rose = s.loss_permille >= self.last_loss.saturating_add(LOSS_RISE_PERMILLE);
        let latency_rose = self.last_rtt > 0
            && u64::from(s.rtt_millis) * 1000
                > u64::from(self.last_rtt) * (1000 + RTT_GROWTH_PERMILLE);
        if loss_rose || latency_rose {
            // Multiplicative decrease.
            self.limit = (self.limit / 2).max(1);
        } else if self.improved(s.throughput_bytes_per_sec) {
            // Additive increase, only while more concurrency is paying.
            self.limit = (self.limit + 1).min(self.ceiling);
        }
        self.remember(s);
        self.limit
    }

    fn improved(&self, throughput: u64) -> bool {
        if self.last_throughput == 0 {
            return throughput > 0;
        }
        u128::from(throughput) * 1000
            >= u128::from(self.last_throughput) * u128::from(1000 + IMPROVEMENT_PERMILLE)
    }

    fn remember(&mut self, s: Sample) {
        self.last_throughput = s.throughput_bytes_per_sec;
        self.last_rtt = s.rtt_millis;
        self.last_loss = s.loss_permille;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calm() -> Sample {
        Sample {
            throughput_bytes_per_sec: 1_000_000,
            loss_permille: 0,
            rtt_millis: 20,
            memory_pressure: false,
            battery_saver: false,
        }
    }

    #[test]
    fn the_per_path_table_follows_the_spec() {
        assert!(path_ceiling(PathKind::Lan) > path_ceiling(PathKind::IrohRelay));
        assert_eq!(path_ceiling(PathKind::Ble), 1);
        assert_eq!(path_ceiling(PathKind::Dtn), 1);
    }

    #[test]
    fn constraints_reduce_parallelism_and_it_never_reaches_zero() {
        let none = ResourceContext::default();
        assert_eq!(effective_parallelism(PathKind::Lan, none, 16), 8);
        let metered = ResourceContext {
            metered: true,
            ..none
        };
        assert_eq!(effective_parallelism(PathKind::Lan, metered, 16), 4);
        let saver = ResourceContext {
            battery_saver: true,
            ..none
        };
        assert_eq!(effective_parallelism(PathKind::Lan, saver, 16), 2);
        let all = ResourceContext {
            battery_saver: true,
            memory_pressure: true,
            metered: true,
            cpu_constrained: true,
        };
        for p in [
            PathKind::Lan,
            PathKind::IrohRelay,
            PathKind::WifiDirect,
            PathKind::WifiAware,
            PathKind::Ble,
            PathKind::Dtn,
        ] {
            assert!(effective_parallelism(p, all, 0) >= 1);
        }
    }

    #[test]
    fn the_negotiated_value_is_a_hard_cap() {
        assert_eq!(
            effective_parallelism(PathKind::Lan, ResourceContext::default(), 2),
            2
        );
        assert_eq!(
            effective_parallelism(PathKind::Lan, ResourceContext::default(), 0),
            1,
            "a zero advertisement is treated as one, not as zero"
        );
    }

    #[test]
    fn concurrency_climbs_while_throughput_improves_up_to_the_ceiling() {
        let mut a = AdaptiveConcurrency::new(4);
        let mut tp = 1_000_000;
        let mut seen = vec![];
        for _ in 0..8 {
            tp = tp * 13 / 10; // +30% each window
            seen.push(a.on_sample(Sample {
                throughput_bytes_per_sec: tp,
                ..calm()
            }));
        }
        assert_eq!(
            seen[..4],
            [2, 3, 4, 4],
            "first window already shows throughput"
        );
        assert!(seen.iter().all(|&n| n <= 4), "never above the ceiling");
        assert_eq!(a.limit(), 4);
    }

    #[test]
    fn it_stops_climbing_when_more_concurrency_no_longer_helps() {
        let mut a = AdaptiveConcurrency::new(8);
        a.on_sample(calm());
        let n = a.on_sample(Sample {
            throughput_bytes_per_sec: 2_000_000,
            ..calm()
        });
        let after_plateau = a.on_sample(Sample {
            throughput_bytes_per_sec: 2_010_000, // +0.5%: noise
            ..calm()
        });
        assert_eq!(after_plateau, n);
    }

    #[test]
    fn loss_and_latency_rises_halve_the_limit() {
        let mut a = AdaptiveConcurrency::new(8);
        for tp in [1, 2, 4, 8, 16, 32] {
            a.on_sample(Sample {
                throughput_bytes_per_sec: tp * 1_000_000,
                ..calm()
            });
        }
        let before = a.limit();
        assert!(before >= 4);
        let after_loss = a.on_sample(Sample {
            loss_permille: 60,
            throughput_bytes_per_sec: 64_000_000,
            ..calm()
        });
        assert_eq!(after_loss, before / 2);
        let before = a.limit();
        let after_lag = a.on_sample(Sample {
            loss_permille: 60,
            rtt_millis: 200,
            throughput_bytes_per_sec: 64_000_000,
            ..calm()
        });
        assert!(after_lag <= before);
        assert!(after_lag >= 1);
    }

    #[test]
    fn memory_pressure_and_battery_saver_drop_to_one_at_once() {
        let mut a = AdaptiveConcurrency::new(8);
        for tp in [1, 2, 4, 8, 16] {
            a.on_sample(Sample {
                throughput_bytes_per_sec: tp * 1_000_000,
                ..calm()
            });
        }
        assert!(a.limit() > 1);
        a.on_sample(Sample {
            memory_pressure: true,
            ..calm()
        });
        assert_eq!(a.limit(), 1);
        let mut b = AdaptiveConcurrency::new(8);
        b.on_sample(calm());
        b.on_sample(Sample {
            battery_saver: true,
            ..calm()
        });
        assert_eq!(b.limit(), 1);
    }

    #[test]
    fn lowering_the_ceiling_clamps_immediately_and_never_below_one() {
        let mut a = AdaptiveConcurrency::new(8);
        for tp in [1, 2, 4, 8, 16, 32] {
            a.on_sample(Sample {
                throughput_bytes_per_sec: tp * 1_000_000,
                ..calm()
            });
        }
        a.set_ceiling(2);
        assert!(a.limit() <= 2);
        a.set_ceiling(0);
        assert_eq!((a.limit(), a.ceiling()), (1, 1));
    }

    #[test]
    fn the_limit_is_always_within_one_and_the_ceiling_for_any_sample_sequence() {
        let mut a = AdaptiveConcurrency::new(6);
        let mut x = 0x9E37_79B9u32;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            a.on_sample(Sample {
                throughput_bytes_per_sec: u64::from(x),
                loss_permille: (x % 200) as u16,
                rtt_millis: x % 500,
                memory_pressure: x.is_multiple_of(97),
                battery_saver: x.is_multiple_of(89),
            });
            assert!((1..=6).contains(&a.limit()));
        }
    }
}
