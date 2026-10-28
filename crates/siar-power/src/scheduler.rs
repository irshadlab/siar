//! §21, §23, §75-§80, §110-§112: the durable-deferral scheduler.
//!
//! Owns the *deferred* set — durable work the policy said "not now" to — and
//! decides when to hand it back. It does not run work and does not own it:
//! §75's "battery policy changes eligibility/quantum, not durable
//! ownership". The owning subsystem (files, DTN, sync…) keeps the real
//! operation; this keeps only a descriptor and a wait hint.
//!
//! Design points that are easy to get wrong:
//!
//! * **No re-evaluation unless inputs changed.** An entry is re-evaluated
//!   only when the policy epoch (or resource-pressure generation) moved, or
//!   a time gate (aging / deadline) was crossed. With thousands of deferred
//!   operations this keeps `poll` cheap (§133).
//! * **No thundering herd (§112).** When a charger appears, every deferral
//!   may become eligible at once. Release is ordered by priority then age,
//!   capped per poll, and each non-urgent entry gets a deterministic
//!   per-operation jitter before it is released.
//! * **Restart never resumes blindly (§111).** [`PowerScheduler::restore`]
//!   re-inserts persisted records as *deferred*; the normal poll path
//!   evaluates them against the **current** state first.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{PowerDiagnostics, PowerMetrics, WaitCounts};
use crate::error::{PowerError, Result};
use crate::policy::{evaluate_detailed, ResourcePressure, ResourceSnapshot, DEADLINE_GRACE_MS};
use crate::profile::PowerPolicyConfig;
use crate::tracker::{PowerStateEpoch, PowerTracker, RawPowerReading, Transition};
use crate::work::{
    DecisionReason, EnergyClass, Evaluation, PowerAwareWork, PowerDecision, WaitFor, WorkDomain,
    WorkPriority,
};

/// Caller-chosen operation id (the owning subsystem's own id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OpId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// Hard bound on the deferred set (Part 08: nothing unbounded).
    pub max_deferred: usize,
    /// §112: releases per `poll` beyond Critical/Interactive.
    pub release_batch: usize,
    /// §112: spread of the per-operation jitter, milliseconds.
    pub herd_window_ms: u64,
    /// §76: how long deferred work may wait before it is promoted.
    pub max_wait_normal_ms: u64,
    pub max_wait_bulk_ms: u64,
    pub max_wait_background_ms: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_deferred: 100_000,
            release_batch: 16,
            herd_window_ms: 30_000,
            max_wait_normal_ms: 15 * 60_000,
            max_wait_bulk_ms: 6 * 3_600_000,
            max_wait_background_ms: 24 * 3_600_000,
        }
    }
}

impl SchedulerConfig {
    const fn max_wait(&self, p: WorkPriority) -> u64 {
        match p {
            WorkPriority::Critical | WorkPriority::Interactive | WorkPriority::Normal => {
                self.max_wait_normal_ms
            }
            WorkPriority::Bulk => self.max_wait_bulk_ms,
            WorkPriority::Background => self.max_wait_background_ms,
        }
    }
}

/// §110: everything persisted about a deferral. **Not** persisted: the
/// epoch, jitter, eligibility timing or any scheduler memory — only what is
/// needed to re-evaluate from scratch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedDeferral {
    pub id: OpId,
    pub work: PowerAwareWork,
    pub wait_hint: WaitFor,
    pub deferred_at: u64,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    work: PowerAwareWork,
    wait: WaitFor,
    deferred_at: u64,
    /// Generation the entry was last evaluated under.
    evaluated_gen: u64,
    /// Earliest ms at which a *time-based* change (aging, deadline) can alter
    /// the verdict.
    time_gate: u64,
    /// Set when first found runnable in the current generation; the entry is
    /// released once `now >= release_at`.
    release_at: Option<u64>,
}

/// The result of [`PowerScheduler::submit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submission {
    pub evaluation: Evaluation,
    /// Epoch the decision was made under (§83).
    pub epoch: PowerStateEpoch,
}

/// One operation handed back for execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub id: OpId,
    pub work: PowerAwareWork,
    pub decision: PowerDecision,
    pub reason: DecisionReason,
}

#[derive(Debug, Clone)]
pub struct PowerScheduler {
    cfg: SchedulerConfig,
    tracker: PowerTracker,
    pressure: ResourcePressure,
    pressure_gen: u64,
    deferred: BTreeMap<OpId, Entry>,
    metrics: PowerMetrics,
}

