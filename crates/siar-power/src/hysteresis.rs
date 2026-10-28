//! §81-§82, §84: hysteresis and event coalescing.
//!
//! A battery at 19-21 % must not flip the whole runtime between profiles on
//! every reading, and a phone that just came off a thermal-critical episode
//! must not snap straight back to full CPU. Both filters here are pure state
//! machines over an injected clock (milliseconds), so they are exactly
//! reproducible in tests.

use crate::state::{BatteryLevelClass, ThermalState};

/// Percent at which a band is **entered going up**, indexed by the band
/// below it: Critical→Low, Low→Medium, Medium→High, High→Full.
const UP: [u8; 4] = [13, 25, 60, 95];
/// Percent *below which* a band is **left going down**, same indexing:
/// Low→Critical, Medium→Low, High→Medium, Full→High. Every entry is strictly
/// below the matching `UP` entry — that gap *is* the hysteresis (§81: enter
/// Low at <20 %, leave Low at ≥25 %).
const DOWN: [u8; 4] = [10, 20, 55, 90];

const BANDS: [BatteryLevelClass; 5] = [
    BatteryLevelClass::Critical,
    BatteryLevelClass::Low,
    BatteryLevelClass::Medium,
    BatteryLevelClass::High,
    BatteryLevelClass::Full,
];

#[cfg(test)]
fn band_index(c: BatteryLevelClass) -> Option<usize> {
    BANDS.iter().position(|b| *b == c)
}

/// Turns raw percentages into stable [`BatteryLevelClass`] values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatteryClassifier {
    band: Option<usize>,
}

impl BatteryClassifier {
    #[must_use]
    pub const fn new() -> Self {
        Self { band: None }
    }

    /// Feed one reading. `None` (no battery / no data) yields `Unknown` and
    /// forgets the band, so the next real reading classifies from scratch.
    /// Percentages above 100 are clamped.
    pub fn update(&mut self, percent: Option<u8>) -> BatteryLevelClass {
        let Some(p) = percent.map(|p| p.min(100)) else {
            self.band = None;
            return BatteryLevelClass::Unknown;
        };
        let mut i = match self.band {
            Some(i) => i,
            // First reading: the band whose "stay" range contains `p`.
            None => DOWN.iter().take_while(|&&d| p >= d).count(),
        };
        while i < 4 && p >= UP[i] {
            i += 1;
        }
        while i > 0 && p < DOWN[i - 1] {
            i -= 1;
        }
        self.band = Some(i);
        BANDS[i]
    }

    /// The class last reported, without feeding a reading.
    #[must_use]
    pub fn current(&self) -> BatteryLevelClass {
        self.band.map_or(BatteryLevelClass::Unknown, |i| BANDS[i])
    }
}

/// §82. Escalation is **immediate** (a hot device cannot wait); recovery
/// steps down **one level per dwell period**: `Critical → Serious →
/// Elevated → Nominal`, each only after `dwell_ms` at the lower-severity
/// reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalFilter {
    dwell_ms: u64,
    current: ThermalState,
    since: u64,
}

impl ThermalFilter {
    /// Default dwell per recovery step: 30 s.
    pub const DEFAULT_DWELL_MS: u64 = 30_000;

    #[must_use]
    pub const fn new(dwell_ms: u64) -> Self {
        Self {
            dwell_ms,
            current: ThermalState::Unknown,
            since: 0,
        }
    }

    /// Feed a raw reading at `now_ms`. A regressing clock never recovers
    /// early (saturating subtraction).
    pub fn update(&mut self, raw: ThermalState, now_ms: u64) -> ThermalState {
        if raw == ThermalState::Unknown && self.current == ThermalState::Unknown {
            return ThermalState::Unknown;
        }
        if self.current == ThermalState::Unknown {
            self.current = raw;
            self.since = now_ms;
            return self.current;
        }
        // An `Unknown` reading mid-run (sensor dropout) decays as Nominal
        // rather than pinning the last hot value forever.
        let raw_sev = raw.severity();
        let cur_sev = self.current.severity();
        if raw_sev >= cur_sev {
            if raw_sev > cur_sev || raw != ThermalState::Unknown {
                self.current = ThermalState::from_severity(raw_sev);
                self.since = now_ms;
            }
        } else if now_ms.saturating_sub(self.since) >= self.dwell_ms {
            self.current = ThermalState::from_severity(cur_sev - 1);
            self.since = now_ms;
        }
        self.current
    }

    #[must_use]
    pub const fn current(&self) -> ThermalState {
        self.current
    }
}

