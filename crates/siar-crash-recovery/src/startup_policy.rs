//! §166 "Lazy Secondary Recovery", §171 "Recovery and Battery
//! Scheduling", §172 "Recovery and Proximity", §175 "Recovery and
//! Emergency Priority".
//!
//! Policy for *when* and *how hard* recovered work restarts. After a
//! crash the device's battery and thermal state are re-read, resumption
//! ramps up instead of starting at full throughput, transient
//! proximity state is discarded while the durable DTN store is kept —
//! and pending emergency work goes first, always, by an invariant the
//! property test below checks over arbitrary queues.

use crate::progressive::RecoveryTier;
use crate::recovery_scheduler::{
    jitter_millis, plan_resume, AdmissionConfig, PendingOp, RecoveryClass, ResumePlan,
};
use crate::timing_recovery::WallMillis;

// ---------------------------------------------------------------------
// §166
// ---------------------------------------------------------------------

/// Search/index/cache recovery may continue after the network is up,
/// but only if it is isolated from everything else (its failure can't
/// affect messaging). Critical tiers never run lazily.
pub const fn may_run_after_network(tier: RecoveryTier, isolated: bool) -> bool {
    !tier.is_critical() && isolated
}

// ---------------------------------------------------------------------
// §171
// ---------------------------------------------------------------------

/// Canonical thermal scale from `siar-power` (`Fair` is now `Elevated`).
pub use siar_power::ThermalState as ThermalLevel;

/// Re-read from the platform after every restart — never restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceConditions {
    pub battery_percent: u8,
    pub charging: bool,
    pub thermal: ThermalLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThrottleLevel {
    Full,
    Reduced,
    Minimal,
    Paused,
}

/// The harsher of the thermal and battery verdicts.
pub const fn throttle_level(c: DeviceConditions) -> ThrottleLevel {
    let thermal = match c.thermal {
        ThermalLevel::Nominal => ThrottleLevel::Full,
        ThermalLevel::Elevated => ThrottleLevel::Reduced,
        ThermalLevel::Serious => ThrottleLevel::Minimal,
        ThermalLevel::Critical => ThrottleLevel::Paused,
        ThermalLevel::Unknown => ThrottleLevel::Full,
    };
    let battery = if c.charging {
        ThrottleLevel::Full
    } else if c.battery_percent < 10 {
        ThrottleLevel::Minimal
    } else if c.battery_percent < 20 {
        ThrottleLevel::Reduced
    } else {
        ThrottleLevel::Full
    };
    if (thermal as u8) >= (battery as u8) {
        thermal
    } else {
        battery
    }
}

/// Scales the admission limits for a throttle level. `Paused` admits
/// nothing.
pub fn adjust_admission(cfg: AdmissionConfig, level: ThrottleLevel) -> AdmissionConfig {
    let scale = |n: usize| match level {
        ThrottleLevel::Full => n,
        ThrottleLevel::Reduced => (n / 2).max(1),
        ThrottleLevel::Minimal => 1.min(n),
        ThrottleLevel::Paused => 0,
    };
    AdmissionConfig {
        batch_size: scale(cfg.batch_size),
        max_in_flight: scale(cfg.max_in_flight),
        ..cfg
    }
}

/// §171: "do not immediately resume everything at maximum
/// throughput." Starts at ⅛ of the configured batch and doubles each
/// completed wave until it reaches the configured size.
pub fn ramp_up_batch(configured_batch: usize, waves_completed: u32) -> usize {
    if configured_batch == 0 {
        return 0;
    }
    let start = (configured_batch / 8).max(1);
    let grown = start.saturating_mul(
        1usize
            .checked_shl(waves_completed.min(63))
            .unwrap_or(usize::MAX),
    );
    grown.min(configured_batch)
}

/// Emergency work is exempt from battery/thermal throttling.
pub const fn exempt_from_throttle(c: RecoveryClass) -> bool {
    matches!(c, RecoveryClass::Critical)
}

