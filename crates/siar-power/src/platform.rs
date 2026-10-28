//! §58-§67, §139: platform adapters.
//!
//! Adapters only *report* (§8: "map platform behaviour into this neutral
//! model"); the Rust policy stays authoritative. Provided here:
//!
//! * [`PushPowerSource`] — a watch-channel source a platform glue layer
//!   writes into (Android's `BatteryManager` / `PowerManager` / Doze
//!   callbacks arrive through JNI and call [`PushPowerSource::push`]; iOS
//!   likewise).
//! * [`read_linux_sysfs`] — a real, dependency-free desktop/headless reader
//!   for `/sys/class/power_supply` and `/sys/class/thermal`, with the root
//!   injectable so it is fully testable.

use std::fs;
use std::path::Path;

use tokio::sync::watch;

use crate::state::{ChargingState, ThermalState};
use crate::tracker::RawPowerReading;

/// §139's `PowerStateStream`. (Carries raw readings: the tracker turns them
/// into a [`crate::state::PowerState`] so hysteresis lives in exactly one place.)
pub type PowerStateStream = watch::Receiver<RawPowerReading>;

/// §139.
pub trait PowerStateProvider {
    fn current(&self) -> RawPowerReading;
    fn subscribe(&self) -> PowerStateStream;
}

/// A pushed source: the platform layer owns the writing half.
#[derive(Debug)]
pub struct PushPowerSource {
    tx: watch::Sender<RawPowerReading>,
}

impl PushPowerSource {
    #[must_use]
    pub fn new(initial: RawPowerReading) -> Self {
        Self {
            tx: watch::channel(initial).0,
        }
    }

    /// Replace the reading. Identical consecutive readings are not re-sent,
    /// so subscribers wake only on real change (§84).
    pub fn push(&self, reading: RawPowerReading) {
        self.tx.send_if_modified(|cur| {
            if *cur == reading {
                false
            } else {
                *cur = reading;
                true
            }
        });
    }

    /// Update part of the reading in place (e.g. only the activity state).
    pub fn update(&self, f: impl FnOnce(&mut RawPowerReading)) {
        self.tx.send_if_modified(|cur| {
            let before = *cur;
            f(cur);
            *cur != before
        });
    }
}

impl PowerStateProvider for PushPowerSource {
    fn current(&self) -> RawPowerReading {
        *self.tx.borrow()
    }
    fn subscribe(&self) -> PowerStateStream {
        self.tx.subscribe()
    }
}

/// Temperature bands in °C for mapping a sensor to [`ThermalState`] (§7:
/// "exact thresholds platform/policy-specific").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalThresholds {
    pub elevated_c: u32,
    pub serious_c: u32,
    pub critical_c: u32,
}

impl Default for ThermalThresholds {
    fn default() -> Self {
        Self {
            elevated_c: 75,
            serious_c: 85,
            critical_c: 95,
        }
    }
}

