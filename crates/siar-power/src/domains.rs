//! §39-§57, §92-§95, §138: per-feature advice derived from one budget.
//!
//! `power.files()`, `power.dtn()`, `power.media()` — thin, pure views that
//! answer the questions each subsystem actually asks, so **no feature
//! re-derives battery logic of its own** (Definition of Done: "power logic
//! is centralized rather than scattered").

use serde::{Deserialize, Serialize};

use crate::budget::{BackgroundBudgetClass, CpuBudgetClass, PowerBudget};
use crate::discovery::{self, DiscoveryPlan};
use crate::profile::PowerPolicyProfile;
use crate::state::PowerState;

/// Borrowed view over the current power picture.
#[derive(Debug, Clone, Copy)]
pub struct PowerAdvisor<'a> {
    pub state: &'a PowerState,
    pub budget: &'a PowerBudget,
    pub profile: PowerPolicyProfile,
}

impl<'a> PowerAdvisor<'a> {
    #[must_use]
    pub const fn new(
        state: &'a PowerState,
        budget: &'a PowerBudget,
        profile: PowerPolicyProfile,
    ) -> Self {
        Self {
            state,
            budget,
            profile,
        }
    }

    #[must_use]
    pub fn files(&self) -> FilePowerAdvice {
        let parallel = self.budget.max_parallel_bulk;
        FilePowerAdvice {
            // §41-§42: chunk concurrency follows the bulk budget; one stream is always allowed
            // for a *foreground* transfer so the user is never stuck at zero.
            chunk_parallelism: if parallel == 0
                && self.state.activity.is_foreground()
                && self.state.thermal.severity() < 3
            {
                1
            } else {
                parallel
            },
            // §40: hashing is CPU-heavy; do it when the CPU budget allows.
            hashing_allowed: self.budget.cpu_class >= CpuBudgetClass::Low
                && self.state.thermal.severity() < 3,
            // §92/§94/§95: speculative work needs a generous budget.
            prefetch_allowed: self.budget.background_class >= BackgroundBudgetClass::Limited
                && self.state.external_power(),
            indexing_allowed: self.budget.background_class >= BackgroundBudgetClass::Limited
                && self.budget.cpu_class >= CpuBudgetClass::Normal,
            backup_allowed: self.budget.background_class >= BackgroundBudgetClass::Limited
                && (self.state.external_power()
                    || self.state.battery.effective() >= crate::state::BatteryLevelClass::High),
        }
    }

    #[must_use]
    pub fn dtn(&self) -> DtnPowerAdvice {
        let scan = discovery::plan(self.profile, self.state, self.budget);
        let relay = if self.budget.background_class == BackgroundBudgetClass::CriticalOnly
            && self.profile != PowerPolicyProfile::Emergency
        {
            RelayIntensity::Off
        } else if matches!(self.profile, PowerPolicyProfile::Saver) {
            RelayIntensity::Light
        } else if self.state.external_power() && self.state.thermal.severity() == 0 {
            // §46: charging devices are the preferred relays.
            RelayIntensity::Heavy
        } else {
            RelayIntensity::Normal
        };
        DtnPowerAdvice {
            scan,
            relay_intensity: relay,
            gateway_active: self.profile == PowerPolicyProfile::AlwaysOnNode,
        }
    }

