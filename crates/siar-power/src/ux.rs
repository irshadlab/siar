//! §115-§119: diagnostics lines, user-facing wording and the notification
//! policy. Pure functions over the same types the policy uses, so the words
//! a user reads can never drift from the decision that was actually made.

use crate::budget::PowerBudget;
use crate::diagnostics::PowerDiagnostics;
use crate::discovery;
use crate::profile::PowerPolicyProfile;
use crate::state::{BatteryLevelClass, PowerState, ThermalState};
use crate::work::{
    DecisionReason as R, Evaluation, PowerAwareWork, PowerDecision, WaitFor, WorkDomain,
};

fn profile_name(p: PowerPolicyProfile) -> &'static str {
    match p {
        PowerPolicyProfile::Performance => "Performance",
        PowerPolicyProfile::Balanced => "Balanced",
        PowerPolicyProfile::Saver => "Saver",
        PowerPolicyProfile::Emergency => "Emergency",
        PowerPolicyProfile::AlwaysOnNode => "Always-on node",
    }
}

/// §115 "Advanced" diagnostics, one fact per line. Classes only — no exact
/// percentage, no timestamps (§86).
#[must_use]
pub fn diagnostic_lines(d: &PowerDiagnostics) -> Vec<String> {
    let s = &d.state;
    let b = &d.budget;
    let base = PowerBudget::base(d.profile).max_parallel_bulk;
    let battery = match s.battery {
        BatteryLevelClass::Critical => "Critical",
        BatteryLevelClass::Low => "Low",
        BatteryLevelClass::Medium => "Medium",
        BatteryLevelClass::High => "High",
        BatteryLevelClass::Full => "Full",
        BatteryLevelClass::Unknown => "Unknown",
    };
    let thermal = match s.thermal {
        ThermalState::Nominal => "Nominal",
        ThermalState::Elevated => "Elevated",
        ThermalState::Serious => "Serious",
        ThermalState::Critical => "Critical",
        ThermalState::Unknown => "Unknown",
    };
    let plan = discovery::plan(d.profile, s, b);
    let scan = if !plan.enabled {
        "off"
    } else if plan.duty_percent() >= 90 {
        "continuous"
    } else if plan.duty_percent() <= 2 {
        "reduced"
    } else {
        "normal"
    };
    vec![
        format!("Power mode: {}", profile_name(d.profile)),
        format!("Battery: {battery}"),
        format!(
            "Charging: {}",
            if s.external_power() { "Yes" } else { "No" }
        ),
        format!("Thermal: {thermal}"),
        format!("Bulk workers: {}/{}", b.max_parallel_bulk, base),
        format!(
            "Multipath: {}",
            if b.multipath_allowed {
                "enabled"
            } else {
                "disabled"
            }
        ),
        format!("DTN scan: {scan}"),
        format!("Waiting for better conditions: {}", d.deferred.total()),
    ]
}

/// §116: the one sentence a user sees for a deferral or throttle, or `None`
/// when there is nothing to say. Wording is actionable: *what happened* and
/// *what will happen / what you can do*.
#[must_use]
pub fn user_message(work: &PowerAwareWork, eval: &Evaluation) -> Option<&'static str> {
    let media = work.domain == WorkDomain::Media;
    match (eval.decision, eval.reason) {
        (PowerDecision::Run, _) | (PowerDecision::Drop, _) => None,
        (_, R::ThermalSafety | R::ThermalThrottle) if media => {
            Some("Video quality reduced to cool the device.")
        }
        (_, R::ThermalSafety | R::ThermalThrottle) => {
            Some("Paused while the device cools down. It will resume by itself.")
        }
        (_, R::OsRestriction) => {
            Some("The system is limiting background activity. Open Siar to continue.")
        }
        (_, R::MeteredNetwork) => {
            Some("Waiting for Wi-Fi to save mobile data. Tap “Send now” to use mobile data.")
        }
        (_, R::ResourcePressure) => {
            Some("Waiting for the device to free up memory. It will resume by itself.")
        }
        (_, R::AppPolicy) => {
            Some("Waiting until the device is charging, as set in this app’s settings.")
        }
        (_, R::CriticalBatteryReserve | R::BackgroundBudget | R::BatteryThrottle) => {
            match eval.wait {
                Some(WaitFor::Charging) => {
                    Some("Large transfer paused to save battery. It will resume while charging.")
                }
                Some(WaitFor::Foreground) => {
                    Some("Paused in the background. Open Siar to continue.")
                }
                Some(WaitFor::ThermalRecovery) => {
                    Some("Paused while the device cools down. It will resume by itself.")
                }
                _ if eval.decision == PowerDecision::RunThrottled => {
                    Some("Running slower to save battery.")
                }
                _ => Some("Paused to save battery. It will resume automatically."),
            }
        }
        _ => None,
    }
}

