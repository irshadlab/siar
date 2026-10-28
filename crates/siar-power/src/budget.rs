//! §16-§19, §140: converting a profile + live state into an execution
//! budget, and the per-domain [`EffectiveLimits`] view features consume.
//!
//! Construction order mirrors `siar-resource-limits::modes::apply_modes`:
//! profile base, then **restrictions only** — activity, battery, thermal,
//! OS. A restriction always beats a boost, so no input combination can make
//! the budget *larger* than the profile's base (property-tested).

use serde::{Deserialize, Serialize};

use crate::profile::{PowerPolicyConfig, PowerPolicyProfile};
use crate::state::{AppActivityState, PowerState, ThermalState};
use crate::work::WorkDomain;

/// §17. Declared low → high so `Ord` means "more".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CpuBudgetClass {
    Minimal,
    Low,
    Normal,
    High,
}

/// §18.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RadioBudgetClass {
    Minimal,
    Opportunistic,
    Normal,
    Aggressive,
}

/// §19. `CriticalOnly` < `Deferred` < `Limited` < `Normal`.
///
/// * `CriticalOnly` — only critical-priority work runs.
/// * `Deferred` — small durable work runs; bulk waits for better conditions.
/// * `Limited` — bulk runs, throttled.
/// * `Normal` — everything runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum BackgroundBudgetClass {
    CriticalOnly,
    Deferred,
    Limited,
    Normal,
}

/// §16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerBudget {
    pub cpu_class: CpuBudgetClass,
    pub radio_class: RadioBudgetClass,
    pub background_class: BackgroundBudgetClass,
    pub max_parallel_bulk: u8,
    pub multipath_allowed: bool,
    pub aggressive_discovery_allowed: bool,
    /// §72. Carried here so every consumer sees the same hard fact.
    pub network_blocked_by_os: bool,
}

impl PowerBudget {
    /// The profile's unrestricted base (§10-§14).
    #[must_use]
    pub const fn base(profile: PowerPolicyProfile) -> Self {
        use BackgroundBudgetClass as B;
        use CpuBudgetClass as C;
        use PowerPolicyProfile as P;
        use RadioBudgetClass as R;
        let (cpu, radio, bg, bulk, multipath, discovery) = match profile {
            P::Performance => (C::High, R::Aggressive, B::Normal, 8, true, true),
            P::Balanced => (C::Normal, R::Normal, B::Limited, 3, true, false),
            P::Saver => (C::Low, R::Opportunistic, B::Deferred, 1, false, false),
            // §13: reachability and critical relay, not "max power".
            P::Emergency => (C::Normal, R::Normal, B::CriticalOnly, 0, true, true),
            P::AlwaysOnNode => (C::High, R::Aggressive, B::Normal, 8, true, true),
        };
        Self {
            cpu_class: cpu,
            radio_class: radio,
            background_class: bg,
            max_parallel_bulk: bulk,
            multipath_allowed: multipath,
            aggressive_discovery_allowed: discovery,
            network_blocked_by_os: false,
        }
    }

