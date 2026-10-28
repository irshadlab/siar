//! §20, §26-§28, §68, §72-§74, §96-§97: the policy engine.
//!
//! [`evaluate_detailed`] is a **pure function** of (work, state, budget,
//! config, resources). Layers are applied in §73's order and each layer can
//! only *restrict* what the layer below it would have allowed:
//!
//! ```text
//! OS hard restriction → system safety (thermal/resource) → critical
//! priority → critical-battery reserve → user & application policy →
//! user-initiated override → battery profile (budget) → thermal/resource
//! throttles → deadline/aging promotion
//! ```
//!
//! Promotion (deadline close, or aged past its maximum wait — §76) can only
//! lift the *power-derived* deferrals at the bottom. It can never lift an OS
//! restriction, thermal safety, the critical reserve, or an explicit user
//! policy such as "no bulk on mobile data".

use crate::budget::{BackgroundBudgetClass, PowerBudget};
use crate::profile::{select_profile, PowerPolicyConfig, PowerPolicyProfile};
use crate::state::{AppActivityState, PowerState};
use crate::work::{
    DecisionReason as R, EnergyClass, Evaluation, PowerAwareWork, PowerDecision, WaitFor,
    WorkDomain, WorkPriority,
};

/// Pressure from the resource-limits layer (Part 08), reduced to what power
/// policy needs. `siar-resource-limits` maps its own `PressureState` onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ResourcePressure {
    #[default]
    Normal,
    Elevated,
    High,
    Critical,
}

/// The `resources` argument of §68, plus the runtime clock (§172) that
/// deadline checks need.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceSnapshot {
    pub pressure: ResourcePressure,
    pub now_millis: u64,
}

/// §68.
pub trait PowerPolicyEngine {
    fn evaluate(
        &self,
        work: &PowerAwareWork,
        state: &PowerState,
        resources: &ResourceSnapshot,
    ) -> PowerDecision;
}

/// A deadline this close promotes durable deferred work (still subject to
/// every hard layer).
pub const DEADLINE_GRACE_MS: u64 = 60_000;

/// The standard engine: derives profile and budget from the state each call.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPowerPolicy {
    pub config: PowerPolicyConfig,
    pub emergency_active: bool,
}

impl DefaultPowerPolicy {
    #[must_use]
    pub const fn new(config: PowerPolicyConfig) -> Self {
        Self {
            config,
            emergency_active: false,
        }
    }

    #[must_use]
    pub fn profile(&self, state: &PowerState) -> PowerPolicyProfile {
        select_profile(state, &self.config, self.emergency_active)
    }

    #[must_use]
    pub fn budget(&self, state: &PowerState) -> PowerBudget {
        PowerBudget::derive(self.profile(state), state, &self.config)
    }

    #[must_use]
    pub fn evaluate_detailed(
        &self,
        work: &PowerAwareWork,
        state: &PowerState,
        res: &ResourceSnapshot,
    ) -> Evaluation {
        let profile = self.profile(state);
        let budget = PowerBudget::derive(profile, state, &self.config);
        evaluate_detailed(work, state, &budget, profile, &self.config, res)
    }
}

impl PowerPolicyEngine for DefaultPowerPolicy {
    fn evaluate(
        &self,
        work: &PowerAwareWork,
        state: &PowerState,
        resources: &ResourceSnapshot,
    ) -> PowerDecision {
        self.evaluate_detailed(work, state, resources).decision
    }
}

/// §21-§22: work that cannot run now is **kept** when durable, **dropped**
/// when it is ephemeral and deferrable, and **rejected** when it is
/// ephemeral and the caller needs an immediate yes/no.
fn postpone(work: &PowerAwareWork, reason: R, wait: WaitFor) -> Evaluation {
    if work.durable {
        Evaluation {
            decision: PowerDecision::Defer,
            reason,
            wait: Some(wait),
        }
    } else if work.deferrable {
        Evaluation {
            decision: PowerDecision::Drop,
            reason: R::Ephemeral,
            wait: None,
        }
    } else {
        Evaluation {
            decision: PowerDecision::Reject,
            reason,
            wait: None,
        }
    }
}