/// §118: shown only when the user *enabled* emergency mode and the battery is
/// critical — never otherwise.
#[must_use]
pub fn emergency_banner(state: &PowerState, emergency_enabled: bool) -> Option<&'static str> {
    (emergency_enabled && state.in_critical_reserve())
        .then_some("Emergency mode is conserving power for messages and SOS.")
}

/// §119: shown during a call/stream when thermal pressure cut quality.
#[must_use]
pub const fn thermal_call_notice(thermal: ThermalState) -> Option<&'static str> {
    if thermal.severity() >= 1 {
        Some("Video quality reduced to cool the device.")
    } else {
        None
    }
}

/// What may justify interrupting the user (§117). Anything not listed here
/// can never produce a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyTrigger {
    UserActionRequired,
    /// A transfer the user can see has been paused for `paused_for_ms`.
    VisibleTransferPaused {
        paused_for_ms: u64,
    },
    CriticalFeatureUnavailable,
}

impl NotifyTrigger {
    const fn slot(self) -> usize {
        match self {
            Self::UserActionRequired => 0,
            Self::VisibleTransferPaused { .. } => 1,
            Self::CriticalFeatureUnavailable => 2,
        }
    }
}

/// Rate-limits power notifications: never spam (§117).
#[derive(Debug, Clone, Copy)]
pub struct NotificationGate {
    /// Per-kind minimum gap between notifications.
    pub cooldown_ms: u64,
    /// A visible pause must last this long to count as "long-running".
    pub min_visible_pause_ms: u64,
    last: [Option<u64>; 3],
}

impl Default for NotificationGate {
    fn default() -> Self {
        Self {
            cooldown_ms: 10 * 60_000,
            min_visible_pause_ms: 30_000,
            last: [None; 3],
        }
    }
}

