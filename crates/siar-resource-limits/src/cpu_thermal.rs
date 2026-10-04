//! §86 CPU pressure, §87 CPU pressure response, §88 thermal pressure,
//! §89 battery/energy budget.
//!
//! CPU pressure is derived from cheap, local signals and reduced to the
//! same coarse four-level [`PressureState`] everything else uses. The
//! responses are plain data describing what to throttle; the callers
//! (hash pool, AV1 encoder, indexer) apply them. Critical communication
//! is never one of the things thermal or CPU pressure can switch off.

use crate::effective_limits::pressure_limit_for;
use crate::storage::PressureState;
use crate::token_bucket::TokenBucket;

// --------------------------------------------------------------- §86

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuSignals {
    pub queue_depth: u64,
    pub queue_capacity: u64,
    pub task_latency_ms: u64,
    pub latency_target_ms: u64,
    pub encoder_overloaded: bool,
    pub thermal: PressureState,
}

fn permille(num: u64, den: u64) -> Option<u64> {
    (den != 0).then(|| (u128::from(num) * 1000 / u128::from(den)).min(u128::from(u64::MAX)) as u64)
}

pub fn cpu_pressure(s: &CpuSignals) -> PressureState {
    // Queue fullness: 50% / 75% / 100% of capacity.
    let queue = match permille(s.queue_depth, s.queue_capacity) {
        None if s.queue_depth > 0 => PressureState::Exhausted, // work but no capacity
        None => PressureState::Normal,
        Some(p) if p < 500 => PressureState::Normal,
        Some(p) if p < 750 => PressureState::Elevated,
        Some(p) if p < 1000 => PressureState::Critical,
        Some(_) => PressureState::Exhausted,
    };
    // Latency vs target: 1.5x / 3x / 6x.
    let latency = match permille(s.task_latency_ms, s.latency_target_ms) {
        None if s.task_latency_ms > 0 => PressureState::Exhausted,
        None => PressureState::Normal,
        Some(p) if p < 1_500 => PressureState::Normal,
        Some(p) if p < 3_000 => PressureState::Elevated,
        Some(p) if p < 6_000 => PressureState::Critical,
        Some(_) => PressureState::Exhausted,
    };
    let encoder = if s.encoder_overloaded {
        PressureState::Elevated
    } else {
        PressureState::Normal
    };
    queue.max(latency).max(encoder).max(s.thermal)
}