    /// Derive the effective budget. Pure and total.
    #[must_use]
    pub fn derive(
        profile: PowerPolicyProfile,
        state: &PowerState,
        config: &PowerPolicyConfig,
    ) -> Self {
        let mut b = Self::base(profile);
        let emergency = profile == PowerPolicyProfile::Emergency;

        // Config.
        if !config.allow_background_bulk {
            b.background_class = b.background_class.min(BackgroundBudgetClass::Deferred);
        }
        if !state.external_power() && !config.multipath_on_battery && !emergency {
            b.multipath_allowed = false;
        }

        // Activity (§8): the OS may freeze us; do not start heavy work.
        match state.activity {
            AppActivityState::ForegroundInteractive | AppActivityState::ForegroundIdle => {}
            AppActivityState::BackgroundAllowed => {
                if !matches!(
                    profile,
                    PowerPolicyProfile::Emergency | PowerPolicyProfile::AlwaysOnNode
                ) {
                    b.aggressive_discovery_allowed = false;
                }
            }
            AppActivityState::BackgroundRestricted | AppActivityState::SuspendedLikely => {
                b.background_class = b.background_class.min(BackgroundBudgetClass::Deferred);
                b.radio_class = b.radio_class.min(RadioBudgetClass::Opportunistic);
                b.aggressive_discovery_allowed = false;
                b.max_parallel_bulk = b.max_parallel_bulk.min(1);
                b.cpu_class = b.cpu_class.min(CpuBudgetClass::Low);
            }
        }

        // Battery (§29): unplugged and low.
        if !state.external_power() {
            if state.battery.is_critical() {
                b.background_class = BackgroundBudgetClass::CriticalOnly;
                b.max_parallel_bulk = 0;
                b.cpu_class = b.cpu_class.min(CpuBudgetClass::Low);
                b.radio_class = b.radio_class.min(RadioBudgetClass::Normal);
                b.multipath_allowed &= emergency;
                b.aggressive_discovery_allowed &= emergency;
            } else if state.battery.is_low() {
                b.background_class = b.background_class.min(BackgroundBudgetClass::Deferred);
                b.max_parallel_bulk = b.max_parallel_bulk.min(1);
                b.multipath_allowed &= emergency;
                b.aggressive_discovery_allowed &= emergency;
            }
        }

        // Thermal (§7, §20): hard safety. Strongest cap, applied last.
        match state.thermal {
            ThermalState::Nominal | ThermalState::Unknown => {}
            ThermalState::Elevated => {
                b.cpu_class = b.cpu_class.min(CpuBudgetClass::Normal);
                b.max_parallel_bulk = b.max_parallel_bulk.min(2);
            }
            ThermalState::Serious => {
                b.cpu_class = b.cpu_class.min(CpuBudgetClass::Low);
                b.max_parallel_bulk = b.max_parallel_bulk.min(1);
                b.multipath_allowed = false;
                b.aggressive_discovery_allowed = false;
                b.radio_class = b.radio_class.min(RadioBudgetClass::Normal);
                b.background_class = b.background_class.min(BackgroundBudgetClass::Deferred);
            }
            ThermalState::Critical => {
                b.cpu_class = CpuBudgetClass::Minimal;
                b.max_parallel_bulk = 0;
                b.multipath_allowed = false;
                b.aggressive_discovery_allowed = false;
                b.radio_class = b.radio_class.min(RadioBudgetClass::Opportunistic);
                b.background_class = BackgroundBudgetClass::CriticalOnly;
            }
        }

        // OS hard restriction (§72).
        b.network_blocked_by_os =
            state.os_background_network_blocked && state.activity.is_background();
        if b.network_blocked_by_os {
            b.radio_class = RadioBudgetClass::Minimal;
            b.aggressive_discovery_allowed = false;
            b.multipath_allowed = false;
        }
        b
    }

    /// §140: the limits one feature domain should plan against.
    #[must_use]
    pub fn effective_limits(&self, domain: WorkDomain) -> EffectiveLimits {
        let bulk_domain = domain.is_bulk();
        let parallelism = if bulk_domain {
            self.max_parallel_bulk
        } else {
            1
        };
        let background_allowed = match domain {
            d if d.is_bulk() => self.background_class >= BackgroundBudgetClass::Limited,
            _ => self.background_class >= BackgroundBudgetClass::Deferred,
        };
        EffectiveLimits {
            parallelism,
            background_allowed,
            multipath_allowed: self.multipath_allowed,
            cpu_class: self.cpu_class,
        }
    }
}

