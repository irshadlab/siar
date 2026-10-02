//! §81 pressure state, §83/§87 pressure response, §101-104 load shedding
//! and the overload / critical-only modes.
//!
//! Pressure is reported per resource ([`PressureSignals`]); the worst
//! signal selects an [`OverloadMode`]. Escalation is immediate (shed
//! early), de-escalation is one step at a time and only after the
//! signals have stayed calmer for `min_calm_millis` — otherwise a signal
//! hovering at a threshold would flap the whole runtime between modes.
//! Shedding is by priority cutoff, so Critical (SOS, calls' signaling)
//! and Control (acks, which *release* pressure) survive longest.

use crate::admission::WorkPriority;
use crate::storage::PressureState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PressureSignals {
    pub memory: PressureState,
    pub cpu: PressureState,
    pub storage: PressureState,
    pub thermal: PressureState,
    pub energy: PressureState,
}

impl PressureSignals {
    pub fn worst(&self) -> PressureState {
        self.memory
            .max(self.cpu)
            .max(self.storage)
            .max(self.thermal)
            .max(self.energy)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum OverloadMode {
    #[default]
    Normal,
    /// Elevated pressure: background work is shed.
    Shedding,
    /// Critical pressure: only interactive-or-better is admitted.
    Overload,
    /// Exhausted: Critical + Control only (§104).
    CriticalOnly,
}

impl OverloadMode {
    fn for_pressure(p: PressureState) -> Self {
        match p {
            PressureState::Normal => Self::Normal,
            PressureState::Elevated => Self::Shedding,
            PressureState::Critical => Self::Overload,
            PressureState::Exhausted => Self::CriticalOnly,
        }
    }

    fn eased(self) -> Self {
        match self {
            Self::CriticalOnly => Self::Overload,
            Self::Overload => Self::Shedding,
            Self::Shedding | Self::Normal => Self::Normal,
        }
    }

    /// Least important priority still admitted in this mode.
    pub fn cutoff(self) -> WorkPriority {
        match self {
            Self::Normal => WorkPriority::Background,
            Self::Shedding => WorkPriority::Bulk,
            Self::Overload => WorkPriority::Interactive,
            Self::CriticalOnly => WorkPriority::Control,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShedDecision {
    Admit,
    /// Durable work: keep it, retry when pressure eases (§22).
    Defer,
    /// Ephemeral work: not worth keeping (§22, §101).
    Drop,
}

/// §101-104.
pub fn shed(mode: OverloadMode, priority: WorkPriority, durable: bool) -> ShedDecision {
    if priority <= mode.cutoff() {
        ShedDecision::Admit
    } else if durable {
        ShedDecision::Defer
    } else {
        ShedDecision::Drop
    }
}

#[derive(Debug)]
pub struct ModeController {
    mode: OverloadMode,
    calm_since_millis: Option<u64>,
    min_calm_millis: u64,
}

impl ModeController {
    pub fn new(min_calm_millis: u64) -> Self {
        Self {
            mode: OverloadMode::Normal,
            calm_since_millis: None,
            min_calm_millis,
        }
    }

    pub fn mode(&self) -> OverloadMode {
        self.mode
    }

    pub fn observe(&mut self, signals: &PressureSignals, now_millis: u64) -> OverloadMode {
        let target = OverloadMode::for_pressure(signals.worst());
        if target >= self.mode {
            self.mode = target;
            self.calm_since_millis = None;
            return self.mode;
        }
        let since = *self.calm_since_millis.get_or_insert(now_millis);
        if now_millis.saturating_sub(since) >= self.min_calm_millis {
            self.mode = self.mode.eased().max(target);
            // Restart the calm window for the next step down.
            self.calm_since_millis = (target < self.mode).then_some(now_millis);
        }
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(mem: PressureState) -> PressureSignals {
        PressureSignals {
            memory: mem,
            ..Default::default()
        }
    }

    #[test]
    fn worst_signal_wins_and_escalation_is_immediate() {
        let mut c = ModeController::new(1_000);
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 0),
            OverloadMode::Normal
        );
        assert_eq!(
            c.observe(&sig(PressureState::Exhausted), 1),
            OverloadMode::CriticalOnly
        );
        let mixed = PressureSignals {
            thermal: PressureState::Critical,
            storage: PressureState::Elevated,
            ..Default::default()
        };
        assert_eq!(mixed.worst(), PressureState::Critical);
    }

    #[test]
    fn deescalation_is_stepwise_and_needs_a_calm_window() {
        let mut c = ModeController::new(1_000);
        c.observe(&sig(PressureState::Exhausted), 0);
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 10),
            OverloadMode::CriticalOnly
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 500),
            OverloadMode::CriticalOnly
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 1_010),
            OverloadMode::Overload
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 1_500),
            OverloadMode::Overload
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 2_010),
            OverloadMode::Shedding
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 3_010),
            OverloadMode::Normal
        );
    }

    #[test]
    fn a_spike_during_the_calm_window_resets_it() {
        let mut c = ModeController::new(1_000);
        c.observe(&sig(PressureState::Critical), 0);
        c.observe(&sig(PressureState::Normal), 100);
        c.observe(&sig(PressureState::Critical), 900); // not calm
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 1_200),
            OverloadMode::Overload
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 2_000),
            OverloadMode::Overload
        );
        assert_eq!(
            c.observe(&sig(PressureState::Normal), 2_300),
            OverloadMode::Shedding
        );
    }

    #[test]
    fn easing_never_drops_below_the_current_target() {
        let mut c = ModeController::new(0);
        c.observe(&sig(PressureState::Exhausted), 0);
        // Pressure only eased to Critical: must not ease past Overload.
        assert_eq!(
            c.observe(&sig(PressureState::Critical), 1),
            OverloadMode::Overload
        );
        assert_eq!(
            c.observe(&sig(PressureState::Critical), 2),
            OverloadMode::Overload
        );
    }

    #[test]
    fn shed_by_priority_cutoff_and_durability() {
        use siar_protocol_ext::TrafficPriority::*;
        assert_eq!(
            shed(OverloadMode::Normal, Background, false),
            ShedDecision::Admit
        );
        assert_eq!(
            shed(OverloadMode::Shedding, Background, false),
            ShedDecision::Drop
        );
        assert_eq!(
            shed(OverloadMode::Shedding, Background, true),
            ShedDecision::Defer
        );
        assert_eq!(
            shed(OverloadMode::Overload, Normal, true),
            ShedDecision::Defer
        );
        assert_eq!(
            shed(OverloadMode::Overload, Interactive, false),
            ShedDecision::Admit
        );
        assert_eq!(
            shed(OverloadMode::CriticalOnly, Interactive, false),
            ShedDecision::Drop
        );
        assert_eq!(
            shed(OverloadMode::CriticalOnly, Control, false),
            ShedDecision::Admit
        );
        assert_eq!(
            shed(OverloadMode::CriticalOnly, Critical, false),
            ShedDecision::Admit
        );
    }
}