/// §29: what may still run on a critically low, unplugged battery.
fn reserve_allows(work: &PowerAwareWork) -> bool {
    work.energy_class <= EnergyClass::Low
        && (work.domain.in_critical_reserve()
            || (work.user_initiated && work.priority <= WorkPriority::Interactive))
}

/// The most useful thing a deferred operation can wait for, given why the
/// budget is tight.
fn natural_wait(state: &PowerState) -> WaitFor {
    if state.battery_constrained() || state.in_critical_reserve() {
        WaitFor::Charging
    } else if state.thermal.severity() >= 1 {
        WaitFor::ThermalRecovery
    } else if state.activity.is_restricted() {
        WaitFor::Foreground
    } else if state.costly_network() {
        WaitFor::UnmeteredNetwork
    } else {
        WaitFor::Foreground
    }
}

/// The full layered evaluation. See the module docs for the layer order.
#[must_use]
pub fn evaluate_detailed(
    work: &PowerAwareWork,
    state: &PowerState,
    budget: &PowerBudget,
    profile: PowerPolicyProfile,
    config: &PowerPolicyConfig,
    res: &ResourceSnapshot,
) -> Evaluation {
    let critical = work.priority == WorkPriority::Critical;
    let constrained = state.battery_constrained() || state.thermal.severity() >= 2;

    // ── Layer 1: OS hard restriction (§72). Not even critical work passes.
    if work.uses_network && budget.network_blocked_by_os {
        return postpone(work, R::OsRestriction, WaitFor::Foreground);
    }

    // ── Layer 2: system safety (§20). Thermal-critical and resource-critical
    // are absolute; only tiny critical work may continue under thermal-critical.
    if state.thermal.severity() >= 3 && !(critical && work.energy_class <= EnergyClass::Low) {
        return postpone(work, R::ThermalSafety, WaitFor::ThermalRecovery);
    }
    if res.pressure == crate::policy::ResourcePressure::Critical && !critical {
        return postpone(work, R::ResourcePressure, WaitFor::ResourceRelief);
    }

    // ── Layer 3: critical priority overrides saver (§28) but still orders
    // efficiently — heavy critical work on a constrained device is throttled.
    if critical {
        return if constrained && work.energy_class >= EnergyClass::Medium {
            Evaluation::throttled(R::CriticalOverride)
        } else {
            Evaluation::run(R::CriticalOverride)
        };
    }

    // ── Layer 4: critical battery reserve (§29).
    if state.in_critical_reserve() && !reserve_allows(work) {
        return postpone(work, R::CriticalBatteryReserve, WaitFor::Charging);
    }

    // ── Layer 5: user & application policy (§70-§71, §74).
    let emergency_may_spend =
        profile == PowerPolicyProfile::Emergency && config.emergency_consent_metered;
    if work.is_bulk_like()
        && state.costly_network()
        && !config.allow_metered_bulk
        && !work.user_initiated
        && !emergency_may_spend
    {
        return postpone(work, R::MeteredNetwork, WaitFor::UnmeteredNetwork);
    }
    if config.defer_on_battery.contains(work.domain)
        && !state.external_power()
        && !work.user_initiated
        && work.priority >= WorkPriority::Normal
    {
        return postpone(work, R::AppPolicy, WaitFor::Charging);
    }
    if config.small_messages_immediate
        && work.domain == WorkDomain::Message
        && work.energy_class <= EnergyClass::Low
        && work.priority <= WorkPriority::Normal
    {
        return Evaluation::run(R::SmallMessageImmediate);
    }

    // ── Layer 6: user-initiated override (§96-§97): this operation only.
    if work.user_initiated {
        return if constrained {
            Evaluation::throttled(R::UserInitiated)
        } else {
            Evaluation::run(R::UserInitiated)
        };
    }

    // ── Layer 6b: §29 — work the critical reserve exists for (tiny messages,
    // ACKs, identity/security) runs even though the background budget is
    // `CriticalOnly`. Layer 4 already filtered everything else out.
    if state.in_critical_reserve() {
        return Evaluation::run(R::Unrestricted);
    }

    // ── Layer 7: battery profile via the budget, then throttles.
    let mut eval = power_budget_verdict(work, state, budget, res);

    // ── Layer 8: promotion (§76). Lifts only power-derived deferrals.
    if eval.decision == PowerDecision::Defer && work.durable {
        let near = work
            .deadline
            .is_some_and(|d| d.saturating_sub(res.now_millis) <= DEADLINE_GRACE_MS);
        if near {
            eval = Evaluation::throttled(R::DeadlineNear);
        } else if work.aged {
            eval = Evaluation::throttled(R::Aged);
        }
    }
    eval
}