/// §140.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveLimits {
    pub parallelism: u8,
    pub background_allowed: bool,
    pub multipath_allowed: bool,
    pub cpu_class: CpuBudgetClass,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::*;
    use proptest::prelude::*;

    fn arb_battery() -> impl Strategy<Value = BatteryLevelClass> {
        prop_oneof![
            Just(BatteryLevelClass::Critical),
            Just(BatteryLevelClass::Low),
            Just(BatteryLevelClass::Medium),
            Just(BatteryLevelClass::High),
            Just(BatteryLevelClass::Full),
            Just(BatteryLevelClass::Unknown)
        ]
    }
    fn arb_charging() -> impl Strategy<Value = ChargingState> {
        prop_oneof![
            Just(ChargingState::NotCharging),
            Just(ChargingState::ChargingSlow),
            Just(ChargingState::ChargingFast),
            Just(ChargingState::Full),
            Just(ChargingState::Unknown)
        ]
    }
    fn arb_thermal() -> impl Strategy<Value = ThermalState> {
        prop_oneof![
            Just(ThermalState::Nominal),
            Just(ThermalState::Elevated),
            Just(ThermalState::Serious),
            Just(ThermalState::Critical),
            Just(ThermalState::Unknown)
        ]
    }
    fn arb_activity() -> impl Strategy<Value = AppActivityState> {
        prop_oneof![
            Just(AppActivityState::ForegroundInteractive),
            Just(AppActivityState::ForegroundIdle),
            Just(AppActivityState::BackgroundAllowed),
            Just(AppActivityState::BackgroundRestricted),
            Just(AppActivityState::SuspendedLikely)
        ]
    }
    fn arb_profile() -> impl Strategy<Value = PowerPolicyProfile> {
        prop_oneof![
            Just(PowerPolicyProfile::Performance),
            Just(PowerPolicyProfile::Balanced),
            Just(PowerPolicyProfile::Saver),
            Just(PowerPolicyProfile::Emergency),
            Just(PowerPolicyProfile::AlwaysOnNode)
        ]
    }
    prop_compose! {
        fn arb_state()(battery in arb_battery(), charging in arb_charging(), thermal in arb_thermal(),
                       activity in arb_activity(), saver in any::<bool>(), metered in any::<bool>(),
                       roaming in any::<bool>(), blocked in any::<bool>()) -> PowerState {
            PowerState { battery, charging, thermal, activity, saver_enabled: saver, metered_network: metered,
                         roaming, os_background_network_blocked: blocked }
        }
    }

    proptest! {
        /// A restriction always beats a boost: no state grows the budget past the profile's base.
        #[test]
        fn never_exceeds_profile_base(p in arb_profile(), s in arb_state()) {
            let base = PowerBudget::base(p);
            let b = PowerBudget::derive(p, &s, &PowerPolicyConfig::default());
            prop_assert!(b.cpu_class <= base.cpu_class);
            prop_assert!(b.radio_class <= base.radio_class);
            prop_assert!(b.background_class <= base.background_class);
            prop_assert!(b.max_parallel_bulk <= base.max_parallel_bulk);
            prop_assert!(!b.multipath_allowed || base.multipath_allowed);
            prop_assert!(!b.aggressive_discovery_allowed || base.aggressive_discovery_allowed);
        }

        /// §29/§7: critical thermal or critical unplugged battery ⇒ bulk is off.
        #[test]
        fn critical_conditions_stop_bulk(p in arb_profile(), s in arb_state()) {
            let b = PowerBudget::derive(p, &s, &PowerPolicyConfig::default());
            if s.thermal == ThermalState::Critical || s.in_critical_reserve() {
                prop_assert_eq!(b.max_parallel_bulk, 0);
                prop_assert_eq!(b.background_class, BackgroundBudgetClass::CriticalOnly);
            }
        }

        /// Deterministic: same inputs, same budget.
        #[test]
        fn deterministic(p in arb_profile(), s in arb_state()) {
            let c = PowerPolicyConfig::default();
            prop_assert_eq!(PowerBudget::derive(p, &s, &c), PowerBudget::derive(p, &s, &c));
        }

        /// §23/§140: "more battery never means a smaller bulk budget" (monotone in thermal).
        #[test]
        fn cooler_never_shrinks_bulk(p in arb_profile(), s in arb_state()) {
            let c = PowerPolicyConfig::default();
            let mut cool = s;
            cool.thermal = ThermalState::Nominal;
            prop_assert!(PowerBudget::derive(p, &cool, &c).max_parallel_bulk >= PowerBudget::derive(p, &s, &c).max_parallel_bulk);
        }
    }

    #[test]
    fn saver_collapses_multipath_and_serious_thermal_does_too() {
        let s = PowerState {
            charging: ChargingState::Full,
            ..PowerState::default()
        };
        let c = PowerPolicyConfig::default();
        assert!(PowerBudget::derive(PowerPolicyProfile::Balanced, &s, &c).multipath_allowed);
        assert!(!PowerBudget::derive(PowerPolicyProfile::Saver, &s, &c).multipath_allowed);
        let hot = PowerState {
            thermal: ThermalState::Serious,
            ..s
        };
        assert!(!PowerBudget::derive(PowerPolicyProfile::Performance, &hot, &c).multipath_allowed);
    }

    #[test]
    fn multipath_on_battery_needs_explicit_config() {
        let s = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::High,
            ..PowerState::default()
        };
        let mut c = PowerPolicyConfig::default();
        assert!(!PowerBudget::derive(PowerPolicyProfile::Balanced, &s, &c).multipath_allowed);
        c.multipath_on_battery = true;
        assert!(PowerBudget::derive(PowerPolicyProfile::Balanced, &s, &c).multipath_allowed);
    }

    #[test]
    fn emergency_keeps_discovery_and_multipath_on_low_battery() {
        let s = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::Low,
            ..PowerState::default()
        };
        let b = PowerBudget::derive(
            PowerPolicyProfile::Emergency,
            &s,
            &PowerPolicyConfig::default(),
        );
        assert!(b.aggressive_discovery_allowed && b.multipath_allowed);
        assert_eq!(b.background_class, BackgroundBudgetClass::CriticalOnly);
    }

    #[test]
    fn os_block_only_applies_in_background() {
        let mut s = PowerState {
            os_background_network_blocked: true,
            activity: AppActivityState::ForegroundInteractive,
            ..PowerState::default()
        };
        let c = PowerPolicyConfig::default();
        assert!(!PowerBudget::derive(PowerPolicyProfile::Balanced, &s, &c).network_blocked_by_os);
        s.activity = AppActivityState::BackgroundRestricted;
        assert!(PowerBudget::derive(PowerPolicyProfile::Balanced, &s, &c).network_blocked_by_os);
    }
}