impl ThermalThresholds {
    #[must_use]
    pub const fn classify(&self, milli_c: i64) -> ThermalState {
        let c = milli_c / 1000;
        if c <= 0 || c > 150 {
            return ThermalState::Unknown; // sensor noise, not a temperature
        }
        let c = c as u32;
        if c >= self.critical_c {
            ThermalState::Critical
        } else if c >= self.serious_c {
            ThermalState::Serious
        } else if c >= self.elevated_c {
            ThermalState::Elevated
        } else {
            ThermalState::Nominal
        }
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

/// Read the Linux power-supply and thermal sysfs trees under `root`
/// (normally `/sys`). Never fails: anything missing is `Unknown`.
///
/// * No battery found → `charging = Full`, `battery_percent = None`: a
///   battery-less desktop or server *is* on external power.
/// * Peripheral batteries (`scope = Device`, e.g. a wireless mouse) are ignored.
#[must_use]
pub fn read_linux_sysfs(root: &Path, thresholds: ThermalThresholds) -> RawPowerReading {
    let mut r = RawPowerReading::default();

    let mut battery: Option<(Option<u8>, String, Option<String>)> = None;
    let mut mains_online = false;
    if let Ok(dir) = fs::read_dir(root.join("class/power_supply")) {
        let mut entries: Vec<_> = dir.flatten().collect();
        entries.sort_by_key(|e| e.file_name()); // deterministic choice among several batteries
        for e in entries {
            let p = e.path();
            let kind = read_trim(&p.join("type")).unwrap_or_default();
            if read_trim(&p.join("scope")).as_deref() == Some("Device") {
                continue;
            }
            match kind.as_str() {
                "Battery" => {
                    if battery.is_none() {
                        let cap = read_trim(&p.join("capacity")).and_then(|s| s.parse::<u8>().ok());
                        let status = read_trim(&p.join("status")).unwrap_or_default();
                        battery = Some((cap, status, read_trim(&p.join("charge_type"))));
                    }
                }
                "Mains" | "USB" | "UPS" | "USB_C" | "USB_PD" => {
                    if read_trim(&p.join("online")).as_deref() == Some("1") {
                        mains_online = true;
                    }
                }
                _ => {}
            }
        }
    }
    match battery {
        Some((cap, status, charge_type)) => {
            r.battery_percent = cap;
            r.charging = match status.as_str() {
                "Charging" => match charge_type.as_deref() {
                    Some("Trickle") => ChargingState::ChargingSlow,
                    _ => ChargingState::ChargingFast,
                },
                "Full" => ChargingState::Full,
                // Plugged in but not charging (charge-limit threshold): mains is present.
                "Not charging" => ChargingState::Full,
                "Discharging" => ChargingState::NotCharging,
                _ if mains_online => ChargingState::Full,
                _ => ChargingState::Unknown,
            };
        }
        None => r.charging = ChargingState::Full,
    }

    let mut hottest: Option<i64> = None;
    if let Ok(dir) = fs::read_dir(root.join("class/thermal")) {
        for e in dir.flatten() {
            if !e.file_name().to_string_lossy().starts_with("thermal_zone") {
                continue;
            }
            if let Some(t) = read_trim(&e.path().join("temp")).and_then(|s| s.parse::<i64>().ok()) {
                if thresholds.classify(t) != ThermalState::Unknown {
                    hottest = Some(hottest.map_or(t, |h| h.max(t)));
                }
            }
        }
    }
    r.thermal = hottest.map_or(ThermalState::Unknown, |t| thresholds.classify(t));
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fake(PathBuf);
    impl Fake {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("siar-power-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn put(&self, rel: &str, v: &str) {
            let f = self.0.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, v).unwrap();
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn laptop_on_battery() {
        let f = Fake::new("bat");
        f.put("class/power_supply/BAT0/type", "Battery\n");
        f.put("class/power_supply/BAT0/capacity", "42\n");
        f.put("class/power_supply/BAT0/status", "Discharging\n");
        f.put("class/power_supply/AC/type", "Mains\n");
        f.put("class/power_supply/AC/online", "0\n");
        f.put("class/thermal/thermal_zone0/temp", "61000\n");
        let r = read_linux_sysfs(&f.0, ThermalThresholds::default());
        assert_eq!(r.battery_percent, Some(42));
        assert_eq!(r.charging, ChargingState::NotCharging);
        assert_eq!(r.thermal, ThermalState::Nominal);
    }

    #[test]
    fn laptop_charging_hot() {
        let f = Fake::new("hot");
        f.put("class/power_supply/BAT0/type", "Battery");
        f.put("class/power_supply/BAT0/capacity", "80");
        f.put("class/power_supply/BAT0/status", "Charging");
        f.put("class/thermal/thermal_zone0/temp", "60000");
        f.put("class/thermal/thermal_zone1/temp", "91000");
        let r = read_linux_sysfs(&f.0, ThermalThresholds::default());
        assert_eq!(r.charging, ChargingState::ChargingFast);
        assert_eq!(r.thermal, ThermalState::Serious, "the hottest zone wins");
    }

    #[test]
    fn trickle_is_slow_and_not_charging_threshold_counts_as_mains() {
        let f = Fake::new("trk");
        f.put("class/power_supply/BAT0/type", "Battery");
        f.put("class/power_supply/BAT0/status", "Charging");
        f.put("class/power_supply/BAT0/charge_type", "Trickle");
        assert_eq!(
            read_linux_sysfs(&f.0, ThermalThresholds::default()).charging,
            ChargingState::ChargingSlow
        );
        f.put("class/power_supply/BAT0/status", "Not charging");
        assert_eq!(
            read_linux_sysfs(&f.0, ThermalThresholds::default()).charging,
            ChargingState::Full
        );
    }

    #[test]
    fn batteryless_desktop_counts_as_external_power() {
        let f = Fake::new("desk");
        let r = read_linux_sysfs(&f.0, ThermalThresholds::default());
        assert_eq!(r.battery_percent, None);
        assert!(r.charging.on_external_power());
        assert_eq!(r.thermal, ThermalState::Unknown);
    }

    #[test]
    fn peripheral_batteries_are_ignored() {
        let f = Fake::new("mouse");
        f.put("class/power_supply/hid-mouse/type", "Battery");
        f.put("class/power_supply/hid-mouse/scope", "Device");
        f.put("class/power_supply/hid-mouse/capacity", "3");
        f.put("class/power_supply/hid-mouse/status", "Discharging");
        let r = read_linux_sysfs(&f.0, ThermalThresholds::default());
        assert_eq!(
            r.battery_percent, None,
            "a dying mouse must not put the laptop in saver"
        );
    }

    #[test]
    fn garbage_sensors_are_unknown_not_critical() {
        let f = Fake::new("junk");
        f.put("class/thermal/thermal_zone0/temp", "-274000");
        f.put("class/thermal/thermal_zone1/temp", "999999999");
        f.put("class/thermal/thermal_zone2/temp", "not-a-number");
        assert_eq!(
            read_linux_sysfs(&f.0, ThermalThresholds::default()).thermal,
            ThermalState::Unknown
        );
    }

    #[test]
    fn missing_sysfs_never_panics() {
        let r = read_linux_sysfs(
            Path::new("/definitely/not/here"),
            ThermalThresholds::default(),
        );
        assert!(r.charging.on_external_power());
    }

    #[tokio::test]
    async fn push_source_wakes_subscribers_only_on_real_change() {
        let src = PushPowerSource::new(RawPowerReading::default());
        let mut rx = src.subscribe();
        rx.borrow_and_update();
        src.push(RawPowerReading::default());
        assert!(
            !rx.has_changed().unwrap(),
            "identical reading is not a change"
        );
        src.update(|r| r.saver_enabled = true);
        assert!(rx.has_changed().unwrap());
        assert!(src.current().saver_enabled);
    }
}
