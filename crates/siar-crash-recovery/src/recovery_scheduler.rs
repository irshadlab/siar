//! §94 "Thundering Herd Prevention", §95 "Recovery Scheduler", §96
//! "SOS Recovery".
//!
//! On restart with 10 000 pending operations nothing may resume all at
//! once. [`plan_resume`] admits a bounded batch in class-priority
//! order and gives every deferred operation a start time spread by
//! batch and deterministic per-operation jitter. Deterministic jitter
//! (a hash of the operation's seed, not an RNG) keeps plans
//! reproducible in tests and across repeated planning.

use crate::timing_recovery::{eligibility, Eligibility, RetryState, WallMillis};
use serde::{Deserialize, Serialize};

/// §95's five classes, highest first (declaration order = priority).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RecoveryClass {
    Critical,
    Interactive,
    Normal,
    Bulk,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingOp {
    pub class: RecoveryClass,
    pub retry: RetryState,
    /// Stable per-operation value (e.g. a hash of its id) for jitter.
    pub jitter_seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionConfig {
    /// Most operations started in one planning pass.
    pub batch_size: usize,
    /// Resource admission: total operations allowed in flight.
    pub max_in_flight: usize,
    /// Gap between successive deferred batches.
    pub batch_interval_millis: u64,
    /// Upper bound of per-operation jitter added to a deferred start.
    pub jitter_max_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResumePlan {
    /// Indexes to start now, in priority order.
    pub start_now: Vec<usize>,
    /// `(index, start_at)` for eligible work that must wait its turn.
    pub deferred: Vec<(usize, WallMillis)>,
    /// Indexes whose expiry has passed.
    pub expired: Vec<usize>,
    /// Indexes not yet eligible (`next_attempt_at` in the future); they
    /// keep their persisted time.
    pub not_yet_due: Vec<usize>,
}

/// SplitMix64 finaliser — a well-mixed deterministic hash.
const fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic jitter in `[0, max]`.
pub const fn jitter_millis(seed: u64, max: u64) -> u64 {
    if max == u64::MAX {
        mix(seed)
    } else {
        mix(seed) % (max + 1)
    }
}

pub fn plan_resume(
    ops: &[PendingOp],
    in_flight_now: usize,
    now: WallMillis,
    cfg: AdmissionConfig,
) -> ResumePlan {
    let mut plan = ResumePlan::default();
    let mut ready: Vec<usize> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match eligibility(&op.retry, now) {
            Eligibility::Expired => plan.expired.push(i),
            Eligibility::WaitUntil(_) => plan.not_yet_due.push(i),
            Eligibility::EligibleNow => ready.push(i),
        }
    }
    // Priority: class, then oldest due time, then index (stable).
    ready.sort_by_key(|&i| (ops[i].class, ops[i].retry.next_attempt_at, i));

    let slots = cfg
        .max_in_flight
        .saturating_sub(in_flight_now)
        .min(cfg.batch_size);
    let batch = cfg.batch_size.max(1);
    for (rank, &i) in ready.iter().enumerate() {
        if rank < slots {
            plan.start_now.push(i);
        } else {
            let wave = ((rank - slots) / batch) as u64 + 1;
            let start = now
                .0
                .saturating_add(wave.saturating_mul(cfg.batch_interval_millis))
                .saturating_add(jitter_millis(ops[i].jitter_seed, cfg.jitter_max_millis));
            plan.deferred.push((i, WallMillis(start)));
        }
    }
    plan
}

// ---------------------------------------------------------------------
// §96
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SosState {
    Pending,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyChecks {
    pub identity_ok: bool,
    pub storage_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SosDecision {
    /// Resume at the highest priority.
    ResumeHighestPriority,
    /// Pending SOS exists, but identity/storage safety checks haven't
    /// passed: it goes first the moment they do.
    WaitForSafetyChecks,
    /// Expired or cancelled: nothing to resume.
    Skip,
}

pub const fn sos_decision(sos: SosState, checks: SafetyChecks) -> SosDecision {
    match sos {
        SosState::Expired | SosState::Cancelled => SosDecision::Skip,
        SosState::Pending => {
            if checks.identity_ok && checks.storage_ok {
                SosDecision::ResumeHighestPriority
            } else {
                SosDecision::WaitForSafetyChecks
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: AdmissionConfig = AdmissionConfig {
        batch_size: 50,
        max_in_flight: 100,
        batch_interval_millis: 1_000,
        jitter_max_millis: 500,
    };

    fn op(class: RecoveryClass, next: u64, seed: u64) -> PendingOp {
        PendingOp {
            class,
            retry: RetryState {
                next_attempt_at: WallMillis(next),
                attempt_count: 1,
                expiry: WallMillis(1_000_000),
            },
            jitter_seed: seed,
        }
    }

    #[test]
    fn class_order_is_the_spec_order() {
        use RecoveryClass::*;
        assert!(
            Critical < Interactive && Interactive < Normal && Normal < Bulk && Bulk < Background
        );
    }

    #[test]
    fn ten_thousand_pending_do_not_all_start_at_once() {
        let ops: Vec<_> = (0..10_000u64)
            .map(|i| op(RecoveryClass::Normal, 0, i))
            .collect();
        let plan = plan_resume(&ops, 0, WallMillis(10), CFG);
        assert_eq!(plan.start_now.len(), 50);
        assert_eq!(plan.deferred.len(), 9_950);
        assert!(plan.expired.is_empty());
    }

    #[test]
    fn higher_classes_start_first() {
        use RecoveryClass::*;
        let ops = [
            op(Background, 0, 1),
            op(Normal, 0, 2),
            op(Critical, 0, 3),
            op(Interactive, 0, 4),
        ];
        let cfg = AdmissionConfig {
            batch_size: 2,
            ..CFG
        };
        let plan = plan_resume(&ops, 0, WallMillis(5), cfg);
        assert_eq!(plan.start_now, vec![2, 3]);
        let deferred: Vec<_> = plan.deferred.iter().map(|d| d.0).collect();
        assert_eq!(deferred, vec![1, 0]);
    }

    #[test]
    fn in_flight_work_consumes_admission_slots() {
        let ops: Vec<_> = (0..10).map(|i| op(RecoveryClass::Normal, 0, i)).collect();
        let cfg = AdmissionConfig {
            batch_size: 5,
            max_in_flight: 8,
            ..CFG
        };
        assert_eq!(plan_resume(&ops, 6, WallMillis(1), cfg).start_now.len(), 2);
        assert_eq!(plan_resume(&ops, 8, WallMillis(1), cfg).start_now.len(), 0);
        assert_eq!(plan_resume(&ops, 99, WallMillis(1), cfg).start_now.len(), 0);
    }

    #[test]
    fn deferred_starts_are_spread_across_waves_with_bounded_jitter() {
        let ops: Vec<_> = (0..120u64)
            .map(|i| op(RecoveryClass::Normal, 0, i))
            .collect();
        let plan = plan_resume(&ops, 0, WallMillis(1_000), CFG);
        assert_eq!(plan.start_now.len(), 50);
        for (rank, (_, at)) in plan.deferred.iter().enumerate() {
            let wave = (rank / 50) as u64 + 1;
            let lo = 1_000 + wave * 1_000;
            assert!(at.0 >= lo && at.0 <= lo + 500, "rank {rank}: {at:?}");
        }
        let distinct: std::collections::HashSet<_> = plan.deferred.iter().map(|d| d.1).collect();
        assert!(distinct.len() > 20, "jitter should de-synchronise starts");
    }

    #[test]
    fn planning_is_deterministic() {
        let ops: Vec<_> = (0..200u64)
            .map(|i| op(RecoveryClass::Bulk, i % 7, i))
            .collect();
        assert_eq!(
            plan_resume(&ops, 0, WallMillis(50), CFG),
            plan_resume(&ops, 0, WallMillis(50), CFG)
        );
    }

    #[test]
    fn expired_and_not_yet_due_are_set_aside() {
        let mut expired = op(RecoveryClass::Normal, 0, 1);
        expired.retry.expiry = WallMillis(5);
        let future = op(RecoveryClass::Normal, 9_000, 2);
        let due = op(RecoveryClass::Normal, 0, 3);
        let plan = plan_resume(&[expired, future, due], 0, WallMillis(100), CFG);
        assert_eq!(plan.expired, vec![0]);
        assert_eq!(plan.not_yet_due, vec![1]);
        assert_eq!(plan.start_now, vec![2]);
    }

    #[test]
    fn jitter_stays_within_bounds_including_extremes() {
        for s in 0..1_000 {
            assert!(jitter_millis(s, 99) <= 99);
        }
        assert_eq!(jitter_millis(7, 0), 0);
        let _ = jitter_millis(7, u64::MAX);
    }

    #[test]
    fn sos_resumes_first_only_after_safety_checks() {
        let ok = SafetyChecks {
            identity_ok: true,
            storage_ok: true,
        };
        assert_eq!(
            sos_decision(SosState::Pending, ok),
            SosDecision::ResumeHighestPriority
        );
        for bad in [
            SafetyChecks {
                identity_ok: false,
                storage_ok: true,
            },
            SafetyChecks {
                identity_ok: true,
                storage_ok: false,
            },
        ] {
            assert_eq!(
                sos_decision(SosState::Pending, bad),
                SosDecision::WaitForSafetyChecks
            );
        }
        assert_eq!(sos_decision(SosState::Expired, ok), SosDecision::Skip);
        assert_eq!(sos_decision(SosState::Cancelled, ok), SosDecision::Skip);
    }
}
