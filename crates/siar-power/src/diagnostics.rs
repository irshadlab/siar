//! §85, §98, §115: counters and a diagnostics snapshot.
//!
//! §98: "do not pretend to know exact joules without hardware telemetry" —
//! nothing here estimates energy. These count *decisions*. §86: a snapshot
//! contains classes only, so it is safe to show in a diagnostics screen or
//! attach to a bug report.

use serde::{Deserialize, Serialize};

use crate::budget::PowerBudget;
use crate::profile::PowerPolicyProfile;
use crate::state::PowerState;
use crate::tracker::PowerStateEpoch;
use crate::work::{DecisionReason, WaitFor};

/// §85. Saturating `u64` counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerMetrics {
    pub deferred_by_power: u64,
    pub bulk_resumed_on_charge: u64,
    pub multipath_disabled_by_saver: u64,
    pub dtn_scans_reduced: u64,
    pub media_downgraded_thermal: u64,
    pub mode_transitions: u64,
    pub ephemeral_dropped: u64,
    pub user_overrides: u64,
    pub aged_promotions: u64,
}

impl PowerMetrics {
    pub(crate) fn bump(counter: &mut u64) {
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn note_reason(&mut self, reason: DecisionReason, deferred: bool, dropped: bool) {
        if dropped {
            Self::bump(&mut self.ephemeral_dropped);
        }
        if deferred && !matches!(reason, DecisionReason::OsRestriction) {
            Self::bump(&mut self.deferred_by_power);
        }
        match reason {
            DecisionReason::UserInitiated => Self::bump(&mut self.user_overrides),
            DecisionReason::Aged => Self::bump(&mut self.aged_promotions),
            _ => {}
        }
    }

    pub(crate) fn note_transition(&mut self, t: &crate::tracker::Transition) {
        Self::bump(&mut self.mode_transitions);
        if t.previous_budget.multipath_allowed
            && !t.budget.multipath_allowed
            && t.profile == PowerPolicyProfile::Saver
        {
            Self::bump(&mut self.multipath_disabled_by_saver);
        }
    }
}

/// Count of deferred operations per thing they wait for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitCounts {
    pub charging: u32,
    pub unmetered_network: u32,
    pub foreground: u32,
    pub thermal_recovery: u32,
    pub battery_recovery: u32,
    pub resource_relief: u32,
    pub timed: u32,
}

impl WaitCounts {
    pub(crate) fn add(&mut self, w: WaitFor) {
        let slot = match w {
            WaitFor::Charging => &mut self.charging,
            WaitFor::UnmeteredNetwork => &mut self.unmetered_network,
            WaitFor::Foreground => &mut self.foreground,
            WaitFor::ThermalRecovery => &mut self.thermal_recovery,
            WaitFor::BatteryRecovery => &mut self.battery_recovery,
            WaitFor::ResourceRelief => &mut self.resource_relief,
            WaitFor::Time(_) => &mut self.timed,
        };
        *slot = slot.saturating_add(1);
    }

    #[must_use]
    pub const fn total(&self) -> u32 {
        self.charging
            + self.unmetered_network
            + self.foreground
            + self.thermal_recovery
            + self.battery_recovery
            + self.resource_relief
            + self.timed
    }
}

/// §115: safe to display or export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerDiagnostics {
    pub epoch: PowerStateEpoch,
    pub profile: PowerPolicyProfile,
    pub state: PowerState,
    pub budget: PowerBudget,
    pub deferred: WaitCounts,
    pub metrics: PowerMetrics,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_saturate() {
        let mut m = PowerMetrics {
            mode_transitions: u64::MAX,
            ..Default::default()
        };
        PowerMetrics::bump(&mut m.mode_transitions);
        assert_eq!(m.mode_transitions, u64::MAX);
    }

    #[test]
    fn wait_counts_total() {
        let mut w = WaitCounts::default();
        w.add(WaitFor::Charging);
        w.add(WaitFor::Time(5));
        w.add(WaitFor::Foreground);
        assert_eq!(w.total(), 3);
    }
}
