//! §82 memory-pressure sources, §84 Android and §85 iOS integration.
//!
//! The platform adapters (Kotlin / Swift) report **raw facts only** — the
//! trim-memory integer, "a warning happened". All policy lives here, in
//! Rust, once (§84: "do not duplicate resource policy in Kotlin"). Reports
//! from the four sources are merged into the single
//! [`crate::storage::PressureState`] the runtime reacts to.
//!
//! Platform signals are edge-triggered (a warning arrives; no "all clear"
//! ever does), so each report is *held* for a while and then decays back
//! to Normal unless refreshed.

use crate::pressure::PressureSignals;
use crate::storage::PressureState;

/// Android `ComponentCallbacks2.onTrimMemory` levels →
/// pressure. Foreground "RUNNING_*" levels (5/10/15) mean the system is
/// short on memory *now*; background levels (40/60/80) mean our process is
/// moving toward the kill list. 20 (`UI_HIDDEN`) is not memory pressure —
/// it is the cue to drop UI-only caches, which the caller handles.
/// Unknown values are bucketed by range so a future level can't be
/// misread as "no pressure".
pub fn android_trim_level(level: i32) -> PressureState {
    match level {
        i32::MIN..=4 => PressureState::Normal,
        5..=14 => PressureState::Elevated, // RUNNING_MODERATE, RUNNING_LOW
        15..=19 => PressureState::Critical, // RUNNING_CRITICAL
        20..=39 => PressureState::Normal,  // UI_HIDDEN
        40..=59 => PressureState::Elevated, // BACKGROUND
        60..=79 => PressureState::Critical, // MODERATE
        80..=i32::MAX => PressureState::Exhausted, // COMPLETE
    }
}

/// iOS `didReceiveMemoryWarning`: the system is about to start
/// terminating processes — treated as Critical.
pub fn ios_memory_warning() -> PressureState {
    PressureState::Critical
}