fn power_budget_verdict(
    work: &PowerAwareWork,
    state: &PowerState,
    budget: &PowerBudget,
    res: &ResourceSnapshot,
) -> Evaluation {
    use BackgroundBudgetClass as B;
    let energy = work.energy_class;
    let bulkish = work.priority >= WorkPriority::Bulk;

    let mut eval = match work.priority {
        WorkPriority::Critical => Evaluation::run(R::CriticalOverride), // unreachable via evaluate_detailed
        WorkPriority::Interactive => Evaluation::run(R::Unrestricted),
        _ => match budget.background_class {
            B::CriticalOnly => return postpone(work, R::BackgroundBudget, natural_wait(state)),
            B::Deferred => {
                if work.priority == WorkPriority::Normal && energy <= EnergyClass::Low {
                    Evaluation::run(R::Unrestricted)
                } else {
                    return postpone(work, R::BackgroundBudget, natural_wait(state));
                }
            }
            B::Limited => {
                if bulkish && energy >= EnergyClass::High && !state.external_power() {
                    return postpone(work, R::BatteryThrottle, WaitFor::Charging);
                }
                if bulkish {
                    Evaluation::throttled(R::BatteryThrottle)
                } else {
                    Evaluation::run(R::Unrestricted)
                }
            }
            B::Normal => Evaluation::run(R::Unrestricted),
        },
    };

    // Thermal throttle (§7, §50, §55).
    let sev = state.thermal.severity();
    if sev >= 2 && energy == EnergyClass::VeryHigh {
        return postpone(work, R::ThermalThrottle, WaitFor::ThermalRecovery);
    }
    if eval.decision == PowerDecision::Run && sev >= 1 && energy >= EnergyClass::High {
        eval = Evaluation::throttled(R::ThermalThrottle);
    }
    if sev >= 2 && eval.decision == PowerDecision::Run && energy >= EnergyClass::Medium {
        eval = Evaluation::throttled(R::ThermalThrottle);
    }

    // Resource pressure (Part 08 interplay).
    match res.pressure {
        ResourcePressure::High if bulkish => {
            return postpone(work, R::ResourcePressure, WaitFor::ResourceRelief)
        }
        ResourcePressure::Elevated | ResourcePressure::High
            if eval.decision == PowerDecision::Run && bulkish =>
        {
            eval = Evaluation::throttled(R::ResourcePressure);
        }
        _ => {}
    }
    eval
}