impl NotificationGate {
    pub fn should_notify(&mut self, t: NotifyTrigger, now: u64) -> bool {
        if let NotifyTrigger::VisibleTransferPaused { paused_for_ms } = t {
            if paused_for_ms < self.min_visible_pause_ms {
                return false;
            }
        }
        let slot = &mut self.last[t.slot()];
        if slot.is_some_and(|prev| now.saturating_sub(prev) < self.cooldown_ms) {
            return false;
        }
        *slot = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{DefaultPowerPolicy, ResourceSnapshot};
    use crate::state::{AppActivityState, ChargingState};
    use crate::work::{EnergyClass, WorkPriority};

    fn eval(work: &PowerAwareWork, state: &PowerState) -> Evaluation {
        DefaultPowerPolicy::default().evaluate_detailed(work, state, &ResourceSnapshot::default())
    }

    #[test]
    fn diagnostics_match_the_specs_example_shape_and_leak_nothing_exact() {
        let mut sched =
            crate::scheduler::PowerScheduler::new(Default::default(), Default::default());
        sched.observe(
            &crate::tracker::RawPowerReading {
                battery_percent: Some(17),
                charging: ChargingState::NotCharging,
                thermal: ThermalState::Elevated,
                activity: AppActivityState::BackgroundAllowed,
                ..Default::default()
            },
            0,
        );
        let lines = diagnostic_lines(&sched.diagnostics());
        let text = lines.join("\n");
        assert!(text.contains("Power mode: Saver"), "{text}");
        assert!(
            text.contains("Battery: Low")
                && text.contains("Charging: No")
                && text.contains("Thermal: Elevated")
        );
        assert!(text.contains("Bulk workers: 1/1"), "{text}");
        assert!(text.contains("Multipath: disabled") && text.contains("DTN scan: reduced"));
        assert!(!text.contains("17") && !text.contains('%'));
    }

    #[test]
    fn messages_are_actionable_and_name_the_resume_condition() {
        let bulk = PowerAwareWork::new(WorkDomain::Files, WorkPriority::Bulk, EnergyClass::High);
        let low = PowerState {
            battery: BatteryLevelClass::Low,
            charging: ChargingState::NotCharging,
            ..Default::default()
        };
        let m = user_message(&bulk, &eval(&bulk, &low)).unwrap();
        assert!(
            m.contains("paused to save battery") && m.contains("charging"),
            "{m}"
        );

        let metered = PowerState {
            metered_network: true,
            charging: ChargingState::Full,
            ..Default::default()
        };
        assert!(user_message(&bulk, &eval(&bulk, &metered))
            .unwrap()
            .contains("Send now"));

        let av1 = PowerAwareWork::new(
            WorkDomain::Media,
            WorkPriority::Interactive,
            EnergyClass::VeryHigh,
        )
        .with_durable(true);
        let hot = PowerState {
            thermal: ThermalState::Elevated,
            charging: ChargingState::Full,
            ..Default::default()
        };
        assert_eq!(
            user_message(&av1, &eval(&av1, &hot)),
            Some("Video quality reduced to cool the device.")
        );
    }

    #[test]
    fn nothing_to_say_when_work_just_runs() {
        let w = PowerAwareWork::new(WorkDomain::Message, WorkPriority::Normal, EnergyClass::Low);
        assert_eq!(user_message(&w, &eval(&w, &PowerState::default())), None);
    }

    #[test]
    fn emergency_banner_needs_the_user_to_have_enabled_it() {
        let dying = PowerState {
            battery: BatteryLevelClass::Critical,
            charging: ChargingState::NotCharging,
            ..Default::default()
        };
        assert!(emergency_banner(&dying, true).is_some());
        assert!(emergency_banner(&dying, false).is_none());
        assert!(emergency_banner(&PowerState::default(), true).is_none());
    }

    #[test]
    fn notifications_are_rare_and_rate_limited() {
        let mut g = NotificationGate::default();
        assert!(
            !g.should_notify(
                NotifyTrigger::VisibleTransferPaused {
                    paused_for_ms: 5_000
                },
                0
            ),
            "short pauses never notify"
        );
        assert!(g.should_notify(
            NotifyTrigger::VisibleTransferPaused {
                paused_for_ms: 60_000
            },
            1
        ));
        assert!(
            !g.should_notify(
                NotifyTrigger::VisibleTransferPaused {
                    paused_for_ms: 90_000
                },
                2
            ),
            "cooldown"
        );
        assert!(
            g.should_notify(NotifyTrigger::UserActionRequired, 3),
            "kinds are independent"
        );
        assert!(g.should_notify(
            NotifyTrigger::VisibleTransferPaused {
                paused_for_ms: 60_000
            },
            1 + 10 * 60_000
        ));
    }

    #[test]
    fn thermal_call_notice_only_when_cooling_matters() {
        assert!(thermal_call_notice(ThermalState::Nominal).is_none());
        assert!(thermal_call_notice(ThermalState::Serious).is_some());
    }
}
