//! ui-ux-18/20 + sys-arch Part 13 §116-§119: honest, plain-language power
//! and background status.
//!
//! Pure mapping from [`siar_power::PowerDiagnostics`] to display strings.
//! Only coarse classes ever reach the screen (Part 13 §86): there is no
//! battery percentage in the input, so none can be shown or leaked into a
//! screenshot or bug report.

use siar_power::{
    AppActivityState, BatteryLevelClass, ChargingState, PowerDiagnostics, PowerPolicyProfile,
    ThermalState,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerStatus {
    /// One line: what Siar is doing about power right now.
    pub headline: String,
    /// Zero or more short lines: the concrete effects.
    pub details: Vec<String>,
    /// Large transfers / sync waiting for better conditions.
    pub waiting: u32,
}

impl PowerStatus {
    #[must_use]
    pub fn details_text(&self) -> String {
        self.details.join("\n")
    }
}

fn battery_word(b: BatteryLevelClass) -> Option<&'static str> {
    match b {
        BatteryLevelClass::Critical => Some("Battery is critically low"),
        BatteryLevelClass::Low => Some("Battery is low"),
        BatteryLevelClass::Medium => Some("Battery is fine"),
        BatteryLevelClass::High | BatteryLevelClass::Full => Some("Battery is good"),
        BatteryLevelClass::Unknown => None,
    }
}

/// Describe the current situation. Never promises more than the engine does.
#[must_use]
pub fn describe(d: &PowerDiagnostics) -> PowerStatus {
    let s = &d.state;
    let headline = match d.profile {
        PowerPolicyProfile::Performance => "Running at full speed",
        PowerPolicyProfile::Balanced => "Running normally",
        PowerPolicyProfile::Saver => "Saving power",
        PowerPolicyProfile::Emergency => "Emergency mode: reachability first",
        PowerPolicyProfile::AlwaysOnNode => "Always-on node",
    }
    .to_owned();

    let mut details = Vec::new();
    if let Some(w) = battery_word(s.battery) {
        let mains = match s.charging {
            ChargingState::ChargingSlow | ChargingState::ChargingFast => ", charging",
            ChargingState::Full if s.battery != BatteryLevelClass::Unknown => ", plugged in",
            _ => "",
        };
        details.push(format!("{w}{mains}"));
    } else if s.charging.on_external_power() {
        details.push("Plugged in".to_owned());
    }
    match s.thermal {
        ThermalState::Elevated => details.push("Device is warm — heavy work is eased off".into()),
        ThermalState::Serious => {
            details.push("Device is hot — video and big transfers are limited".into())
        }
        ThermalState::Critical => {
            details.push("Device is very hot — only essential messages are sent".into())
        }
        ThermalState::Nominal | ThermalState::Unknown => {}
    }
    if s.saver_enabled {
        details.push("Your device’s battery saver is on".into());
    }
    if s.costly_network() {
        details.push("On a metered or roaming connection — large files wait for Wi-Fi".into());
    }
    if s.os_background_network_blocked && s.activity != AppActivityState::ForegroundInteractive {
        details.push("The system is blocking background network use".into());
    }
    if d.budget.max_parallel_bulk == 0 {
        details.push("Large transfers are paused".into());
    } else if d.budget.max_parallel_bulk == 1 {
        details.push("Large transfers run one at a time".into());
    }
    if !d.budget.multipath_allowed && d.profile == PowerPolicyProfile::Saver {
        details.push("Using one connection at a time".into());
    }
    let waiting = d.deferred.total();
    if waiting > 0 {
        details.push(format!(
            "{waiting} {} waiting for better conditions — nothing is lost",
            if waiting == 1 { "item is" } else { "items are" }
        ));
    }
    PowerStatus {
        headline,
        details,
        waiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_power::{
        EnergyClass, OpId, PowerAwareWork, PowerPolicyConfig, PowerScheduler, RawPowerReading,
        SchedulerConfig, WorkDomain, WorkPriority,
    };

    fn diag(pct: u8, ch: ChargingState, thermal: ThermalState) -> PowerDiagnostics {
        let mut s = PowerScheduler::new(PowerPolicyConfig::default(), SchedulerConfig::default());
        s.observe(
            &RawPowerReading {
                battery_percent: Some(pct),
                charging: ch,
                thermal,
                ..Default::default()
            },
            0,
        );
        s.diagnostics()
    }

    #[test]
    fn low_battery_says_saving_and_pauses_nothing_silently() {
        let mut s = PowerScheduler::new(PowerPolicyConfig::default(), SchedulerConfig::default());
        s.observe(
            &RawPowerReading {
                battery_percent: Some(12),
                charging: ChargingState::NotCharging,
                ..Default::default()
            },
            0,
        );
        s.submit(
            OpId(1),
            PowerAwareWork::new(WorkDomain::Files, WorkPriority::Bulk, EnergyClass::High),
            1,
        )
        .unwrap();
        let p = describe(&s.diagnostics());
        assert_eq!(p.headline, "Saving power");
        assert!(p.details.iter().any(|l| l.contains("Battery is")));
        assert!(
            p.details.iter().any(|l| l.contains("nothing is lost")),
            "{:?}",
            p.details
        );
        assert_eq!(p.waiting, 1);
    }

    #[test]
    fn plugged_in_and_cool_is_full_speed() {
        let p = describe(&diag(
            80,
            ChargingState::ChargingFast,
            ThermalState::Nominal,
        ));
        assert_eq!(p.headline, "Running at full speed");
        assert!(p.details[0].contains("charging"));
    }

    #[test]
    fn heat_is_explained_in_plain_words() {
        let p = describe(&diag(80, ChargingState::Full, ThermalState::Serious));
        assert!(p.details_text().contains("hot"));
    }

    #[test]
    fn output_never_contains_a_percentage_sign_or_digits_from_the_battery() {
        for pct in [3u8, 17, 42, 88, 100] {
            let t = describe(&diag(
                pct,
                ChargingState::NotCharging,
                ThermalState::Nominal,
            ));
            let all = format!("{} {}", t.headline, t.details_text());
            assert!(!all.contains('%'), "{all}");
            assert!(!all.contains(&pct.to_string()) || pct == 1, "{all}");
        }
    }

    #[test]
    fn batteryless_desktop_just_says_plugged_in() {
        let mut s = PowerScheduler::new(PowerPolicyConfig::default(), SchedulerConfig::default());
        s.observe(
            &RawPowerReading {
                battery_percent: None,
                charging: ChargingState::Full,
                ..Default::default()
            },
            0,
        );
        let p = describe(&s.diagnostics());
        assert_eq!(p.details[0], "Plugged in");
    }
}
