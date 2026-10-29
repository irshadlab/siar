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

// ───────────────────── §105: stable host-report API ─────────────────────

/// What an external platform adapter (Android service, iOS app, a host
/// application embedding the core over FFI) reports. **Reports facts only;
/// the Rust core decides** (§105). Stable and deliberately small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostReport {
    pub foreground: bool,
    /// Foreground *and* the user is actively interacting.
    pub interactive: bool,
    /// The OS says background execution is restricted for this app.
    pub background_restricted: bool,
    pub power_saver: bool,
    pub metered: bool,
    pub roaming: bool,
    pub battery_percent: Option<u8>,
    pub charging: ChargingState,
    pub thermal: ThermalState,
}

impl HostReport {
    /// Map onto the neutral reading the tracker consumes.
    #[must_use]
    pub fn to_raw(&self) -> RawPowerReading {
        use crate::state::AppActivityState as A;
        let activity = match (
            self.foreground,
            self.interactive,
            self.background_restricted,
        ) {
            (true, true, _) => A::ForegroundInteractive,
            (true, false, _) => A::ForegroundIdle,
            (false, _, true) => A::BackgroundRestricted,
            (false, _, false) => A::BackgroundAllowed,
        };
        RawPowerReading {
            battery_percent: self.battery_percent,
            charging: self.charging,
            thermal: self.thermal,
            activity,
            saver_enabled: self.power_saver,
            metered_network: self.metered,
            roaming: self.roaming,
            os_background_network_blocked: self.background_restricted && !self.foreground,
        }
    }
}

impl PushPowerSource {
    /// Convenience for host adapters.
    pub fn report(&self, r: &HostReport) {
        self.push(r.to_raw());
    }
}

// ─────────────── §100-§102: vendor quirks live in the adapter ───────────────

/// How hard a vendor's OS tends to kill background apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAggression {
    Standard,
    /// Known to freeze or kill ordinary background apps quickly.
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorQuirk {
    pub aggression: BackgroundAggression,
    /// Optional, plain-language help. Never a requirement (§102).
    pub guidance: &'static str,
}

const GUIDANCE_AGGRESSIVE: &str =
    "Your phone may pause Siar in the background to save power. Siar still works without changing anything; \
     if you want more reliable background delivery, you can allow Siar in your phone’s battery settings.";

/// Data-driven (§101: no code-level hacks). Matched case-insensitively on the
/// manufacturer string. Sources are public vendor-behaviour reports; extend
/// the table, never the policy.
const AGGRESSIVE_VENDORS: &[&str] = &[
    "xiaomi", "redmi", "poco", "huawei", "honor", "oppo", "realme", "oneplus", "vivo", "iqoo",
    "meizu", "asus", "tecno", "infinix", "itel",
];

#[must_use]
pub fn quirk_for(manufacturer: &str) -> VendorQuirk {
    let m = manufacturer.trim().to_ascii_lowercase();
    if AGGRESSIVE_VENDORS
        .iter()
        .any(|v| m == *v || m.starts_with(&format!("{v} ")))
    {
        VendorQuirk {
            aggression: BackgroundAggression::Aggressive,
            guidance: GUIDANCE_AGGRESSIVE,
        }
    } else {
        VendorQuirk {
            aggression: BackgroundAggression::Standard,
            guidance: "",
        }
    }
}

/// The only way a quirk touches policy: on an aggressive vendor, *assume* the
/// app will be restricted sooner. That makes the core schedule less in the
/// background — safer — without hard-coding anything vendor-specific into it.
#[must_use]
pub fn adjust_activity(
    a: crate::state::AppActivityState,
    q: VendorQuirk,
) -> crate::state::AppActivityState {
    use crate::state::AppActivityState as A;
    match (q.aggression, a) {
        (BackgroundAggression::Aggressive, A::BackgroundAllowed) => A::BackgroundRestricted,
        _ => a,
    }
}

#[cfg(test)]
mod host_and_quirk_tests {
    use super::*;
    use crate::state::AppActivityState as A;

    fn rep() -> HostReport {
        HostReport {
            foreground: false,
            interactive: false,
            background_restricted: false,
            power_saver: false,
            metered: false,
            roaming: false,
            battery_percent: Some(50),
            charging: ChargingState::NotCharging,
            thermal: ThermalState::Nominal,
        }
    }

    #[test]
    fn activity_mapping_covers_every_combination() {
        let f = |fg, i, br| {
            HostReport {
                foreground: fg,
                interactive: i,
                background_restricted: br,
                ..rep()
            }
            .to_raw()
            .activity
        };
        assert_eq!(f(true, true, false), A::ForegroundInteractive);
        assert_eq!(f(true, false, false), A::ForegroundIdle);
        assert_eq!(f(false, false, false), A::BackgroundAllowed);
        assert_eq!(f(false, false, true), A::BackgroundRestricted);
        assert_eq!(
            f(true, true, true),
            A::ForegroundInteractive,
            "foreground beats a stale restriction flag"
        );
    }

    #[test]
    fn os_network_block_only_when_restricted_in_background() {
        assert!(
            HostReport {
                background_restricted: true,
                ..rep()
            }
            .to_raw()
            .os_background_network_blocked
        );
        assert!(
            !HostReport {
                background_restricted: true,
                foreground: true,
                ..rep()
            }
            .to_raw()
            .os_background_network_blocked
        );
    }

    #[tokio::test]
    async fn host_report_flows_through_the_push_source() {
        let src = PushPowerSource::new(RawPowerReading::default());
        let mut rx = src.subscribe();
        rx.borrow_and_update();
        src.report(&HostReport {
            power_saver: true,
            ..rep()
        });
        assert!(rx.has_changed().unwrap());
        assert!(src.current().saver_enabled);
    }

    #[test]
    fn aggressive_vendors_make_the_core_more_conservative_never_less() {
        for v in ["Xiaomi", " HUAWEI ", "OnePlus", "realme"] {
            assert_eq!(
                quirk_for(v).aggression,
                BackgroundAggression::Aggressive,
                "{v}"
            );
        }
        for v in ["Google", "samsung", "Motorola", ""] {
            assert_eq!(
                quirk_for(v).aggression,
                BackgroundAggression::Standard,
                "{v}"
            );
        }
        let q = quirk_for("xiaomi");
        assert_eq!(
            adjust_activity(A::BackgroundAllowed, q),
            A::BackgroundRestricted
        );
        assert_eq!(
            adjust_activity(A::ForegroundInteractive, q),
            A::ForegroundInteractive
        );
        assert_eq!(
            adjust_activity(A::BackgroundAllowed, quirk_for("google")),
            A::BackgroundAllowed
        );
    }

    #[test]
    fn guidance_is_optional_never_a_demand() {
        let g = quirk_for("xiaomi").guidance.to_ascii_lowercase();
        assert!(g.contains("still works") && g.contains("if you want"));
        for banned in ["must", "required", "disable battery optimization"] {
            assert!(!g.contains(banned), "§102 violated by {banned:?}: {g}");
        }
        assert!(quirk_for("google").guidance.is_empty());
    }
}