impl PowerScheduler {
    #[must_use]
    pub fn new(policy: PowerPolicyConfig, cfg: SchedulerConfig) -> Self {
        Self {
            cfg,
            tracker: PowerTracker::new(policy),
            pressure: ResourcePressure::Normal,
            pressure_gen: 0,
            deferred: BTreeMap::new(),
            metrics: PowerMetrics::default(),
        }
    }

    #[must_use]
    pub const fn tracker(&self) -> &PowerTracker {
        &self.tracker
    }
    #[must_use]
    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }
    #[must_use]
    pub fn is_deferred(&self, id: OpId) -> bool {
        self.deferred.contains_key(&id)
    }

    /// Inputs generation: any change to the power epoch or to resource
    /// pressure invalidates every cached verdict.
    fn generation(&self) -> u64 {
        self.tracker
            .epoch()
            .0
            .wrapping_add(self.pressure_gen.rotate_left(32))
    }

    /// Feed a platform reading. Returns the transition if it was meaningful.
    pub fn observe(&mut self, raw: &RawPowerReading, now: u64) -> Option<Transition> {
        let t = self.tracker.observe(raw, now);
        if let Some(t) = &t {
            self.metrics.note_transition(t);
        }
        t
    }

    pub fn tick(&mut self, now: u64) -> Option<Transition> {
        let t = self.tracker.tick(now);
        if let Some(t) = &t {
            self.metrics.note_transition(t);
        }
        t
    }

    pub fn set_emergency(&mut self, active: bool) -> Option<Transition> {
        let t = self.tracker.set_emergency(active);
        if let Some(t) = &t {
            self.metrics.note_transition(t);
        }
        t
    }

    pub fn set_policy(&mut self, config: PowerPolicyConfig) -> Option<Transition> {
        let t = self.tracker.set_config(config);
        if let Some(t) = &t {
            self.metrics.note_transition(t);
        }
        t
    }

    /// Part 08 → Part 13: the resource layer's current pressure.
    pub fn set_pressure(&mut self, p: ResourcePressure) {
        if self.pressure != p {
            self.pressure = p;
            self.pressure_gen = self.pressure_gen.wrapping_add(1);
        }
    }

    fn snapshot(&self, now: u64) -> ResourceSnapshot {
        ResourceSnapshot {
            pressure: self.pressure,
            now_millis: now,
        }
    }

    fn evaluate(&self, work: &PowerAwareWork, now: u64) -> Evaluation {
        evaluate_detailed(
            work,
            self.tracker.state(),
            self.tracker.budget(),
            self.tracker.profile(),
            &self.tracker.policy().config,
            &self.snapshot(now),
        )
    }

    fn time_gate(&self, work: &PowerAwareWork, deferred_at: u64) -> u64 {
        let aging = if work.aged {
            u64::MAX
        } else {
            deferred_at.saturating_add(self.cfg.max_wait(work.priority))
        };
        let deadline = work
            .deadline
            .map_or(u64::MAX, |d| d.saturating_sub(DEADLINE_GRACE_MS));
        aging.min(deadline)
    }

    /// Evaluate new work. `Run`/`RunThrottled` are returned for the caller to
    /// execute now; `Defer` is **recorded here** until [`Self::poll`] releases
    /// it; `Drop`/`Reject` are not recorded.
    pub fn submit(&mut self, id: OpId, work: PowerAwareWork, now: u64) -> Result<Submission> {
        if self.deferred.contains_key(&id) {
            return Err(PowerError::Duplicate);
        }
        let evaluation = self.evaluate(&work, now);
        self.metrics.note_reason(
            evaluation.reason,
            evaluation.decision == PowerDecision::Defer,
            evaluation.decision == PowerDecision::Drop,
        );
        if evaluation.decision == PowerDecision::Defer {
            if self.deferred.len() >= self.cfg.max_deferred {
                return Err(PowerError::DeferredFull(self.cfg.max_deferred));
            }
            let wait = evaluation.wait.unwrap_or(WaitFor::Foreground);
            let gate = self.time_gate(&work, now);
            self.deferred.insert(
                id,
                Entry {
                    work,
                    wait,
                    deferred_at: now,
                    evaluated_gen: self.generation(),
                    time_gate: gate,
                    release_at: None,
                },
            );
        }
        Ok(Submission {
            evaluation,
            epoch: self.tracker.epoch(),
        })
    }

    /// §75: cancel a deferral (the owner dropped the operation).
    pub fn cancel(&mut self, id: OpId) -> bool {
        self.deferred.remove(&id).is_some()
    }

    /// §96-§97: the user tapped "Send now" for **this operation**. Returns
    /// the release if it may run; the runtime profile is untouched.
    pub fn promote(&mut self, id: OpId, now: u64) -> Option<Release> {
        let entry = self.deferred.get(&id)?;
        let mut work = entry.work;
        work.user_initiated = true;
        let eval = self.evaluate(&work, now);
        if eval.decision.runs() {
            self.deferred.remove(&id);
            PowerMetrics::bump(&mut self.metrics.user_overrides);
            Some(Release {
                id,
                work,
                decision: eval.decision,
                reason: eval.reason,
            })
        } else {
            None
        }
    }

    fn jitter_ms(&self, id: OpId) -> u64 {
        if self.cfg.herd_window_ms == 0 {
            return 0;
        }
        // splitmix64 over (id, epoch): deterministic, well spread, no RNG.
        let mut z = id.0 ^ self.tracker.epoch().0.rotate_left(17) ^ 0x9E37_79B9_7F4A_7C15;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        z % self.cfg.herd_window_ms
    }

    /// Hand back deferred work that may run now. At most `limit` entries (and
    /// at most `release_batch` non-urgent ones) per call; call again to
    /// continue draining (§112).
    pub fn poll(&mut self, now: u64, limit: usize) -> Vec<Release> {
        let generation = self.generation();
        let charging_now = self.tracker.state().external_power();

        // Dispatch order: priority, then age, then id (stable, deterministic).
        let mut order: Vec<(WorkPriority, u64, OpId)> = self
            .deferred
            .iter()
            .map(|(id, e)| (e.work.priority, e.deferred_at, *id))
            .collect();
        order.sort_unstable();

        let mut out = Vec::new();
        let mut non_urgent = 0usize;
        for (_, _, id) in order {
            if out.len() >= limit {
                break;
            }
            let Some(entry) = self.deferred.get(&id).copied() else {
                continue;
            };

            // Cheap skip: nothing relevant changed since the last verdict.
            let stale = entry.evaluated_gen != generation || now >= entry.time_gate;
            if !stale && entry.release_at.is_none() {
                continue;
            }

            let mut work = entry.work;
            if now.saturating_sub(entry.deferred_at) >= self.cfg.max_wait(work.priority) {
                work.aged = true;
            }
            let eval = if stale {
                self.evaluate(&work, now)
            } else {
                Evaluation::throttled(DecisionReason::Unrestricted)
            };

            let urgent = work.priority <= WorkPriority::Interactive;
            let e = self.deferred.get_mut(&id).expect("present");
            e.evaluated_gen = generation;
            e.work.aged = work.aged;
            if !eval.decision.runs() {
                e.release_at = None;
                if let Some(w) = eval.wait {
                    e.wait = w;
                }
                // Next time-based change: a gate still in the future, else none
                // (an aged or past-deadline entry only changes with new inputs).
                let tg = self.time_gate(&work, entry.deferred_at);
                if let Some(e) = self.deferred.get_mut(&id) {
                    e.time_gate = if tg > now { tg } else { u64::MAX };
                }
                continue;
            }

            // Runnable: apply herd control.
            let jitter = if urgent { 0 } else { self.jitter_ms(id) };
            let e = self.deferred.get_mut(&id).expect("present");
            let at = *e.release_at.get_or_insert(now.saturating_add(jitter));
            if now < at {
                continue;
            }
            if !urgent {
                if non_urgent >= self.cfg.release_batch {
                    continue;
                }
                non_urgent += 1;
            }
            let wait = e.wait;
            self.deferred.remove(&id);
            if wait == WaitFor::Charging && charging_now {
                PowerMetrics::bump(&mut self.metrics.bulk_resumed_on_charge);
            }
            if eval.reason == DecisionReason::Aged {
                PowerMetrics::bump(&mut self.metrics.aged_promotions);
            }
            out.push(Release {
                id,
                work,
                decision: eval.decision,
                reason: eval.reason,
            });
        }
        out
    }

    /// §110: records to persist. Call after any change to the deferred set.
    #[must_use]
    pub fn persisted(&self) -> Vec<PersistedDeferral> {
        self.deferred
            .iter()
            .map(|(id, e)| PersistedDeferral {
                id: *id,
                work: e.work,
                wait_hint: e.wait,
                deferred_at: e.deferred_at,
            })
            .collect()
    }

    /// §111: re-insert persisted deferrals after a restart. **Nothing is
    /// resumed here** — every record goes back in as deferred and is judged
    /// by the normal path against the *current* power state. Records beyond
    /// the cap, or with an impossible shape, are skipped (returned count is
    /// the number accepted).
    pub fn restore(&mut self, records: impl IntoIterator<Item = PersistedDeferral>) -> usize {
        let mut accepted = 0;
        for r in records {
            if self.deferred.len() >= self.cfg.max_deferred || self.deferred.contains_key(&r.id) {
                continue;
            }
            // Only durable work is ever persisted (§21/§110).
            if !r.work.durable {
                continue;
            }
            let gate = self.time_gate(&r.work, r.deferred_at);
            self.deferred.insert(
                r.id,
                Entry {
                    work: r.work,
                    wait: r.wait_hint,
                    deferred_at: r.deferred_at,
                    evaluated_gen: u64::MAX,
                    time_gate: gate,
                    release_at: None,
                },
            );
            accepted += 1;
        }
        accepted
    }

    #[must_use]
    pub fn diagnostics(&self) -> PowerDiagnostics {
        let mut deferred = WaitCounts::default();
        for e in self.deferred.values() {
            deferred.add(e.wait);
        }
        PowerDiagnostics {
            epoch: self.tracker.epoch(),
            profile: self.tracker.profile(),
            state: *self.tracker.state(),
            budget: *self.tracker.budget(),
            deferred,
            metrics: self.metrics,
        }
    }

    /// For domain wrappers and tests that report a metric from outside.
    pub fn metrics_mut(&mut self) -> &mut PowerMetrics {
        &mut self.metrics
    }
}