/// Internal budget exhaustion (§82's fourth source) from usage / budget.
pub fn budget_pressure(used: u64, budget: u64) -> PressureState {
    if budget == 0 {
        return PressureState::Exhausted;
    }
    // used/budget in permille without overflow
    let permille = (u128::from(used) * 1000 / u128::from(budget)).min(u128::from(u32::MAX)) as u32;
    match permille {
        0..=699 => PressureState::Normal,
        700..=849 => PressureState::Elevated,
        850..=949 => PressureState::Critical,
        _ => PressureState::Exhausted,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    AndroidTrim,
    IosWarning,
    OsLowMemory,
    BudgetExhaustion,
}

impl MemorySource {
    const COUNT: usize = 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldTimes {
    pub android_trim_millis: u64,
    pub ios_warning_millis: u64,
    pub os_low_memory_millis: u64,
    /// Level-triggered and re-sampled often, so short.
    pub budget_millis: u64,
}

impl Default for HoldTimes {
    fn default() -> Self {
        Self {
            android_trim_millis: 60_000,
            ios_warning_millis: 30_000,
            os_low_memory_millis: 30_000,
            budget_millis: 5_000,
        }
    }
}

#[derive(Debug)]
pub struct MemoryPressureAggregator {
    holds: HoldTimes,
    // (state, held until)
    slots: [Option<(PressureState, u64)>; MemorySource::COUNT],
}

impl MemoryPressureAggregator {
    pub fn new(holds: HoldTimes) -> Self {
        Self {
            holds,
            slots: [None; MemorySource::COUNT],
        }
    }

    fn hold_for(&self, s: MemorySource) -> u64 {
        match s {
            MemorySource::AndroidTrim => self.holds.android_trim_millis,
            MemorySource::IosWarning => self.holds.ios_warning_millis,
            MemorySource::OsLowMemory => self.holds.os_low_memory_millis,
            MemorySource::BudgetExhaustion => self.holds.budget_millis,
        }
    }

    /// The latest report from a source replaces its previous one (a lower
    /// reading is a genuine improvement signal from that source).
    pub fn report(&mut self, source: MemorySource, state: PressureState, now_millis: u64) {
        let until = now_millis.saturating_add(self.hold_for(source));
        self.slots[source as usize] = Some((state, until));
    }

    pub fn report_android_trim(&mut self, level: i32, now_millis: u64) {
        self.report(
            MemorySource::AndroidTrim,
            android_trim_level(level),
            now_millis,
        );
    }

    pub fn report_ios_warning(&mut self, now_millis: u64) {
        self.report(MemorySource::IosWarning, ios_memory_warning(), now_millis);
    }

    pub fn report_budget(&mut self, used: u64, budget: u64, now_millis: u64) {
        self.report(
            MemorySource::BudgetExhaustion,
            budget_pressure(used, budget),
            now_millis,
        );
    }

    /// Worst non-expired report across sources.
    pub fn current(&self, now_millis: u64) -> PressureState {
        self.slots
            .iter()
            .flatten()
            .filter(|(_, until)| now_millis < *until)
            .map(|(s, _)| *s)
            .max()
            .unwrap_or_default()
    }

    /// Feed the merged memory reading into the runtime's signal set.
    pub fn apply_to(&self, signals: PressureSignals, now_millis: u64) -> PressureSignals {
        PressureSignals {
            memory: self.current(now_millis),
            ..signals
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PressureState::*;

    #[test]
    fn android_levels_map_per_documented_semantics() {
        for (lvl, want) in [
            (5, Elevated),   // RUNNING_MODERATE
            (10, Elevated),  // RUNNING_LOW
            (15, Critical),  // RUNNING_CRITICAL
            (20, Normal),    // UI_HIDDEN is not memory pressure
            (40, Elevated),  // BACKGROUND
            (60, Critical),  // MODERATE
            (80, Exhausted), // COMPLETE
        ] {
            assert_eq!(android_trim_level(lvl), want, "level {lvl}");
        }
    }

    #[test]
    fn unknown_and_extreme_levels_are_bucketed_safely() {
        assert_eq!(android_trim_level(0), Normal);
        assert_eq!(android_trim_level(-5), Normal);
        assert_eq!(android_trim_level(7), Elevated);
        assert_eq!(
            android_trim_level(100),
            Exhausted,
            "a future higher level reads as worse, not as nothing"
        );
        assert_eq!(android_trim_level(i32::MAX), Exhausted);
    }

    #[test]
    fn trim_severity_is_monotonic_within_each_family() {
        let fg: Vec<_> = [5, 10, 15].map(android_trim_level).to_vec();
        let bg: Vec<_> = [20, 40, 60, 80].map(android_trim_level).to_vec();
        assert!(fg.windows(2).all(|w| w[0] <= w[1]));
        assert!(bg.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn budget_pressure_bands_and_degenerate_budget() {
        assert_eq!(budget_pressure(0, 100), Normal);
        assert_eq!(budget_pressure(70, 100), Elevated);
        assert_eq!(budget_pressure(85, 100), Critical);
        assert_eq!(budget_pressure(95, 100), Exhausted);
        assert_eq!(budget_pressure(1, 0), Exhausted);
        assert_eq!(
            budget_pressure(u64::MAX, u64::MAX),
            Exhausted,
            "no overflow"
        );
    }

    #[test]
    fn edge_triggered_reports_hold_then_decay_to_normal() {
        let mut a = MemoryPressureAggregator::new(HoldTimes::default());
        assert_eq!(a.current(0), Normal);
        a.report_ios_warning(1_000);
        assert_eq!(a.current(30_999), Critical);
        assert_eq!(
            a.current(31_000),
            Normal,
            "no all-clear is ever sent; it decays"
        );
    }

    #[test]
    fn sources_merge_by_worst_and_each_source_can_improve() {
        let mut a = MemoryPressureAggregator::new(HoldTimes::default());
        a.report_android_trim(80, 0); // Exhausted for 60s
        a.report_budget(10, 100, 0); // Normal
        assert_eq!(a.current(1_000), Exhausted);
        a.report_android_trim(5, 2_000); // same source improves
        assert_eq!(a.current(2_001), Elevated);
        a.report_budget(90, 100, 2_100);
        assert_eq!(a.current(2_200), Critical);
        assert_eq!(
            a.current(8_000),
            Elevated,
            "budget hold (5s) lapsed; trim hold remains"
        );
    }

    #[test]
    fn feeds_the_overload_controller() {
        use crate::pressure::{ModeController, OverloadMode};
        let mut a = MemoryPressureAggregator::new(HoldTimes::default());
        let mut c = ModeController::new(1_000);
        a.report_android_trim(80, 0);
        let sig = a.apply_to(PressureSignals::default(), 1);
        assert_eq!(c.observe(&sig, 1), OverloadMode::CriticalOnly);
        // the app is told nothing more; after the hold the signal clears
        let calm = a.apply_to(PressureSignals::default(), 70_000);
        assert_eq!(calm.memory, Normal);
    }
}