/// Like [`plan_resume`], but ordinary work is throttled by the current
/// device conditions while `Critical` work keeps the full limits.
///
/// The whole queue is planned **once**, in priority order, with the
/// unthrottled limits; the throttle then only *moves non-critical
/// starts into the deferred list*. Planning critical and ordinary work
/// as two separate queues looks equivalent but is not: with a batch
/// limit smaller than the number of emergency ops, the ordinary queue
/// gets its own slots and starts while emergency work is still
/// waiting. (A property test found exactly that.) Throttling after a
/// single priority-ordered plan makes "emergency first" hold by
/// construction.
pub fn plan_resume_throttled(
    ops: &[PendingOp],
    in_flight_now: usize,
    now: WallMillis,
    cfg: AdmissionConfig,
    level: ThrottleLevel,
) -> ResumePlan {
    let mut plan = plan_resume(ops, in_flight_now, now, cfg);
    let adj = adjust_admission(cfg, level);

    let critical_started = plan
        .start_now
        .iter()
        .filter(|&&i| exempt_from_throttle(ops[i].class))
        .count();
    let allowed_ordinary = adj
        .max_in_flight
        .saturating_sub(in_flight_now + critical_started)
        .min(adj.batch_size);

    let mut kept_ordinary = 0usize;
    let mut started = Vec::with_capacity(plan.start_now.len());
    for &i in &plan.start_now {
        if exempt_from_throttle(ops[i].class) {
            started.push(i);
        } else if kept_ordinary < allowed_ordinary {
            kept_ordinary += 1;
            started.push(i);
        } else {
            let at = now
                .0
                .saturating_add(cfg.batch_interval_millis)
                .saturating_add(jitter_millis(ops[i].jitter_seed, cfg.jitter_max_millis));
            plan.deferred.push((i, WallMillis(at)));
        }
    }
    plan.start_now = started;
    plan
}

// ---------------------------------------------------------------------
// §172
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProximityRecovery {
    /// Nearby-encounter state is transient: dropped on restart.
    pub encounter_state_discarded: bool,
    /// The durable DTN bundle store survives.
    pub dtn_store_retained: bool,
}

pub const fn proximity_recovery() -> ProximityRecovery {
    ProximityRecovery {
        encounter_state_discarded: true,
        dtn_store_retained: true,
    }
}

/// When proximity returns, forwarding resumes — provided the
/// encounter scheduler has been restarted (§27's last step) and there
/// is something durable to forward.
pub const fn forwarding_resumes_on_encounter(
    durable_bundles_pending: usize,
    encounter_scheduler_resumed: bool,
) -> bool {
    durable_bundles_pending > 0 && encounter_scheduler_resumed
}

// ---------------------------------------------------------------------
// §175
// ---------------------------------------------------------------------