/// Is background activity restricted such that "send now" is the only way?
#[must_use]
pub const fn needs_user_action(state: &PowerState) -> bool {
    matches!(state.activity, AppActivityState::SuspendedLikely)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::*;
    use crate::work::DomainMask;
    use proptest::prelude::*;

    fn engine() -> DefaultPowerPolicy {
        DefaultPowerPolicy::default()
    }
    fn res() -> ResourceSnapshot {
        ResourceSnapshot::default()
    }
    fn unplugged(b: BatteryLevelClass) -> PowerState {
        PowerState {
            battery: b,
            charging: ChargingState::NotCharging,
            activity: AppActivityState::BackgroundAllowed,
            ..Default::default()
        }
    }
    fn file(energy: EnergyClass) -> PowerAwareWork {
        PowerAwareWork::new(WorkDomain::Files, WorkPriority::Bulk, energy)
    }
    fn sos() -> PowerAwareWork {
        PowerAwareWork::new(
            WorkDomain::Message,
            WorkPriority::Critical,
            EnergyClass::Tiny,
        )
    }

    #[test]
    fn durable_bulk_on_low_battery_is_deferred_not_dropped() {
        let e = engine().evaluate_detailed(
            &file(EnergyClass::High),
            &unplugged(BatteryLevelClass::Low),
            &res(),
        );
        assert_eq!(e.decision, PowerDecision::Defer);
        assert_eq!(e.wait, Some(WaitFor::Charging));
    }

    #[test]
    fn ephemeral_work_is_dropped_when_budget_is_tight() {
        let typing = PowerAwareWork::new(
            WorkDomain::Presence,
            WorkPriority::Background,
            EnergyClass::Tiny,
        );
        let e = engine().evaluate_detailed(&typing, &unplugged(BatteryLevelClass::Low), &res());
        assert_eq!(e.decision, PowerDecision::Drop);
        let must_answer = typing.with_deferrable(false);
        assert_eq!(
            engine().evaluate(&must_answer, &unplugged(BatteryLevelClass::Low), &res()),
            PowerDecision::Reject
        );
    }

    #[test]
    fn sos_runs_on_critical_battery_but_bulk_does_not() {
        let s = unplugged(BatteryLevelClass::Critical);
        assert_eq!(engine().evaluate(&sos(), &s, &res()), PowerDecision::Run);
        assert_eq!(
            engine().evaluate(&file(EnergyClass::Medium), &s, &res()),
            PowerDecision::Defer
        );
    }

    #[test]
    fn critical_reserve_keeps_small_messages_acks_and_identity() {
        let s = unplugged(BatteryLevelClass::Critical);
        for d in [
            WorkDomain::Message,
            WorkDomain::Receipt,
            WorkDomain::Identity,
        ] {
            let w = PowerAwareWork::new(d, WorkPriority::Normal, EnergyClass::Tiny);
            assert!(engine().evaluate(&w, &s, &res()).runs(), "{d:?}");
        }
        for d in [
            WorkDomain::Files,
            WorkDomain::Relay,
            WorkDomain::Sync,
            WorkDomain::Indexing,
        ] {
            let w = PowerAwareWork::new(d, WorkPriority::Normal, EnergyClass::Low);
            assert!(
                !engine().evaluate(&w, &s, &res()).runs(),
                "{d:?} must pause at the critical reserve"
            );
        }
    }

    #[test]
    fn os_restriction_beats_everything_including_critical() {
        let s = PowerState {
            os_background_network_blocked: true,
            activity: AppActivityState::BackgroundRestricted,
            ..Default::default()
        };
        let e = engine().evaluate_detailed(&sos(), &s, &res());
        // SOS is durable (Message) → deferred, not run.
        assert_eq!(e.decision, PowerDecision::Defer);
        assert_eq!(e.reason, R::OsRestriction);
        // Non-network work is unaffected.
        let local =
            PowerAwareWork::new(WorkDomain::Indexing, WorkPriority::Normal, EnergyClass::Low);
        assert!(
            engine().evaluate(&local, &s, &res()).runs()
                || engine().evaluate(&local, &s, &res()) == PowerDecision::Defer
        );
    }

    #[test]
    fn thermal_critical_stops_everything_but_tiny_critical() {
        let s = PowerState {
            thermal: ThermalState::Critical,
            charging: ChargingState::Full,
            ..Default::default()
        };
        assert!(engine().evaluate(&sos(), &s, &res()).runs());
        let big_critical =
            PowerAwareWork::new(WorkDomain::Media, WorkPriority::Critical, EnergyClass::High)
                .with_durable(false);
        assert!(!engine().evaluate(&big_critical, &s, &res()).runs());
        let m = PowerAwareWork::new(
            WorkDomain::Message,
            WorkPriority::Interactive,
            EnergyClass::Tiny,
        );
        assert_eq!(
            engine().evaluate(&m.user_initiated(), &s, &res()),
            PowerDecision::Defer,
            "user override cannot beat thermal safety (§96)"
        );
    }

    #[test]
    fn user_initiated_overrides_saver_for_that_op_only() {
        let s = unplugged(BatteryLevelClass::Low);
        let bulk = file(EnergyClass::Medium);
        assert_eq!(engine().evaluate(&bulk, &s, &res()), PowerDecision::Defer);
        let e = engine().evaluate_detailed(&bulk.user_initiated(), &s, &res());
        assert!(e.decision.runs());
        assert_eq!(e.reason, R::UserInitiated);
        // The runtime's profile did not change.
        assert_eq!(engine().profile(&s), PowerPolicyProfile::Saver);
    }

    #[test]
    fn metered_network_defers_bulk_unless_user_asks_or_consent_given() {
        let s = PowerState {
            charging: ChargingState::Full,
            metered_network: true,
            ..Default::default()
        };
        let w = file(EnergyClass::Medium);
        assert_eq!(
            engine().evaluate_detailed(&w, &s, &res()).reason,
            R::MeteredNetwork
        );
        assert!(engine().evaluate(&w.user_initiated(), &s, &res()).runs());
        let mut e = engine();
        e.config.allow_metered_bulk = true;
        assert!(e.evaluate(&w, &s, &res()).runs());
    }

    #[test]
    fn emergency_does_not_silently_spend_mobile_data() {
        let s = PowerState {
            metered_network: true,
            ..Default::default()
        };
        let w = PowerAwareWork::new(WorkDomain::Relay, WorkPriority::Bulk, EnergyClass::Medium);
        let mut e = engine();
        e.emergency_active = true;
        assert_eq!(e.evaluate(&w, &s, &res()), PowerDecision::Defer);
        e.config.emergency_consent_metered = true;
        // With explicit consent the metered rule steps aside; the Emergency
        // budget (CriticalOnly background) still defers non-critical bulk.
        assert_eq!(
            e.evaluate_detailed(&w, &s, &res()).reason,
            R::BackgroundBudget
        );
    }

    #[test]
    fn app_policy_can_defer_a_domain_on_battery() {
        let mut e = engine();
        e.config.defer_on_battery = DomainMask::EMPTY.with(WorkDomain::Sync);
        let sync = PowerAwareWork::new(WorkDomain::Sync, WorkPriority::Normal, EnergyClass::Low);
        let on_bat = unplugged(BatteryLevelClass::High);
        assert_eq!(
            e.evaluate_detailed(&sync, &on_bat, &res()).reason,
            R::AppPolicy
        );
        let plugged = PowerState {
            charging: ChargingState::ChargingFast,
            ..on_bat
        };
        assert!(e.evaluate(&sync, &plugged, &res()).runs());
    }

    #[test]
    fn small_messages_are_immediate_even_in_saver() {
        let m = PowerAwareWork::new(WorkDomain::Message, WorkPriority::Normal, EnergyClass::Low);
        let e = engine().evaluate_detailed(&m, &unplugged(BatteryLevelClass::Low), &res());
        assert_eq!(e, Evaluation::run(R::SmallMessageImmediate));
    }

    #[test]
    fn thermal_pressure_throttles_then_defers_software_av1() {
        let av1 = PowerAwareWork::new(
            WorkDomain::Media,
            WorkPriority::Interactive,
            EnergyClass::VeryHigh,
        )
        .with_durable(true);
        let hot = |t| PowerState {
            thermal: t,
            charging: ChargingState::Full,
            ..Default::default()
        };
        assert_eq!(
            engine().evaluate(&av1, &hot(ThermalState::Nominal), &res()),
            PowerDecision::Run
        );
        assert_eq!(
            engine().evaluate(&av1, &hot(ThermalState::Elevated), &res()),
            PowerDecision::RunThrottled
        );
        assert_eq!(
            engine().evaluate(&av1, &hot(ThermalState::Serious), &res()),
            PowerDecision::Defer
        );
    }

    #[test]
    fn deadline_and_aging_promote_but_never_pierce_hard_layers() {
        let s = unplugged(BatteryLevelClass::Low);
        let mut w = file(EnergyClass::Medium).with_deadline(100_000);
        let near = ResourceSnapshot {
            now_millis: 50_000,
            ..res()
        };
        assert_eq!(
            engine().evaluate_detailed(&w, &s, &near).reason,
            R::DeadlineNear
        );
        let far = ResourceSnapshot {
            now_millis: 0,
            ..res()
        };
        assert_eq!(engine().evaluate(&w, &s, &far), PowerDecision::Defer);
        w.aged = true;
        assert_eq!(engine().evaluate_detailed(&w, &s, &far).reason, R::Aged);
        // …but not at the critical reserve, under thermal-critical, or on a blocked network.
        assert_eq!(
            engine().evaluate(&w, &unplugged(BatteryLevelClass::Critical), &near),
            PowerDecision::Defer
        );
        let hot = PowerState {
            thermal: ThermalState::Critical,
            ..s
        };
        assert_eq!(engine().evaluate(&w, &hot, &near), PowerDecision::Defer);
        let blocked = PowerState {
            os_background_network_blocked: true,
            activity: AppActivityState::BackgroundRestricted,
            ..s
        };
        assert_eq!(engine().evaluate(&w, &blocked, &near), PowerDecision::Defer);
        // Aging does not defeat an explicit "no mobile data" policy either.
        let metered = PowerState {
            charging: ChargingState::Full,
            metered_network: true,
            ..Default::default()
        };
        assert_eq!(
            engine().evaluate_detailed(&w, &metered, &near).reason,
            R::MeteredNetwork
        );
    }

    #[test]
    fn resource_pressure_defers_bulk_and_spares_interactive() {
        let s = PowerState {
            charging: ChargingState::Full,
            ..Default::default()
        };
        let hi = ResourceSnapshot {
            pressure: ResourcePressure::High,
            now_millis: 0,
        };
        assert_eq!(
            engine().evaluate(&file(EnergyClass::Low), &s, &hi),
            PowerDecision::Defer
        );
        let chat = PowerAwareWork::new(
            WorkDomain::Message,
            WorkPriority::Interactive,
            EnergyClass::Low,
        );
        assert!(engine().evaluate(&chat, &s, &hi).runs());
        let crit = ResourceSnapshot {
            pressure: ResourcePressure::Critical,
            now_millis: 0,
        };
        assert!(engine().evaluate(&sos(), &s, &crit).runs());
        assert!(!engine().evaluate(&chat, &s, &crit).runs());
    }

    fn arb_state() -> impl Strategy<Value = PowerState> {
        (
            0u8..6,
            0u8..5,
            0u8..5,
            0u8..5,
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(b, c, t, a, saver, met, roam, blk)| PowerState {
                battery: [
                    BatteryLevelClass::Critical,
                    BatteryLevelClass::Low,
                    BatteryLevelClass::Medium,
                    BatteryLevelClass::High,
                    BatteryLevelClass::Full,
                    BatteryLevelClass::Unknown,
                ][b as usize],
                charging: [
                    ChargingState::NotCharging,
                    ChargingState::ChargingSlow,
                    ChargingState::ChargingFast,
                    ChargingState::Full,
                    ChargingState::Unknown,
                ][c as usize],
                thermal: [
                    ThermalState::Nominal,
                    ThermalState::Elevated,
                    ThermalState::Serious,
                    ThermalState::Critical,
                    ThermalState::Unknown,
                ][t as usize],
                activity: [
                    AppActivityState::ForegroundInteractive,
                    AppActivityState::ForegroundIdle,
                    AppActivityState::BackgroundAllowed,
                    AppActivityState::BackgroundRestricted,
                    AppActivityState::SuspendedLikely,
                ][a as usize],
                saver_enabled: saver,
                metered_network: met,
                roaming: roam,
                os_background_network_blocked: blk,
            })
    }
    fn arb_work() -> impl Strategy<Value = PowerAwareWork> {
        (
            0usize..13,
            0u8..5,
            0u8..5,
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(d, p, e, durable, deferrable, user, aged)| {
                let prio = [
                    WorkPriority::Critical,
                    WorkPriority::Interactive,
                    WorkPriority::Normal,
                    WorkPriority::Bulk,
                    WorkPriority::Background,
                ][p as usize];
                let en = [
                    EnergyClass::Tiny,
                    EnergyClass::Low,
                    EnergyClass::Medium,
                    EnergyClass::High,
                    EnergyClass::VeryHigh,
                ][e as usize];
                let mut w = PowerAwareWork::new(WorkDomain::ALL[d], prio, en)
                    .with_durable(durable)
                    .with_deferrable(deferrable);
                w.user_initiated = user;
                w.aged = aged;
                w
            })
    }

    proptest! {
        /// §21: durable work is never dropped or rejected — only run or deferred.
        #[test]
        fn durable_work_is_never_lost(w in arb_work(), s in arb_state()) {
            let w = w.with_durable(true);
            let d = engine().evaluate(&w, &s, &res());
            prop_assert!(matches!(d, PowerDecision::Run | PowerDecision::RunThrottled | PowerDecision::Defer));
        }

        /// A deferral always says what it waits for.
        #[test]
        fn defer_carries_a_wait_hint(w in arb_work(), s in arb_state()) {
            let e = engine().evaluate_detailed(&w, &s, &res());
            prop_assert_eq!(e.decision == PowerDecision::Defer, e.wait.is_some());
        }

        /// §20/§96: with a blocked OS network, network work never runs in the background.
        #[test]
        fn os_block_is_absolute(w in arb_work(), mut s in arb_state()) {
            s.os_background_network_blocked = true;
            s.activity = AppActivityState::BackgroundRestricted;
            let w = PowerAwareWork { uses_network: true, ..w };
            prop_assert!(!engine().evaluate(&w, &s, &res()).runs());
        }

        /// Thermal-critical: only tiny/low critical work runs, whatever else is set.
        #[test]
        fn thermal_critical_is_absolute(w in arb_work(), mut s in arb_state()) {
            s.thermal = ThermalState::Critical;
            s.os_background_network_blocked = false;
            if engine().evaluate(&w, &s, &res()).runs() {
                prop_assert!(w.priority == WorkPriority::Critical && w.energy_class <= EnergyClass::Low);
            }
        }

        /// §97: an override never changes the profile.
        #[test]
        fn override_is_scoped_to_the_operation(w in arb_work(), s in arb_state()) {
            let e = engine();
            let before = e.profile(&s);
            let _ = e.evaluate(&w.user_initiated(), &s, &res());
            prop_assert_eq!(before, e.profile(&s));
        }

        /// Determinism (§68 "pure/deterministic").
        #[test]
        fn evaluation_is_pure(w in arb_work(), s in arb_state()) {
            prop_assert_eq!(engine().evaluate_detailed(&w, &s, &res()), engine().evaluate_detailed(&w, &s, &res()));
        }
    }
}