impl Default for ThermalFilter {
    fn default() -> Self {
        Self::new(Self::DEFAULT_DWELL_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn low_is_entered_below_20_and_left_at_25() {
        let mut c = BatteryClassifier::new();
        assert_eq!(c.update(Some(50)), BatteryLevelClass::Medium);
        assert_eq!(c.update(Some(21)), BatteryLevelClass::Medium);
        assert_eq!(c.update(Some(19)), BatteryLevelClass::Low);
        // Wobbling around the boundary does not flip it.
        for p in [20, 22, 24, 21, 23] {
            assert_eq!(c.update(Some(p)), BatteryLevelClass::Low, "at {p}");
        }
        assert_eq!(c.update(Some(25)), BatteryLevelClass::Medium);
        assert_eq!(c.update(Some(22)), BatteryLevelClass::Medium);
    }

    #[test]
    fn critical_band_and_first_reading() {
        let mut c = BatteryClassifier::new();
        assert_eq!(c.update(Some(5)), BatteryLevelClass::Critical);
        assert_eq!(c.update(Some(12)), BatteryLevelClass::Critical);
        assert_eq!(c.update(Some(13)), BatteryLevelClass::Low);
        let mut fresh = BatteryClassifier::new();
        assert_eq!(fresh.update(Some(100)), BatteryLevelClass::Full);
        assert_eq!(fresh.update(Some(200)), BatteryLevelClass::Full, "clamped");
    }

    #[test]
    fn missing_reading_is_unknown_then_reclassifies() {
        let mut c = BatteryClassifier::new();
        c.update(Some(8));
        assert_eq!(c.update(None), BatteryLevelClass::Unknown);
        assert_eq!(c.current(), BatteryLevelClass::Unknown);
        assert_eq!(c.update(Some(8)), BatteryLevelClass::Critical);
    }

    #[test]
    fn thermal_escalates_immediately_and_recovers_one_step_per_dwell() {
        let mut f = ThermalFilter::new(1_000);
        assert_eq!(f.update(ThermalState::Nominal, 0), ThermalState::Nominal);
        assert_eq!(f.update(ThermalState::Critical, 10), ThermalState::Critical);
        // Cooled instantly, but must dwell.
        assert_eq!(f.update(ThermalState::Nominal, 500), ThermalState::Critical);
        assert_eq!(
            f.update(ThermalState::Nominal, 1_010),
            ThermalState::Serious
        );
        assert_eq!(
            f.update(ThermalState::Nominal, 1_500),
            ThermalState::Serious
        );
        assert_eq!(
            f.update(ThermalState::Nominal, 2_020),
            ThermalState::Elevated
        );
        assert_eq!(
            f.update(ThermalState::Nominal, 3_030),
            ThermalState::Nominal
        );
    }

    #[test]
    fn thermal_never_recovers_below_the_raw_reading() {
        let mut f = ThermalFilter::new(10);
        f.update(ThermalState::Serious, 0);
        for t in 1..200 {
            assert!(f.update(ThermalState::Serious, t * 100).severity() >= 2);
        }
    }

    #[test]
    fn sensor_dropout_decays_instead_of_pinning() {
        let mut f = ThermalFilter::new(10);
        f.update(ThermalState::Serious, 0);
        let mut now = 0;
        for _ in 0..5 {
            now += 20;
            f.update(ThermalState::Unknown, now);
        }
        assert_eq!(f.current(), ThermalState::Nominal);
    }

    #[test]
    fn regressing_clock_does_not_recover_early() {
        let mut f = ThermalFilter::new(1_000);
        f.update(ThermalState::Critical, 5_000);
        assert_eq!(f.update(ThermalState::Nominal, 1), ThermalState::Critical);
    }

    proptest! {
        /// Hysteresis invariant: a stable class never changes unless the
        /// reading crossed the corresponding up/down threshold.
        #[test]
        fn class_changes_only_across_thresholds(readings in proptest::collection::vec(0u8..=100, 1..200)) {
            let mut c = BatteryClassifier::new();
            let mut prev = c.update(Some(readings[0]));
            for &p in &readings[1..] {
                let next = c.update(Some(p));
                if next > prev { prop_assert!(p >= UP[band_index(prev).unwrap()]); }
                if next < prev { prop_assert!(p < DOWN[band_index(next).unwrap()]); }
                prev = next;
            }
        }

        #[test]
        fn thermal_filter_output_never_below_raw(
            steps in proptest::collection::vec((0u8..=3, 0u64..5_000), 1..100)
        ) {
            let mut f = ThermalFilter::new(1_000);
            let mut now = 0u64;
            for (sev, dt) in steps {
                now += dt;
                let raw = ThermalState::from_severity(sev);
                let out = f.update(raw, now);
                prop_assert!(out.severity() >= raw.severity());
            }
        }
    }
}
