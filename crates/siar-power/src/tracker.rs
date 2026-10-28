//! §83-§84: raw platform readings → a stable [`PowerState`], with an epoch
//! that advances only on **meaningful** change.
//!
//! A battery reading arrives every few seconds; almost none of them change
//! any decision. [`PowerTracker::observe`] returns `None` unless the coarse
//! state, the profile or the budget actually changed, so downstream
//! subsystems are woken only for real policy transitions (§84), and every
//! decision can carry the epoch it was made under so stale ones are ignored
//! (§83).

use serde::{Deserialize, Serialize};

use crate::budget::PowerBudget;
use crate::hysteresis::{BatteryClassifier, ThermalFilter};
use crate::policy::DefaultPowerPolicy;
use crate::profile::{PowerPolicyConfig, PowerPolicyProfile};
use crate::state::{AppActivityState, ChargingState, PowerState, ThermalState};

/// §83.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct PowerStateEpoch(pub u64);

impl PowerStateEpoch {
    /// Is a decision made under `decided` out of date?
    #[must_use]
    pub const fn is_stale(self, decided: Self) -> bool {
        decided.0 != self.0
    }
}

/// What a platform adapter reports. Unlike [`PowerState`] this may carry an
/// exact percentage — it exists only between the adapter and the tracker and
/// is never stored, logged or exported.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RawPowerReading {
    pub battery_percent: Option<u8>,
    pub charging: ChargingState,
    pub thermal: ThermalState,
    pub activity: AppActivityState,
    pub saver_enabled: bool,
    pub metered_network: bool,
    pub roaming: bool,
    pub os_background_network_blocked: bool,
}

impl Default for RawPowerReading {
    fn default() -> Self {
        Self {
            battery_percent: None,
            charging: ChargingState::Unknown,
            thermal: ThermalState::Unknown,
            activity: AppActivityState::default(),
            saver_enabled: false,
            metered_network: false,
            roaming: false,
            os_background_network_blocked: false,
        }
    }
}

/// §86: the exact percentage must never reach a log line by accident.
impl core::fmt::Debug for RawPowerReading {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawPowerReading")
            .field(
                "battery_percent",
                &self.battery_percent.map(|_| "<redacted>"),
            )
            .field("charging", &self.charging)
            .field("thermal", &self.thermal)
            .field("activity", &self.activity)
            .finish_non_exhaustive()
    }
}

/// One meaningful change (§84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub epoch: PowerStateEpoch,
    pub previous_state: PowerState,
    pub state: PowerState,
    pub previous_profile: PowerPolicyProfile,
    pub profile: PowerPolicyProfile,
    pub previous_budget: PowerBudget,
    pub budget: PowerBudget,
}

impl Transition {
    #[must_use]
    pub fn profile_changed(&self) -> bool {
        self.previous_profile != self.profile
    }

    /// External power just appeared (§77).
    #[must_use]
    pub fn charging_started(&self) -> bool {
        !self.previous_state.external_power() && self.state.external_power()
    }

    /// An unmetered network just appeared (§78).
    #[must_use]
    pub fn unmetered_appeared(&self) -> bool {
        self.previous_state.costly_network() && !self.state.costly_network()
    }

    /// The app just came to the foreground (§79).
    #[must_use]
    pub fn foregrounded(&self) -> bool {
        self.previous_state.activity.is_background() && self.state.activity.is_foreground()
    }

    /// Thermal pressure dropped (§80).
    #[must_use]
    pub fn thermal_recovered(&self) -> bool {
        self.state.thermal.severity() < self.previous_state.thermal.severity()
    }
}

#[derive(Debug, Clone)]
pub struct PowerTracker {
    battery: BatteryClassifier,
    thermal: ThermalFilter,
    policy: DefaultPowerPolicy,
    state: PowerState,
    profile: PowerPolicyProfile,
    budget: PowerBudget,
    epoch: PowerStateEpoch,
    last_raw: RawPowerReading,
}

impl PowerTracker {
    #[must_use]
    pub fn new(config: PowerPolicyConfig) -> Self {
        let policy = DefaultPowerPolicy::new(config);
        let state = PowerState::default();
        let profile = policy.profile(&state);
        let budget = policy.budget(&state);
        Self {
            battery: BatteryClassifier::new(),
            thermal: ThermalFilter::default(),
            policy,
            state,
            profile,
            budget,
            epoch: PowerStateEpoch(0),
            last_raw: RawPowerReading::default(),
        }
    }

    #[must_use]
    pub const fn state(&self) -> &PowerState {
        &self.state
    }
    #[must_use]
    pub const fn profile(&self) -> PowerPolicyProfile {
        self.profile
    }
    #[must_use]
    pub const fn budget(&self) -> &PowerBudget {
        &self.budget
    }
    #[must_use]
    pub const fn epoch(&self) -> PowerStateEpoch {
        self.epoch
    }
    #[must_use]
    pub const fn policy(&self) -> &DefaultPowerPolicy {
        &self.policy
    }

    /// Feed a platform reading at `now_ms`.
    pub fn observe(&mut self, raw: &RawPowerReading, now_ms: u64) -> Option<Transition> {
        self.last_raw = *raw;
        let state = PowerState {
            battery: self.battery.update(raw.battery_percent),
            charging: raw.charging,
            thermal: self.thermal.update(raw.thermal, now_ms),
            activity: raw.activity,
            saver_enabled: raw.saver_enabled,
            metered_network: raw.metered_network,
            roaming: raw.roaming,
            os_background_network_blocked: raw.os_background_network_blocked,
        };
        self.apply(state)
    }

