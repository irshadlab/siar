//! §34-§38, §91: discovery duty cycling and transport-upgrade cost.
//!
//! Part 14 will expose one neutral discovery scheduler; this module is the
//! battery side of that contract: it says *how often*, *for how long* and
//! *how far to escalate*. It never starts a scan itself.

use serde::{Deserialize, Serialize};

use crate::budget::{PowerBudget, RadioBudgetClass};
use crate::profile::PowerPolicyProfile;
use crate::state::{AppActivityState, PowerState};

/// §37: BLE is the low-energy bootstrap; Wi-Fi Direct / Aware only when the
/// data volume justifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Escalation {
    BleOnly,
    AllowWifiUpgrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryPlan {
    /// Scanning is off entirely (radio budget `Minimal`).
    pub enabled: bool,
    /// Start of one scan to start of the next.
    pub interval_ms: u64,
    pub scan_ms: u64,
    pub escalation: Escalation,
    /// §35 "Emergency: frequent but bounded".
    pub max_scans_per_hour: Option<u32>,
}

impl DiscoveryPlan {
    /// Share of time the radio is scanning, 0-100.
    #[must_use]
    pub const fn duty_percent(&self) -> u8 {
        if !self.enabled || self.interval_ms == 0 {
            return 0;
        }
        let d = self.scan_ms.saturating_mul(100) / self.interval_ms;
        if d > 100 {
            100
        } else {
            d as u8
        }
    }
}

const S: u64 = 1_000;
const MIN: u64 = 60 * S;

/// §35's table. `(interval, scan)` per situation.
#[must_use]
pub fn plan(
    profile: PowerPolicyProfile,
    state: &PowerState,
    budget: &PowerBudget,
) -> DiscoveryPlan {
    use PowerPolicyProfile as P;

    let enabled = budget.radio_class > RadioBudgetClass::Minimal;
    let foreground = matches!(
        state.activity,
        AppActivityState::ForegroundInteractive | AppActivityState::ForegroundIdle
    );

    let (mut interval, scan, cap) = match profile {
        P::AlwaysOnNode => (10 * S, 10 * S, None), // continuous
        P::Emergency => (15 * S, 5 * S, Some(120)),
        P::Saver => (10 * MIN, 3 * S, None),
        P::Performance => {
            if foreground {
                (5 * S, 4 * S, None)
            } else {
                (30 * S, 5 * S, None)
            }
        }
        P::Balanced => {
            if foreground {
                (10 * S, 4 * S, None)
            } else {
                (MIN, 5 * S, None)
            }
        }
    };

    // Conditions stretch the interval; they never shorten it.
    if !budget.aggressive_discovery_allowed && matches!(profile, P::Performance) {
        interval = interval.max(MIN);
    }
    if state.in_critical_reserve() && profile != P::Emergency {
        interval = interval.saturating_mul(4);
    }
    if state.thermal.severity() >= 2 {
        interval = interval.saturating_mul(2);
    }

    let escalation = if budget.radio_class >= RadioBudgetClass::Normal && profile != P::Saver {
        Escalation::AllowWifiUpgrade
    } else {
        Escalation::BleOnly
    };

    DiscoveryPlan {
        enabled,
        interval_ms: interval.max(scan),
        scan_ms: scan.min(interval.max(scan)),
        escalation,
        max_scans_per_hour: cap,
    }
}

/// §38: is a Wi-Fi upgrade worth its wake-up cost for this payload?
#[must_use]
pub fn worth_upgrading(
    payload_bytes: u64,
    profile: PowerPolicyProfile,
    plan: &DiscoveryPlan,
) -> bool {
    if plan.escalation == Escalation::BleOnly {
        return false;
    }
    let threshold: u64 = match profile {
        PowerPolicyProfile::Performance | PowerPolicyProfile::AlwaysOnNode => 64 * 1024,
        PowerPolicyProfile::Balanced => 256 * 1024,
        PowerPolicyProfile::Emergency => 1024 * 1024,
        PowerPolicyProfile::Saver => u64::MAX,
    };
    payload_bytes >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::PowerPolicyConfig;
    use crate::state::*;

    fn plan_for(profile: PowerPolicyProfile, state: PowerState) -> DiscoveryPlan {
        plan(
            profile,
            &state,
            &PowerBudget::derive(profile, &state, &PowerPolicyConfig::default()),
        )
    }
    fn fg() -> PowerState {
        PowerState {
            activity: AppActivityState::ForegroundInteractive,
            charging: ChargingState::Full,
            ..Default::default()
        }
    }

    #[test]
    fn saver_scans_rarely_foreground_balanced_scans_often() {
        let saver = plan_for(PowerPolicyProfile::Saver, fg());
        let bal = plan_for(PowerPolicyProfile::Balanced, fg());
        assert!(saver.interval_ms > bal.interval_ms * 10);
        assert!(saver.duty_percent() < bal.duty_percent());
    }

    #[test]
    fn background_balanced_is_periodic_not_continuous() {
        let bg = PowerState {
            activity: AppActivityState::BackgroundAllowed,
            ..fg()
        };
        let p = plan_for(PowerPolicyProfile::Balanced, bg);
        assert!(p.duty_percent() < 15);
        assert!(plan_for(PowerPolicyProfile::Balanced, fg()).duty_percent() > p.duty_percent());
    }

    #[test]
    fn emergency_is_frequent_but_bounded() {
        let p = plan_for(
            PowerPolicyProfile::Emergency,
            PowerState {
                charging: ChargingState::NotCharging,
                battery: BatteryLevelClass::Low,
                ..Default::default()
            },
        );
        assert!(p.interval_ms <= 15_000);
        assert_eq!(p.max_scans_per_hour, Some(120));
        assert!(p.enabled);
    }

    #[test]
    fn always_on_node_is_continuous() {
        assert_eq!(
            plan_for(PowerPolicyProfile::AlwaysOnNode, fg()).duty_percent(),
            100
        );
    }

    #[test]
    fn conditions_only_stretch_never_shorten() {
        let base = plan_for(PowerPolicyProfile::Balanced, fg());
        let hot = plan_for(
            PowerPolicyProfile::Balanced,
            PowerState {
                thermal: ThermalState::Serious,
                ..fg()
            },
        );
        let dying = plan_for(
            PowerPolicyProfile::Balanced,
            PowerState {
                battery: BatteryLevelClass::Critical,
                charging: ChargingState::NotCharging,
                ..fg()
            },
        );
        assert!(hot.interval_ms >= base.interval_ms);
        assert!(dying.interval_ms >= base.interval_ms);
    }

    #[test]
    fn os_blocked_radio_disables_scanning() {
        let s = PowerState {
            os_background_network_blocked: true,
            activity: AppActivityState::BackgroundRestricted,
            ..Default::default()
        };
        let p = plan_for(PowerPolicyProfile::Balanced, s);
        assert!(!p.enabled);
        assert_eq!(p.duty_percent(), 0);
    }

    #[test]
    fn tiny_payloads_never_justify_a_wifi_upgrade() {
        let p = plan_for(PowerPolicyProfile::Balanced, fg());
        assert!(!worth_upgrading(2_000, PowerPolicyProfile::Balanced, &p));
        assert!(worth_upgrading(
            50 * 1024 * 1024,
            PowerPolicyProfile::Balanced,
            &p
        ));
        let saver = plan_for(PowerPolicyProfile::Saver, fg());
        assert!(
            !worth_upgrading(u64::MAX, PowerPolicyProfile::Saver, &saver),
            "saver stays on BLE"
        );
    }
}