    #[must_use]
    pub fn media(&self) -> MediaPowerAdvice {
        let sev = self.state.thermal.severity();
        let constrained =
            self.state.battery_constrained() || self.profile == PowerPolicyProfile::Saver;
        let cap = match (sev, constrained) {
            (3, _) => VideoCap::AudioOnly,
            (2, _) => VideoCap::Low,
            (1, _) | (_, true) => VideoCap::Medium,
            _ => VideoCap::High,
        };
        let av1 = if sev >= 2 || self.state.in_critical_reserve() {
            Av1Software::Disabled
        } else if sev == 1 || constrained || self.budget.cpu_class < CpuBudgetClass::Normal {
            Av1Software::LowComplexity
        } else {
            Av1Software::Normal
        };
        MediaPowerAdvice {
            // §51: prefer hardware codecs everywhere except where CPU is plentiful.
            prefer_hardware: self.profile != PowerPolicyProfile::Performance || sev >= 1,
            av1_software: av1,
            max_video: cap,
            // §53: audio outlives video. Always true; `AudioOnly` is the floor, never silence.
            audio_protected: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePowerAdvice {
    pub chunk_parallelism: u8,
    pub hashing_allowed: bool,
    pub prefetch_allowed: bool,
    pub indexing_allowed: bool,
    pub backup_allowed: bool,
}

/// §46, §88.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RelayIntensity {
    Off,
    Light,
    Normal,
    Heavy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DtnPowerAdvice {
    pub scan: DiscoveryPlan,
    pub relay_intensity: RelayIntensity,
    pub gateway_active: bool,
}

/// §50.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Av1Software {
    Disabled,
    LowComplexity,
    Normal,
}

/// §54: the ladder's video rungs. `AudioOnly` is the floor — **never** below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VideoCap {
    AudioOnly,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPowerAdvice {
    pub prefer_hardware: bool,
    pub av1_software: Av1Software,
    pub max_video: VideoCap,
    pub audio_protected: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{select_profile, PowerPolicyConfig};
    use crate::state::*;

    fn advice(s: PowerState) -> (FilePowerAdvice, DtnPowerAdvice, MediaPowerAdvice) {
        let c = PowerPolicyConfig::default();
        let p = select_profile(&s, &c, false);
        let b = PowerBudget::derive(p, &s, &c);
        let a = PowerAdvisor::new(&s, &b, p);
        (a.files(), a.dtn(), a.media())
    }

    #[test]
    fn file_concurrency_falls_with_battery_and_thermal() {
        let plugged = PowerState {
            charging: ChargingState::Full,
            activity: AppActivityState::ForegroundInteractive,
            ..Default::default()
        };
        let low = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::Low,
            ..plugged
        };
        let hot = PowerState {
            thermal: ThermalState::Serious,
            ..plugged
        };
        let (fp, ..) = advice(plugged);
        let (fl, ..) = advice(low);
        let (fh, ..) = advice(hot);
        assert!(fp.chunk_parallelism > fl.chunk_parallelism);
        assert!(fp.chunk_parallelism > fh.chunk_parallelism);
    }

    #[test]
    fn a_foreground_transfer_is_never_stuck_at_zero_unless_thermal_critical() {
        let crit_batt = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::Critical,
            activity: AppActivityState::ForegroundInteractive,
            ..Default::default()
        };
        assert_eq!(advice(crit_batt).0.chunk_parallelism, 1);
        let melt = PowerState {
            thermal: ThermalState::Critical,
            activity: AppActivityState::ForegroundInteractive,
            ..Default::default()
        };
        assert_eq!(advice(melt).0.chunk_parallelism, 0);
    }

    #[test]
    fn media_degrades_gracefully_and_audio_is_never_dropped() {
        let mut prev = VideoCap::High;
        for t in [
            ThermalState::Nominal,
            ThermalState::Elevated,
            ThermalState::Serious,
            ThermalState::Critical,
        ] {
            let (.., m) = advice(PowerState {
                thermal: t,
                charging: ChargingState::Full,
                ..Default::default()
            });
            assert!(m.max_video <= prev, "ladder is monotone in thermal");
            assert!(m.audio_protected);
            prev = m.max_video;
        }
        assert_eq!(prev, VideoCap::AudioOnly);
    }

    #[test]
    fn software_av1_is_throttled_then_disabled() {
        let c = |t| {
            advice(PowerState {
                thermal: t,
                charging: ChargingState::Full,
                ..Default::default()
            })
            .2
            .av1_software
        };
        assert_eq!(c(ThermalState::Nominal), Av1Software::Normal);
        assert_eq!(c(ThermalState::Elevated), Av1Software::LowComplexity);
        assert_eq!(c(ThermalState::Serious), Av1Software::Disabled);
    }

    #[test]
    fn charging_cool_device_is_the_preferred_relay() {
        let plugged = PowerState {
            charging: ChargingState::ChargingFast,
            ..Default::default()
        };
        let low = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::Low,
            ..Default::default()
        };
        let dying = PowerState {
            battery: BatteryLevelClass::Critical,
            ..low
        };
        assert_eq!(advice(plugged).1.relay_intensity, RelayIntensity::Heavy);
        assert_eq!(
            advice(low).1.relay_intensity,
            RelayIntensity::Light,
            "saver still relays lightly"
        );
        assert_eq!(advice(dying).1.relay_intensity, RelayIntensity::Off);
    }

    #[test]
    fn speculative_work_needs_mains_and_budget() {
        let battery = PowerState {
            charging: ChargingState::NotCharging,
            battery: BatteryLevelClass::High,
            ..Default::default()
        };
        let mains = PowerState {
            charging: ChargingState::Full,
            ..Default::default()
        };
        assert!(!advice(battery).0.prefetch_allowed);
        assert!(advice(mains).0.prefetch_allowed);
    }
}