/// Convenience: classify an operation by size, per §25's examples.
#[must_use]
pub const fn energy_for_bytes(bytes: u64) -> EnergyClass {
    match bytes {
        0..=1_024 => EnergyClass::Tiny,
        1_025..=65_536 => EnergyClass::Low,
        65_537..=33_554_432 => EnergyClass::Medium,
        _ => EnergyClass::High,
    }
}

/// Which `WorkDomain`s count as "large transfers" for §77's charging trigger.
#[must_use]
pub const fn resumes_on_charge(d: WorkDomain) -> bool {
    matches!(
        d,
        WorkDomain::Files
            | WorkDomain::Dtn
            | WorkDomain::Relay
            | WorkDomain::Indexing
            | WorkDomain::Backup
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppActivityState, ChargingState, ThermalState};

    fn raw(pct: u8, charging: ChargingState) -> RawPowerReading {
        RawPowerReading {
            battery_percent: Some(pct),
            charging,
            thermal: ThermalState::Nominal,
            activity: AppActivityState::BackgroundAllowed,
            ..Default::default()
        }
    }
    fn sched() -> PowerScheduler {
        PowerScheduler::new(PowerPolicyConfig::default(), SchedulerConfig::default())
    }
    fn bulk(energy: EnergyClass) -> PowerAwareWork {
        PowerAwareWork::new(WorkDomain::Files, WorkPriority::Bulk, energy)
    }

    #[test]
    fn low_battery_defers_then_charging_resumes_it() {
        let mut s = sched();
        s.observe(&raw(15, ChargingState::NotCharging), 0);
        let sub = s.submit(OpId(1), bulk(EnergyClass::High), 10).unwrap();
        assert_eq!(sub.evaluation.decision, PowerDecision::Defer);
        assert!(s.is_deferred(OpId(1)));
        assert!(
            s.poll(20, 10).is_empty(),
            "nothing changed, nothing released"
        );

        let tr = s
            .observe(&raw(16, ChargingState::ChargingFast), 30)
            .unwrap();
        assert!(tr.charging_started());
        // Herd control: eligible but jittered; drain over the window.
        let mut released = Vec::new();
        let mut t = 30;
        while released.is_empty() && t < 30 + 31_000 {
            released.extend(s.poll(t, 10));
            t += 500;
        }
        assert_eq!(released.len(), 1, "released within the herd window");
        assert_eq!(released[0].id, OpId(1));
        assert!(!s.is_deferred(OpId(1)));
        assert_eq!(s.diagnostics().metrics.bulk_resumed_on_charge, 1);
    }

    #[test]
    fn durable_work_is_not_lost_when_conditions_flap() {
        let mut s = sched();
        s.observe(&raw(9, ChargingState::NotCharging), 0);
        for i in 0..50 {
            let _ = s.submit(OpId(i), bulk(EnergyClass::Medium), 1);
        }
        assert_eq!(s.deferred_len(), 50);
        s.observe(&raw(9, ChargingState::ChargingSlow), 2);
        s.observe(&raw(9, ChargingState::NotCharging), 3);
        assert_eq!(s.deferred_len(), 50, "flapping never drops durable work");
    }

    #[test]
    fn thundering_herd_is_spread_and_batched() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                release_batch: 8,
                herd_window_ms: 10_000,
                ..Default::default()
            },
        );
        s.observe(&raw(8, ChargingState::NotCharging), 0);
        for i in 0..200 {
            s.submit(OpId(i), bulk(EnergyClass::Medium), 1).unwrap();
        }
        s.observe(&raw(8, ChargingState::ChargingFast), 5);

        let first = s.poll(5, 1_000);
        assert!(
            first.is_empty(),
            "jitter means nothing is released at the instant of the trigger"
        );

        let mut per_poll_max = 0;
        let mut total = 0;
        let mut t = 5;
        while total < 200 && t < 5 + 60_000 {
            let r = s.poll(t, 1_000);
            per_poll_max = per_poll_max.max(r.len());
            total += r.len();
            t += 250;
        }
        assert_eq!(total, 200, "everything eventually runs — no starvation");
        assert!(
            per_poll_max <= 8,
            "never more than release_batch per poll, saw {per_poll_max}"
        );
    }

    #[test]
    fn dispatch_order_is_priority_then_age() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                herd_window_ms: 0,
                ..Default::default()
            },
        );
        s.observe(&raw(8, ChargingState::NotCharging), 0);
        let normal =
            PowerAwareWork::new(WorkDomain::Sync, WorkPriority::Normal, EnergyClass::Medium);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 1).unwrap();
        s.submit(OpId(2), normal, 2).unwrap();
        s.submit(OpId(3), bulk(EnergyClass::Medium), 3).unwrap();
        s.observe(&raw(8, ChargingState::Full), 10);
        let ids: Vec<u64> = s.poll(10, 10).iter().map(|r| r.id.0).collect();
        assert_eq!(ids, vec![2, 1, 3]);
    }

    #[test]
    fn restart_does_not_resume_bulk_on_a_low_battery() {
        // Before the crash: three deferrals, persisted.
        let mut before = sched();
        before.observe(&raw(12, ChargingState::NotCharging), 0);
        for i in 0..3 {
            before.submit(OpId(i), bulk(EnergyClass::High), 1).unwrap();
        }
        let saved = before.persisted();
        assert_eq!(saved.len(), 3);

        // After the restart, still on a low battery.
        let mut after = sched();
        after.observe(&raw(12, ChargingState::NotCharging), 100);
        assert_eq!(after.restore(saved), 3);
        assert!(
            after.poll(101, 100).is_empty(),
            "§111: must re-evaluate current state, not resume"
        );
        assert_eq!(after.deferred_len(), 3);

        // Plugged in later: now they flow.
        after.observe(&raw(12, ChargingState::ChargingFast), 200);
        let mut n = 0;
        let mut t = 200;
        while n < 3 && t < 200 + 40_000 {
            n += after.poll(t, 100).len();
            t += 500;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn persisted_records_hold_no_exact_battery_or_scheduler_state() {
        let mut s = sched();
        s.observe(&raw(12, ChargingState::NotCharging), 0);
        s.submit(OpId(7), bulk(EnergyClass::High), 1).unwrap();
        let json_like = format!("{:?}", s.persisted());
        assert!(!json_like.contains("percent"));
        assert!(!json_like.contains("epoch"));
        assert!(!json_like.contains("release_at"));
    }

    #[test]
    fn restore_skips_ephemeral_duplicates_and_overflow() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                max_deferred: 2,
                ..Default::default()
            },
        );
        let good = |id| PersistedDeferral {
            id: OpId(id),
            work: bulk(EnergyClass::Medium),
            wait_hint: WaitFor::Charging,
            deferred_at: 0,
        };
        let ephemeral = PersistedDeferral {
            work: bulk(EnergyClass::Medium).with_durable(false),
            ..good(9)
        };
        assert_eq!(
            s.restore([good(1), good(1), ephemeral, good(2), good(3)]),
            2
        );
    }

    #[test]
    fn aging_eventually_promotes_deferred_normal_work() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                herd_window_ms: 0,
                max_wait_bulk_ms: 10_000,
                ..Default::default()
            },
        );
        s.observe(&raw(12, ChargingState::NotCharging), 0);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 0).unwrap();
        assert!(s.poll(5_000, 10).is_empty());
        let r = s.poll(10_001, 10);
        assert_eq!(r.len(), 1, "no permanent starvation (§76)");
        assert_eq!(r[0].reason, DecisionReason::Aged);
        assert_eq!(r[0].decision, PowerDecision::RunThrottled);
    }

    #[test]
    fn aging_cannot_pierce_the_critical_reserve() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                herd_window_ms: 0,
                max_wait_bulk_ms: 10,
                ..Default::default()
            },
        );
        s.observe(&raw(5, ChargingState::NotCharging), 0);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 0).unwrap();
        assert!(s.poll(1_000_000, 10).is_empty());
    }

    #[test]
    fn promote_overrides_one_operation_not_the_profile() {
        let mut s = sched();
        s.observe(&raw(15, ChargingState::NotCharging), 0);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 1).unwrap();
        s.submit(OpId(2), bulk(EnergyClass::Medium), 1).unwrap();
        let profile = s.tracker().profile();
        let r = s.promote(OpId(1), 2).expect("user override");
        assert_eq!(r.reason, DecisionReason::UserInitiated);
        assert!(s.is_deferred(OpId(2)));
        assert_eq!(s.tracker().profile(), profile);
        assert!(s.promote(OpId(99), 3).is_none());
    }

    #[test]
    fn promote_cannot_beat_thermal_critical() {
        let mut s = sched();
        let hot = RawPowerReading {
            thermal: ThermalState::Critical,
            charging: ChargingState::Full,
            ..Default::default()
        };
        s.observe(&hot, 0);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 1).unwrap();
        assert!(s.promote(OpId(1), 2).is_none());
    }

    #[test]
    fn duplicate_and_capacity_errors() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                max_deferred: 1,
                ..Default::default()
            },
        );
        s.observe(&raw(8, ChargingState::NotCharging), 0);
        s.submit(OpId(1), bulk(EnergyClass::Medium), 1).unwrap();
        assert_eq!(
            s.submit(OpId(1), bulk(EnergyClass::Medium), 1),
            Err(PowerError::Duplicate)
        );
        assert_eq!(
            s.submit(OpId(2), bulk(EnergyClass::Medium), 1),
            Err(PowerError::DeferredFull(1))
        );
    }

    #[test]
    fn pressure_change_invalidates_cached_verdicts() {
        let mut s = PowerScheduler::new(
            PowerPolicyConfig::default(),
            SchedulerConfig {
                herd_window_ms: 0,
                ..Default::default()
            },
        );
        s.observe(&raw(80, ChargingState::ChargingFast), 0);
        s.set_pressure(ResourcePressure::High);
        s.submit(OpId(1), bulk(EnergyClass::Low), 1).unwrap();
        assert!(s.is_deferred(OpId(1)));
        assert!(s.poll(2, 10).is_empty());
        s.set_pressure(ResourcePressure::Normal);
        assert_eq!(s.poll(3, 10).len(), 1);
    }

    #[test]
    fn diagnostics_counts_what_is_waiting_for_what() {
        let mut s = sched();
        s.observe(&raw(10, ChargingState::NotCharging), 0);
        s.submit(OpId(1), bulk(EnergyClass::High), 1).unwrap();
        let d = s.diagnostics();
        assert_eq!(d.deferred.charging, 1);
        assert_eq!(d.metrics.deferred_by_power, 1);
        assert!(d.metrics.mode_transitions >= 1);
    }

    #[test]
    fn size_classes_follow_the_specs_examples() {
        assert_eq!(energy_for_bytes(64), EnergyClass::Tiny);
        assert_eq!(energy_for_bytes(5_000), EnergyClass::Low);
        assert_eq!(energy_for_bytes(4_000_000), EnergyClass::Medium);
        assert_eq!(energy_for_bytes(5_000_000_000), EnergyClass::High);
    }
}
