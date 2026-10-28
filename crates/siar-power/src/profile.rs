//! §9-§14, §69-§70: profiles, the policy configuration and user preferences.

use serde::{Deserialize, Serialize};

use crate::state::PowerState;

/// §9.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerPolicyProfile {
    Performance,
    Balanced,
    Saver,
    Emergency,
    AlwaysOnNode,
}

/// What kind of machine this is. A *deployment fact*, not a transient mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Deployment {
    /// Phone / tablet: battery matters, OS restricts background work.
    #[default]
    Mobile,
    /// Laptop or desktop: often on mains, usually foregrounded.
    Desktop,
    /// §14/§66/§67: relay, Raspberry Pi, server, vehicle gateway.
    AlwaysOn,
}

/// §69, extended with what the layers in §73 need to be expressible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerPolicyConfig {
    pub profile_hint: Option<PowerPolicyProfile>,
    pub deployment: Deployment,
    pub allow_background_bulk: bool,
    pub allow_metered_bulk: bool,
    pub multipath_on_battery: bool,
    /// §74: emergency *may* override saver. It never silently overrides
    /// `allow_metered_bulk = false` unless [`Self::emergency_consent_metered`].
    pub emergency_override: bool,
    /// §74: explicit, product-defined consent to spend mobile data in an
    /// emergency. Default `false`.
    pub emergency_consent_metered: bool,
    /// §71: messenger-style "small messages are always immediate".
    pub small_messages_immediate: bool,
    /// §71: ERP-style "defer these domains while on battery".
    pub defer_on_battery: crate::work::DomainMask,
}

impl Default for PowerPolicyConfig {
    fn default() -> Self {
        Self {
            profile_hint: None,
            deployment: Deployment::Mobile,
            allow_background_bulk: true,
            allow_metered_bulk: false,
            multipath_on_battery: false,
            emergency_override: true,
            emergency_consent_metered: false,
            small_messages_immediate: true,
            defer_on_battery: crate::work::DomainMask::EMPTY,
        }
    }
}

/// §70: the toggles a settings screen actually shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPowerPrefs {
    pub battery_saver: bool,
    pub allow_background_transfers: bool,
    pub use_mobile_data_for_files: bool,
    pub allow_emergency_relay: bool,
    /// "Keep device awake for transfers" — an explicit Performance request.
    pub prefer_performance: bool,
}

impl Default for UserPowerPrefs {
    fn default() -> Self {
        Self {
            battery_saver: false,
            allow_background_transfers: true,
            use_mobile_data_for_files: false,
            allow_emergency_relay: true,
            prefer_performance: false,
        }
    }
}

impl PowerPolicyConfig {
    /// Fold user preferences into a config. Preferences only ever *narrow*
    /// what the deployment allows; they never grant more than `self`.
    #[must_use]
    pub fn with_prefs(mut self, p: &UserPowerPrefs) -> Self {
        self.allow_background_bulk &= p.allow_background_transfers;
        self.allow_metered_bulk = self.allow_metered_bulk || p.use_mobile_data_for_files;
        self.emergency_override &= p.allow_emergency_relay;
        if p.prefer_performance {
            self.profile_hint = Some(PowerPolicyProfile::Performance);
        } else if p.battery_saver {
            self.profile_hint = Some(PowerPolicyProfile::Saver);
        }
        self
    }
}