/// §175's explicit ordering invariant over a produced plan:
/// emergency (`Critical`) work is never behind bulk work.
///
/// * in `start_now`, no non-critical op may precede a critical one;
/// * if any critical op was *deferred*, no non-critical op may have
///   been started at all.
pub fn emergency_ordering_holds(ops: &[PendingOp], plan: &ResumePlan) -> bool {
    let critical = |i: usize| ops[i].class == RecoveryClass::Critical;
    let mut seen_non_critical = false;
    for &i in &plan.start_now {
        if critical(i) {
            if seen_non_critical {
                return false;
            }
        } else {
            seen_non_critical = true;
        }
    }
    let critical_deferred = plan.deferred.iter().any(|&(i, _)| critical(i));
    !(critical_deferred && seen_non_critical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing_recovery::RetryState;
    use proptest::prelude::*;

    #[test]
    fn only_isolated_noncritical_work_runs_after_the_network_is_up() {
        for t in RecoveryTier::ALL {
            assert!(!may_run_after_network(t, false), "{t:?} not isolated");
            assert_eq!(may_run_after_network(t, true), !t.is_critical(), "{t:?}");
        }
        assert!(may_run_after_network(RecoveryTier::Search, true));
        assert!(!may_run_after_network(RecoveryTier::Identity, true));
    }

    fn cond(b: u8, ch: bool, th: ThermalLevel) -> DeviceConditions {
        DeviceConditions {
            battery_percent: b,
            charging: ch,
            thermal: th,
        }
    }

    #[test]
    fn throttle_takes_the_harsher_of_thermal_and_battery() {
        use ThermalLevel::*;
        assert_eq!(
            throttle_level(cond(90, false, Nominal)),
            ThrottleLevel::Full
        );
        assert_eq!(
            throttle_level(cond(15, false, Nominal)),
            ThrottleLevel::Reduced
        );
        assert_eq!(
            throttle_level(cond(5, false, Nominal)),
            ThrottleLevel::Minimal
        );
        assert_eq!(
            throttle_level(cond(90, false, Elevated)),
            ThrottleLevel::Reduced
        );
        assert_eq!(
            throttle_level(cond(90, false, Serious)),
            ThrottleLevel::Minimal
        );
        assert_eq!(
            throttle_level(cond(90, false, Critical)),
            ThrottleLevel::Paused
        );
        assert_eq!(
            throttle_level(cond(5, false, Elevated)),
            ThrottleLevel::Minimal
        );
    }

    #[test]
    fn charging_ignores_low_battery_but_not_heat() {
        use ThermalLevel::*;
        assert_eq!(throttle_level(cond(3, true, Nominal)), ThrottleLevel::Full);
        assert_eq!(
            throttle_level(cond(3, true, Critical)),
            ThrottleLevel::Paused
        );
    }

    const CFG: AdmissionConfig = AdmissionConfig {
        batch_size: 40,
        max_in_flight: 80,
        batch_interval_millis: 1_000,
        jitter_max_millis: 100,
    };

    #[test]
    fn admission_scales_with_the_throttle_level() {
        assert_eq!(adjust_admission(CFG, ThrottleLevel::Full), CFG);
        let r = adjust_admission(CFG, ThrottleLevel::Reduced);
        assert_eq!((r.batch_size, r.max_in_flight), (20, 40));
        let m = adjust_admission(CFG, ThrottleLevel::Minimal);
        assert_eq!((m.batch_size, m.max_in_flight), (1, 1));
        let p = adjust_admission(CFG, ThrottleLevel::Paused);
        assert_eq!((p.batch_size, p.max_in_flight), (0, 0));
        assert_eq!(r.batch_interval_millis, CFG.batch_interval_millis);
    }

    #[test]
    fn resumption_ramps_up_instead_of_starting_at_full_throughput() {
        let v: Vec<_> = (0..6).map(|w| ramp_up_batch(64, w)).collect();
        assert_eq!(v, vec![8, 16, 32, 64, 64, 64]);
        assert!(ramp_up_batch(64, 0) < 64, "first wave is not max");
        assert_eq!(ramp_up_batch(3, 0), 1);
        assert_eq!(ramp_up_batch(0, 5), 0);
        assert_eq!(ramp_up_batch(64, u32::MAX), 64);
    }

    fn op(class: RecoveryClass, seed: u64) -> PendingOp {
        PendingOp {
            class,
            retry: RetryState {
                next_attempt_at: WallMillis(0),
                attempt_count: 1,
                expiry: WallMillis(1_000_000),
            },
            jitter_seed: seed,
        }
    }

    #[test]
    fn paused_device_still_resumes_emergency_work_but_nothing_else() {
        let ops = [
            op(RecoveryClass::Bulk, 1),
            op(RecoveryClass::Critical, 2),
            op(RecoveryClass::Normal, 3),
        ];
        let plan = plan_resume_throttled(&ops, 0, WallMillis(10), CFG, ThrottleLevel::Paused);
        assert_eq!(plan.start_now, vec![1], "only the critical op");
        let deferred: Vec<_> = plan.deferred.iter().map(|d| d.0).collect();
        assert!(deferred.contains(&0) && deferred.contains(&2));
        assert!(emergency_ordering_holds(&ops, &plan));
    }

    #[test]
    fn full_throttle_matches_the_plain_planner_for_non_critical_work() {
        let ops: Vec<_> = (0..10).map(|i| op(RecoveryClass::Normal, i)).collect();
        let a = plan_resume_throttled(&ops, 0, WallMillis(10), CFG, ThrottleLevel::Full);
        let b = plan_resume(&ops, 0, WallMillis(10), CFG);
        assert_eq!(a.start_now, b.start_now);
        assert_eq!(a.deferred, b.deferred);
    }

    #[test]
    fn minimal_throttle_trickles_ordinary_work() {
        let ops: Vec<_> = (0..10).map(|i| op(RecoveryClass::Normal, i)).collect();
        let plan = plan_resume_throttled(&ops, 0, WallMillis(10), CFG, ThrottleLevel::Minimal);
        assert_eq!(plan.start_now.len(), 1);
        assert_eq!(plan.deferred.len(), 9);
    }

    #[test]
    fn regression_ordinary_work_never_starts_while_emergency_work_waits() {
        // Shrunk counterexample found by the property test: 4 emergency
        // ops + 1 interactive op, batch limit 1. Planning the two
        // groups as separate queues started the interactive op while
        // three emergency ops were still deferred.
        let mut ops: Vec<_> = (0..4).map(|i| op(RecoveryClass::Critical, i)).collect();
        ops.push(op(RecoveryClass::Interactive, 99));
        let cfg = AdmissionConfig {
            batch_size: 1,
            max_in_flight: 5,
            ..CFG
        };
        let plan = plan_resume_throttled(&ops, 0, WallMillis(10), cfg, ThrottleLevel::Full);
        assert_eq!(
            plan.start_now,
            vec![0],
            "one emergency op, per the batch limit"
        );
        assert!(
            !plan.start_now.contains(&4),
            "interactive must wait behind emergency work"
        );
        assert!(emergency_ordering_holds(&ops, &plan));
    }

    #[test]
    fn proximity_state_is_discarded_but_the_dtn_store_survives() {
        let p = proximity_recovery();
        assert!(p.encounter_state_discarded && p.dtn_store_retained);
    }

    #[test]
    fn forwarding_resumes_when_a_peer_returns_if_there_is_work_and_a_scheduler() {
        assert!(forwarding_resumes_on_encounter(3, true));
        assert!(
            !forwarding_resumes_on_encounter(0, true),
            "nothing to forward"
        );
        assert!(
            !forwarding_resumes_on_encounter(3, false),
            "scheduler not resumed yet"
        );
    }

    #[test]
    fn emergency_check_detects_a_violation() {
        let ops = [op(RecoveryClass::Bulk, 1), op(RecoveryClass::Critical, 2)];
        let bad = ResumePlan {
            start_now: vec![0, 1],
            ..ResumePlan::default()
        };
        assert!(!emergency_ordering_holds(&ops, &bad));
        let bad2 = ResumePlan {
            start_now: vec![0],
            deferred: vec![(1, WallMillis(5))],
            ..ResumePlan::default()
        };
        assert!(!emergency_ordering_holds(&ops, &bad2));
        let good = ResumePlan {
            start_now: vec![1, 0],
            ..ResumePlan::default()
        };
        assert!(emergency_ordering_holds(&ops, &good));
    }

    fn class_of(n: u8) -> RecoveryClass {
        match n {
            0 => RecoveryClass::Critical,
            1 => RecoveryClass::Interactive,
            2 => RecoveryClass::Normal,
            3 => RecoveryClass::Bulk,
            _ => RecoveryClass::Background,
        }
    }

    proptest! {
        // §175: for ANY queue, throttle level, limits and in-flight
        // count, pending emergency work is never behind bulk work.
        #[test]
        fn emergency_work_always_goes_first(
            classes in prop::collection::vec(0u8..5, 0..60),
            level in 0u8..4,
            batch in 0usize..20,
            max_in_flight in 0usize..30,
            in_flight in 0usize..30,
        ) {
            let ops: Vec<PendingOp> = classes
                .iter()
                .enumerate()
                .map(|(i, c)| op(class_of(*c), i as u64))
                .collect();
            let level = match level {
                0 => ThrottleLevel::Full,
                1 => ThrottleLevel::Reduced,
                2 => ThrottleLevel::Minimal,
                _ => ThrottleLevel::Paused,
            };
            let cfg = AdmissionConfig { batch_size: batch, max_in_flight, ..CFG };
            let plan = plan_resume_throttled(&ops, in_flight, WallMillis(10), cfg, level);
            prop_assert!(emergency_ordering_holds(&ops, &plan));
            // and the plain planner satisfies it too
            let plain = plan_resume(&ops, in_flight, WallMillis(10), cfg);
            prop_assert!(emergency_ordering_holds(&ops, &plain));
        }
    }
}