    /// Re-feed the last reading so thermal recovery can proceed with no new
    /// platform event (§80). Call from a slow timer.
    pub fn tick(&mut self, now_ms: u64) -> Option<Transition> {
        let raw = self.last_raw;
        self.observe(&raw, now_ms)
    }

    /// §13/§74: enter or leave emergency mode.
    pub fn set_emergency(&mut self, active: bool) -> Option<Transition> {
        if self.policy.emergency_active == active {
            return None;
        }
        self.policy.emergency_active = active;
        let s = self.state;
        self.apply(s)
    }

    /// §69/§70: new configuration (e.g. the user changed a setting).
    pub fn set_config(&mut self, config: PowerPolicyConfig) -> Option<Transition> {
        if self.policy.config == config {
            return None;
        }
        self.policy.config = config;
        let s = self.state;
        self.apply(s)
    }

    fn apply(&mut self, state: PowerState) -> Option<Transition> {
        let profile = self.policy.profile(&state);
        let budget = PowerBudget::derive(profile, &state, &self.policy.config);
        if state == self.state && profile == self.profile && budget == self.budget {
            return None;
        }
        let t = Transition {
            epoch: PowerStateEpoch(self.epoch.0.saturating_add(1)),
            previous_state: self.state,
            state,
            previous_profile: self.profile,
            profile,
            previous_budget: self.budget,
            budget,
        };
        self.epoch = t.epoch;
        self.state = state;
        self.profile = profile;
        self.budget = budget;
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::BatteryLevelClass;

    fn raw(pct: u8, charging: ChargingState) -> RawPowerReading {
        RawPowerReading {
            battery_percent: Some(pct),
            charging,
            thermal: ThermalState::Nominal,
            ..Default::default()
        }
    }

    #[test]
    fn one_percent_wobble_emits_nothing() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        assert!(t.observe(&raw(50, ChargingState::NotCharging), 0).is_some());
        let e = t.epoch();
        for (i, p) in [49, 50, 48, 47, 49, 50].into_iter().enumerate() {
            assert!(t
                .observe(&raw(p, ChargingState::NotCharging), 1 + i as u64)
                .is_none());
        }
        assert_eq!(
            t.epoch(),
            e,
            "epoch advances only on meaningful change (§84)"
        );
    }

    #[test]
    fn crossing_into_low_advances_epoch_and_flips_to_saver_once() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        t.observe(&raw(30, ChargingState::NotCharging), 0);
        let tr = t
            .observe(&raw(19, ChargingState::NotCharging), 1)
            .expect("class change");
        assert_eq!(tr.state.battery, BatteryLevelClass::Low);
        assert!(tr.profile_changed());
        assert_eq!(tr.profile, PowerPolicyProfile::Saver);
        assert!(
            t.observe(&raw(21, ChargingState::NotCharging), 2).is_none(),
            "hysteresis holds Low"
        );
        assert!(t.observe(&raw(18, ChargingState::NotCharging), 3).is_none());
    }

    #[test]
    fn charging_start_is_reported_as_a_trigger() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        t.observe(&raw(15, ChargingState::NotCharging), 0);
        let tr = t.observe(&raw(15, ChargingState::ChargingFast), 1).unwrap();
        assert!(tr.charging_started());
        assert!(!tr.foregrounded());
    }

    #[test]
    fn thermal_recovers_on_tick_without_new_platform_events() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        let hot = RawPowerReading {
            thermal: ThermalState::Critical,
            charging: ChargingState::Full,
            ..Default::default()
        };
        t.observe(&hot, 0);
        let cool = RawPowerReading {
            thermal: ThermalState::Nominal,
            ..hot
        };
        assert!(t.observe(&cool, 1_000).is_none(), "still within dwell");
        let tr = t.tick(31_000).expect("one step per dwell");
        assert_eq!(tr.state.thermal, ThermalState::Serious);
        assert!(tr.thermal_recovered());
        assert!(t.tick(40_000).is_none());
        assert_eq!(
            t.tick(61_500).unwrap().state.thermal,
            ThermalState::Elevated
        );
        assert_eq!(t.tick(92_000).unwrap().state.thermal, ThermalState::Nominal);
    }

    #[test]
    fn emergency_toggle_changes_profile_and_is_idempotent() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        let tr = t.set_emergency(true).unwrap();
        assert_eq!(tr.profile, PowerPolicyProfile::Emergency);
        assert!(t.set_emergency(true).is_none());
        assert_eq!(
            t.set_emergency(false).unwrap().previous_profile,
            PowerPolicyProfile::Emergency
        );
    }

    #[test]
    fn debug_never_prints_the_percentage() {
        let r = raw(37, ChargingState::NotCharging);
        let s = format!("{r:?}");
        assert!(!s.contains("37"), "{s}");
        assert!(s.contains("redacted"));
    }

    #[test]
    fn stale_epoch_detection() {
        let mut t = PowerTracker::new(PowerPolicyConfig::default());
        let decided = t.epoch();
        t.observe(&raw(10, ChargingState::NotCharging), 0);
        assert!(t.epoch().is_stale(decided));
        assert!(!t.epoch().is_stale(t.epoch()));
    }
}
