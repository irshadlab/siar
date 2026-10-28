//! §4-§8, §15: the neutral power inputs and the [`PowerState`] snapshot.
//!
//! This is the **one** place in the workspace that defines what "battery
//! level", "charging" or "thermal pressure" mean. Before Part 13 landed there
//! were three independent versions (`siar-routing-policy::platform`,
//! `siar-crash-recovery::startup_policy`, and ad-hoc booleans in
//! `siar-resource-limits::modes`); those now convert from / re-export these.
//!
//! Everything here is deliberately *coarse* (§5, §86): classes, never exact
//! percentages, so nothing in [`PowerState`] can leak a precise battery
//! reading to a peer, a log line or telemetry by construction.

use serde::{Deserialize, Serialize};

/// §5. Declared in the spec's order, so `Unknown` sorts last. **Do not
/// compare `Unknown` numerically** — use [`BatteryLevelClass::effective`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum BatteryLevelClass {
    Critical,
    Low,
    Medium,
    High,
    Full,
    #[default]
    Unknown,
}

impl BatteryLevelClass {
    /// `Unknown` is treated as `Medium`: with no information we neither
    /// panic-throttle nor assume plenty (a desktop with no battery reports
    /// external power through [`ChargingState`], not through this).
    #[must_use]
    pub const fn effective(self) -> Self {
        match self {
            Self::Unknown => Self::Medium,
            other => other,
        }
    }

    /// `Critical` or `Low`, after resolving `Unknown`.
    #[must_use]
    pub const fn is_low(self) -> bool {
        matches!(self.effective(), Self::Critical | Self::Low)
    }

    #[must_use]
    pub const fn is_critical(self) -> bool {
        matches!(self, Self::Critical)
    }
}

/// §6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ChargingState {
    NotCharging,
    ChargingSlow,
    ChargingFast,
    /// On external power with a full battery — **also what a battery-less
    /// desktop or server reports**, since "plugged in" is the only fact the
    /// policy needs.
    Full,
    #[default]
    Unknown,
}

impl ChargingState {
    /// Is there external power right now? `Unknown` is `false`: never assume
    /// mains when the platform could not say.
    #[must_use]
    pub const fn on_external_power(self) -> bool {
        matches!(self, Self::ChargingSlow | Self::ChargingFast | Self::Full)
    }
}

/// §7. Ordered by severity except `Unknown`, which sorts last; use
/// [`ThermalState::severity`] for comparisons.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum ThermalState {
    Nominal,
    Elevated,
    Serious,
    Critical,
    #[default]
    Unknown,
}

impl ThermalState {
    /// 0 (Nominal / Unknown) … 3 (Critical). `Unknown` counts as no
    /// pressure: many desktops have no usable sensor and must not be
    /// throttled for that.
    #[must_use]
    pub const fn severity(self) -> u8 {
        match self {
            Self::Nominal | Self::Unknown => 0,
            Self::Elevated => 1,
            Self::Serious => 2,
            Self::Critical => 3,
        }
    }

    #[must_use]
    pub const fn from_severity(s: u8) -> Self {
        match s {
            0 => Self::Nominal,
            1 => Self::Elevated,
            2 => Self::Serious,
            _ => Self::Critical,
        }
    }
}

/// §8. Platform adapters map Android / iOS / desktop behaviour onto this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum AppActivityState {
    ForegroundInteractive,
    #[default]
    ForegroundIdle,
    BackgroundAllowed,
    BackgroundRestricted,
    SuspendedLikely,
}

impl AppActivityState {
    #[must_use]
    pub const fn is_foreground(self) -> bool {
        matches!(self, Self::ForegroundInteractive | Self::ForegroundIdle)
    }

    #[must_use]
    pub const fn is_background(self) -> bool {
        !self.is_foreground()
    }

    /// The OS is likely to stop or freeze us: do not start long work.
    #[must_use]
    pub const fn is_restricted(self) -> bool {
        matches!(self, Self::BackgroundRestricted | Self::SuspendedLikely)
    }
}

/// §15 plus the two extra inputs §4 lists (roaming) and §72 requires (an OS
/// hard restriction the policy may never override).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PowerState {
    pub battery: BatteryLevelClass,
    pub charging: ChargingState,
    pub thermal: ThermalState,
    pub activity: AppActivityState,
    pub saver_enabled: bool,
    pub metered_network: bool,
    pub roaming: bool,
    /// §72: the OS says background network is unavailable. The policy must
    /// defer; it cannot override (not even for critical work).
    pub os_background_network_blocked: bool,
}

impl PowerState {
    #[must_use]
    pub const fn external_power(&self) -> bool {
        self.charging.on_external_power()
    }

    /// Running on battery with little left (Low/Critical, not charging).
    #[must_use]
    pub const fn battery_constrained(&self) -> bool {
        !self.external_power() && self.battery.is_low()
    }

    /// Critical battery and no charger — §29's "critical reserve" regime.
    #[must_use]
    pub const fn in_critical_reserve(&self) -> bool {
        !self.external_power() && self.battery.is_critical()
    }

    /// Network cost is something the user pays for (metered or roaming).
    #[must_use]
    pub const fn costly_network(&self) -> bool {
        self.metered_network || self.roaming
    }
}

/// §87/§86: what a *peer* may learn about this device. Three buckets, no
/// charging timestamp, no thermal history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeerPowerHint {
    /// Low battery, saver or thermal pressure: ask it for little.
    Constrained,
    Normal,
    /// External power and cool: a good relay candidate (§46, §88).
    Plentiful,
}

impl PeerPowerHint {
    #[must_use]
    pub const fn from_state(s: &PowerState) -> Self {
        if s.saver_enabled || s.battery_constrained() || s.thermal.severity() >= 2 {
            Self::Constrained
        } else if s.external_power() && s.thermal.severity() == 0 {
            Self::Plentiful
        } else {
            Self::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_not_low_and_not_mains() {
        assert!(!BatteryLevelClass::Unknown.is_low());
        assert!(!ChargingState::Unknown.on_external_power());
        assert_eq!(ThermalState::Unknown.severity(), 0);
    }

    #[test]
    fn default_state_never_looks_constrained() {
        let s = PowerState::default();
        assert!(!s.battery_constrained());
        assert!(!s.in_critical_reserve());
        assert_eq!(PeerPowerHint::from_state(&s), PeerPowerHint::Normal);
    }

    #[test]
    fn peer_hint_buckets() {
        let mut s = PowerState {
            charging: ChargingState::ChargingFast,
            thermal: ThermalState::Nominal,
            ..PowerState::default()
        };
        assert_eq!(PeerPowerHint::from_state(&s), PeerPowerHint::Plentiful);
        s.thermal = ThermalState::Serious;
        assert_eq!(PeerPowerHint::from_state(&s), PeerPowerHint::Constrained);
        s = PowerState {
            battery: BatteryLevelClass::Low,
            ..PowerState::default()
        };
        assert_eq!(PeerPowerHint::from_state(&s), PeerPowerHint::Constrained);
    }

    #[test]
    fn severity_round_trips() {
        for s in 0..=3u8 {
            assert_eq!(ThermalState::from_severity(s).severity(), s);
        }
    }
}