/// Pick the profile for the current inputs (§9-§14).
///
/// Order is the §73 layering reduced to profile choice: deployment fact,
/// emergency, explicit user hint, then conditions. Thermal pressure at
/// `Serious` or worse forces `Saver` even over a user's Performance request —
/// hard safety beats preference (§20, §96).
#[must_use]
pub fn select_profile(
    state: &PowerState,
    config: &PowerPolicyConfig,
    emergency_active: bool,
) -> PowerPolicyProfile {
    use PowerPolicyProfile as P;

    let hot = state.thermal.severity() >= 2;

    if emergency_active && config.emergency_override {
        return P::Emergency;
    }
    // §67: an always-on node on mains. On UPS/battery (charging explicitly
    // NotCharging) it falls through to ordinary battery logic instead.
    if config.deployment == Deployment::AlwaysOn
        && (state.external_power() || state.charging == crate::state::ChargingState::Unknown)
    {
        return if hot { P::Saver } else { P::AlwaysOnNode };
    }
    if hot {
        return P::Saver;
    }
    match config.profile_hint {
        Some(P::Performance) => return P::Performance,
        Some(P::Saver) => return P::Saver,
        _ => {}
    }
    if state.saver_enabled || state.battery_constrained() {
        return P::Saver;
    }
    if state.external_power() && state.thermal.severity() == 0 {
        return P::Performance;
    }
    P::Balanced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::*;

    fn st() -> PowerState {
        PowerState::default()
    }

    #[test]
    fn defaults_to_balanced() {
        assert_eq!(
            select_profile(&st(), &PowerPolicyConfig::default(), false),
            PowerPolicyProfile::Balanced
        );
    }

    #[test]
    fn charging_cool_device_is_performance_but_hot_is_not() {
        let mut s = PowerState {
            charging: ChargingState::ChargingFast,
            thermal: ThermalState::Nominal,
            ..st()
        };
        let c = PowerPolicyConfig::default();
        assert_eq!(
            select_profile(&s, &c, false),
            PowerPolicyProfile::Performance
        );
        s.thermal = ThermalState::Elevated;
        assert_eq!(select_profile(&s, &c, false), PowerPolicyProfile::Balanced);
        s.thermal = ThermalState::Serious;
        assert_eq!(select_profile(&s, &c, false), PowerPolicyProfile::Saver);
    }

    #[test]
    fn low_battery_unplugged_is_saver_plugged_is_not() {
        let mut s = PowerState {
            battery: BatteryLevelClass::Low,
            charging: ChargingState::NotCharging,
            ..st()
        };
        let c = PowerPolicyConfig::default();
        assert_eq!(select_profile(&s, &c, false), PowerPolicyProfile::Saver);
        s.charging = ChargingState::ChargingSlow;
        assert_ne!(select_profile(&s, &c, false), PowerPolicyProfile::Saver);
    }

    #[test]
    fn emergency_needs_both_activity_and_permission() {
        let mut c = PowerPolicyConfig::default();
        assert_eq!(
            select_profile(&st(), &c, true),
            PowerPolicyProfile::Emergency
        );
        c.emergency_override = false;
        assert_ne!(
            select_profile(&st(), &c, true),
            PowerPolicyProfile::Emergency
        );
    }

    #[test]
    fn user_performance_request_loses_to_thermal_safety() {
        let c = PowerPolicyConfig {
            profile_hint: Some(PowerPolicyProfile::Performance),
            ..Default::default()
        };
        let s = PowerState {
            thermal: ThermalState::Critical,
            ..st()
        };
        assert_eq!(select_profile(&s, &c, false), PowerPolicyProfile::Saver);
        assert_eq!(
            select_profile(&st(), &c, false),
            PowerPolicyProfile::Performance
        );
    }

    #[test]
    fn always_on_node_but_ups_on_battery_is_not() {
        let c = PowerPolicyConfig {
            deployment: Deployment::AlwaysOn,
            ..Default::default()
        };
        let mains = PowerState {
            charging: ChargingState::Full,
            ..st()
        };
        assert_eq!(
            select_profile(&mains, &c, false),
            PowerPolicyProfile::AlwaysOnNode
        );
        let ups = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::Low,
            ..st()
        };
        assert_eq!(select_profile(&ups, &c, false), PowerPolicyProfile::Saver);
    }

    #[test]
    fn prefs_only_narrow() {
        let base = PowerPolicyConfig {
            allow_background_bulk: false,
            ..Default::default()
        };
        let p = UserPowerPrefs {
            allow_background_transfers: true,
            ..Default::default()
        };
        assert!(
            !base.with_prefs(&p).allow_background_bulk,
            "a pref cannot re-enable what the deployment forbids"
        );
        let saver = UserPowerPrefs {
            battery_saver: true,
            ..Default::default()
        };
        assert_eq!(
            PowerPolicyConfig::default().with_prefs(&saver).profile_hint,
            Some(PowerPolicyProfile::Saver)
        );
    }
}