// --------------------------------------------------------------- §87

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Av1Complexity {
    Minimal,
    Reduced,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuBase {
    pub parallel_hashes: u64,
    pub indexing_concurrency: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuResponse {
    pub parallel_hashes: u64,
    pub av1_complexity: Av1Complexity,
    pub pause_background_compression: bool,
    pub indexing_concurrency: u64,
}

pub fn cpu_response(base: CpuBase, state: PressureState) -> CpuResponse {
    CpuResponse {
        parallel_hashes: pressure_limit_for(base.parallel_hashes, state),
        av1_complexity: match state {
            PressureState::Normal => Av1Complexity::Full,
            PressureState::Elevated => Av1Complexity::Reduced,
            PressureState::Critical | PressureState::Exhausted => Av1Complexity::Minimal,
        },
        pause_background_compression: state >= PressureState::Critical,
        indexing_concurrency: pressure_limit_for(base.indexing_concurrency, state),
    }
}

// --------------------------------------------------------------- §88

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaQuality {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalResponse {
    pub max_media_quality: MediaQuality,
    pub allow_wifi_heavy_transfer: bool,
    /// Percent of normal CPU-job concurrency.
    pub cpu_job_percent: u64,
}

impl ThermalResponse {
    /// There is deliberately no field that can disable it: messaging,
    /// SOS and call signaling stay available at every thermal level.
    pub const fn critical_communication_available(&self) -> bool {
        true
    }
}

pub fn thermal_response(thermal: PressureState) -> ThermalResponse {
    match thermal {
        PressureState::Normal => ThermalResponse {
            max_media_quality: MediaQuality::High,
            allow_wifi_heavy_transfer: true,
            cpu_job_percent: 100,
        },
        PressureState::Elevated => ThermalResponse {
            max_media_quality: MediaQuality::Medium,
            allow_wifi_heavy_transfer: true,
            cpu_job_percent: 60,
        },
        PressureState::Critical => ThermalResponse {
            max_media_quality: MediaQuality::Low,
            allow_wifi_heavy_transfer: false,
            cpu_job_percent: 30,
        },
        PressureState::Exhausted => ThermalResponse {
            max_media_quality: MediaQuality::Low,
            allow_wifi_heavy_transfer: false,
            cpu_job_percent: 10,
        },
    }
}

// --------------------------------------------------------------- §89

/// (burst, units per second) for each of the three budgets the spec names.
/// Callers price work in units (a radio activation is a lump of units).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyRates {
    pub background_work: (u64, u64),
    pub radio_activation: (u64, u64),
    pub cpu_heavy: (u64, u64),
}

/// Re-rate a bucket **without** refilling it. A fresh bucket starts full,
/// so naively rebuilding on "tighten to battery saver" would hand out a
/// free burst at exactly the wrong moment; instead the new bucket keeps
/// the smaller of what was left and its new capacity.
pub fn retune_bucket(
    old: &mut TokenBucket,
    burst: u64,
    per_sec: u64,
    now_millis: u64,
) -> TokenBucket {
    let left = old.available(now_millis).min(burst);
    let mut fresh = TokenBucket::new(burst, per_sec, now_millis);
    let spent = burst - left;
    if spent > 0 {
        let took = fresh.try_consume(spent, now_millis);
        debug_assert!(took);
    }
    fresh
}

#[derive(Debug)]
pub struct EnergyBudgets {
    base: EnergyRates,
    background: TokenBucket,
    radio: TokenBucket,
    cpu_heavy: TokenBucket,
}

fn scaled(r: (u64, u64), pct: u64) -> (u64, u64) {
    let s = |v: u64| (u128::from(v) * u128::from(pct) / 100) as u64;
    (s(r.0), s(r.1))
}

impl EnergyBudgets {
    pub fn new(rates: EnergyRates, now_millis: u64) -> Self {
        Self {
            base: rates,
            background: TokenBucket::new(
                rates.background_work.0,
                rates.background_work.1,
                now_millis,
            ),
            radio: TokenBucket::new(
                rates.radio_activation.0,
                rates.radio_activation.1,
                now_millis,
            ),
            cpu_heavy: TokenBucket::new(rates.cpu_heavy.0, rates.cpu_heavy.1, now_millis),
        }
    }

    pub fn try_background_work(&mut self, units: u64, now: u64) -> bool {
        self.background.try_consume(units, now)
    }

    pub fn try_radio_activation(&mut self, units: u64, now: u64) -> bool {
        self.radio.try_consume(units, now)
    }

    pub fn try_cpu_heavy(&mut self, units: u64, now: u64) -> bool {
        self.cpu_heavy.try_consume(units, now)
    }

    /// Battery saver (Part 13) tightens by percent of the base rates.
    pub fn set_scale_percent(&mut self, pct: u64, now: u64) {
        let (b, r, c) = (
            scaled(self.base.background_work, pct),
            scaled(self.base.radio_activation, pct),
            scaled(self.base.cpu_heavy, pct),
        );
        self.background = retune_bucket(&mut self.background, b.0, b.1, now);
        self.radio = retune_bucket(&mut self.radio, r.0, r.1, now);
        self.cpu_heavy = retune_bucket(&mut self.cpu_heavy, c.0, c.1, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PressureState::*;

    #[test]
    fn each_signal_can_raise_pressure_and_the_worst_wins() {
        let calm = CpuSignals {
            queue_capacity: 100,
            latency_target_ms: 100,
            ..Default::default()
        };
        assert_eq!(cpu_pressure(&calm), Normal);
        assert_eq!(
            cpu_pressure(&CpuSignals {
                queue_depth: 60,
                ..calm
            }),
            Elevated
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                queue_depth: 90,
                ..calm
            }),
            Critical
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                queue_depth: 100,
                ..calm
            }),
            Exhausted
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                task_latency_ms: 200,
                ..calm
            }),
            Elevated
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                task_latency_ms: 700,
                ..calm
            }),
            Exhausted
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                encoder_overloaded: true,
                ..calm
            }),
            Elevated
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                thermal: Critical,
                queue_depth: 60,
                ..calm
            }),
            Critical
        );
    }

    #[test]
    fn zero_capacity_or_target_is_handled_not_divided_by() {
        assert_eq!(cpu_pressure(&CpuSignals::default()), Normal);
        assert_eq!(
            cpu_pressure(&CpuSignals {
                queue_depth: 1,
                ..Default::default()
            }),
            Exhausted
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                task_latency_ms: 1,
                ..Default::default()
            }),
            Exhausted
        );
        assert_eq!(
            cpu_pressure(&CpuSignals {
                queue_depth: u64::MAX,
                queue_capacity: 1,
                ..Default::default()
            }),
            Exhausted
        );
    }

    #[test]
    fn response_follows_the_four_spec_actions() {
        let base = CpuBase {
            parallel_hashes: 8,
            indexing_concurrency: 4,
        };
        let n = cpu_response(base, Normal);
        assert_eq!(
            (
                n.parallel_hashes,
                n.av1_complexity,
                n.pause_background_compression
            ),
            (8, Av1Complexity::Full, false)
        );
        let e = cpu_response(base, Elevated);
        assert_eq!(
            (e.parallel_hashes, e.av1_complexity, e.indexing_concurrency),
            (4, Av1Complexity::Reduced, 2)
        );
        let c = cpu_response(base, Critical);
        assert!(c.pause_background_compression && c.av1_complexity == Av1Complexity::Minimal);
        let x = cpu_response(
            CpuBase {
                parallel_hashes: 1,
                indexing_concurrency: 1,
            },
            Exhausted,
        );
        assert_eq!(
            (x.parallel_hashes, x.indexing_concurrency),
            (1, 1),
            "work still makes progress"
        );
    }

    #[test]
    fn thermal_response_degrades_monotonically_and_never_disables_critical_comms() {
        let levels = [Normal, Elevated, Critical, Exhausted];
        let rs: Vec<_> = levels.iter().map(|l| thermal_response(*l)).collect();
        for w in rs.windows(2) {
            assert!(w[0].max_media_quality >= w[1].max_media_quality);
            assert!(w[0].cpu_job_percent >= w[1].cpu_job_percent);
            assert!(w[0].allow_wifi_heavy_transfer >= w[1].allow_wifi_heavy_transfer);
        }
        assert!(rs
            .iter()
            .all(ThermalResponse::critical_communication_available));
        assert!(!rs[2].allow_wifi_heavy_transfer);
        assert!(
            rs.iter().all(|r| r.cpu_job_percent > 0),
            "CPU jobs are slowed, not stopped"
        );
    }

    fn rates() -> EnergyRates {
        EnergyRates {
            background_work: (100, 10),
            radio_activation: (40, 4),
            cpu_heavy: (200, 20),
        }
    }

    #[test]
    fn three_independent_budgets() {
        let mut e = EnergyBudgets::new(rates(), 0);
        assert!(e.try_background_work(100, 0));
        assert!(!e.try_background_work(1, 0));
        assert!(
            e.try_radio_activation(40, 0),
            "radio budget unaffected by background work"
        );
        assert!(e.try_cpu_heavy(200, 0));
        assert!(e.try_background_work(10, 1_000), "refills at 10/s");
    }

    #[test]
    fn tightening_does_not_hand_out_a_free_burst() {
        let mut e = EnergyBudgets::new(rates(), 0);
        assert!(e.try_background_work(95, 0)); // 5 left of 100
        e.set_scale_percent(50, 0); // burst 50, rate 5
        assert!(e.try_background_work(5, 0));
        assert!(
            !e.try_background_work(1, 0),
            "kept what was left; did not refill to 50"
        );
        assert!(e.try_background_work(5, 1_000), "new, lower rate: 5/s");
        assert!(!e.try_background_work(1, 1_000));
    }

    #[test]
    fn retune_clamps_remaining_tokens_to_the_new_capacity() {
        let mut b = TokenBucket::new(100, 10, 0);
        let mut small = retune_bucket(&mut b, 20, 2, 0);
        assert_eq!(small.available(0), 20);
        let mut empty = TokenBucket::new(100, 10, 0);
        assert!(empty.try_consume(100, 0));
        let mut still_empty = retune_bucket(&mut empty, 20, 2, 0);
        assert_eq!(still_empty.available(0), 0);
    }

    #[test]
    fn scale_zero_stops_budgeted_work_and_scale_100_restores_rates() {
        let mut e = EnergyBudgets::new(rates(), 0);
        e.set_scale_percent(0, 0);
        assert!(!e.try_cpu_heavy(1, 5_000));
        e.set_scale_percent(100, 5_000);
        assert!(e.try_cpu_heavy(20, 6_000), "back to 20/s");
    }
}
