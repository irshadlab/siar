//! §47 "Read-Only Recovery Mode", §48 "Degraded Recovery Mode", §49
//! "Fatal Recovery Mode", §50 "Recovery Classification".
//!
//! [`RecoverySeverity`] is §50's enum verbatim. Each severity maps to
//! a fixed [`Capabilities`] set, and every operation a caller wants to
//! perform goes through [`gate`] — so "disallow new durable
//! operations" (§47) and "do not start network operations" (§49) are
//! one check at the call site, not a convention.

use crate::state_machine::RecoveryState;
use serde::{Deserialize, Serialize};

/// §50, verbatim. Declaration order is severity order, so `max()`
/// over several findings gives the overall outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RecoverySeverity {
    Clean,
    Repaired,
    Degraded,
    ReadOnly,
    Fatal,
}

/// Things a caller may try to do once recovery has finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    ViewMessages,
    ExportData,
    InspectDevices,
    Diagnostics,
    /// Anything that appends/updates durable state.
    DurableWrite,
    /// Starting network operations (sessions, sends, DTN encounters).
    StartNetwork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub view_messages: bool,
    pub export_data: bool,
    pub inspect_devices: bool,
    pub diagnostics: bool,
    pub durable_write: bool,
    pub start_network: bool,
}

/// Severity → what is allowed.
///
/// * `Clean`/`Repaired`/`Degraded` — everything (degraded surfaces
///   diagnostics but runs, §48).
/// * `ReadOnly` — §47's four read capabilities only. Network is also
///   off: accepting inbound traffic would require durable writes.
/// * `Fatal` — diagnostics only; §49's "do not start network
///   operations that could make state worse", plus nothing that
///   touches a store that may be unrecoverable.
pub const fn capabilities(s: RecoverySeverity) -> Capabilities {
    match s {
        RecoverySeverity::Clean | RecoverySeverity::Repaired | RecoverySeverity::Degraded => {
            Capabilities {
                view_messages: true,
                export_data: true,
                inspect_devices: true,
                diagnostics: true,
                durable_write: true,
                start_network: true,
            }
        }
        RecoverySeverity::ReadOnly => Capabilities {
            view_messages: true,
            export_data: true,
            inspect_devices: true,
            diagnostics: true,
            durable_write: false,
            start_network: false,
        },
        RecoverySeverity::Fatal => Capabilities {
            view_messages: false,
            export_data: false,
            inspect_devices: false,
            diagnostics: true,
            durable_write: false,
            start_network: false,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{op:?} is not permitted in {severity:?} recovery mode")]
pub struct OperationDenied {
    pub op: Operation,
    pub severity: RecoverySeverity,
}

/// The single gate. Callers do `gate(sev, Operation::DurableWrite)?`.
pub const fn gate(severity: RecoverySeverity, op: Operation) -> Result<(), OperationDenied> {
    let c = capabilities(severity);
    let ok = match op {
        Operation::ViewMessages => c.view_messages,
        Operation::ExportData => c.export_data,
        Operation::InspectDevices => c.inspect_devices,
        Operation::Diagnostics => c.diagnostics,
        Operation::DurableWrite => c.durable_write,
        Operation::StartNetwork => c.start_network,
    };
    if ok {
        Ok(())
    } else {
        Err(OperationDenied { op, severity })
    }
}

/// §48's examples of noncritical subsystems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradedSubsystem {
    Search,
    ThumbnailCache,
    Other(&'static str),
}

/// Folds findings into one severity (the worst wins) and remembers
/// which noncritical subsystems are down, so the runtime can start
/// degraded and *surface* it (§48).
#[derive(Debug, Clone, Default)]
pub struct SeverityAccumulator {
    worst: Option<RecoverySeverity>,
    degraded: Vec<DegradedSubsystem>,
}

impl SeverityAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, s: RecoverySeverity) {
        self.worst = Some(self.worst.map_or(s, |w| w.max(s)));
    }

    pub fn record_degraded(&mut self, sub: DegradedSubsystem) {
        if !self.degraded.contains(&sub) {
            self.degraded.push(sub);
        }
        self.record(RecoverySeverity::Degraded);
    }

    pub fn severity(&self) -> RecoverySeverity {
        self.worst.unwrap_or(RecoverySeverity::Clean)
    }

    pub fn degraded_subsystems(&self) -> &[DegradedSubsystem] {
        &self.degraded
    }
}

/// The §8 state a given severity ends in. `Fatal` maps to
/// `FatalCorruption`; callers with a more specific cause
/// (`MigrationFailed`, `StorageUnavailable`) pick that state directly.
pub const fn terminal_state_for(s: RecoverySeverity) -> RecoveryState {
    match s {
        RecoverySeverity::Clean | RecoverySeverity::Repaired | RecoverySeverity::Degraded => {
            RecoveryState::Recovered
        }
        RecoverySeverity::ReadOnly => RecoveryState::ReadOnlyRecovery,
        RecoverySeverity::Fatal => RecoveryState::FatalCorruption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Operation::*;
    use RecoverySeverity::*;

    #[test]
    fn severity_order_is_the_spec_order() {
        assert!(Clean < Repaired && Repaired < Degraded && Degraded < ReadOnly && ReadOnly < Fatal);
    }

    #[test]
    fn read_only_allows_exactly_the_four_read_capabilities() {
        for op in [ViewMessages, ExportData, InspectDevices, Diagnostics] {
            assert!(gate(ReadOnly, op).is_ok(), "{op:?}");
        }
        for op in [DurableWrite, StartNetwork] {
            assert_eq!(
                gate(ReadOnly, op),
                Err(OperationDenied {
                    op,
                    severity: ReadOnly
                })
            );
        }
    }

    #[test]
    fn degraded_runs_everything() {
        for op in [
            ViewMessages,
            ExportData,
            InspectDevices,
            Diagnostics,
            DurableWrite,
            StartNetwork,
        ] {
            assert!(gate(Degraded, op).is_ok());
        }
    }

    #[test]
    fn fatal_never_starts_network_or_writes() {
        assert!(gate(Fatal, StartNetwork).is_err());
        assert!(gate(Fatal, DurableWrite).is_err());
        assert!(gate(Fatal, Diagnostics).is_ok());
    }

    #[test]
    fn capabilities_never_widen_as_severity_rises() {
        let count = |c: Capabilities| {
            [
                c.view_messages,
                c.export_data,
                c.inspect_devices,
                c.diagnostics,
                c.durable_write,
                c.start_network,
            ]
            .iter()
            .filter(|b| **b)
            .count()
        };
        let all = [Clean, Repaired, Degraded, ReadOnly, Fatal];
        for w in all.windows(2) {
            assert!(count(capabilities(w[0])) >= count(capabilities(w[1])));
        }
    }

    #[test]
    fn accumulator_takes_the_worst_and_defaults_clean() {
        let mut a = SeverityAccumulator::new();
        assert_eq!(a.severity(), Clean);
        a.record(Repaired);
        a.record(ReadOnly);
        a.record(Degraded);
        assert_eq!(a.severity(), ReadOnly);
    }

    #[test]
    fn degraded_subsystems_are_listed_once_and_raise_severity() {
        let mut a = SeverityAccumulator::new();
        a.record_degraded(DegradedSubsystem::Search);
        a.record_degraded(DegradedSubsystem::Search);
        a.record_degraded(DegradedSubsystem::ThumbnailCache);
        assert_eq!(a.degraded_subsystems().len(), 2);
        assert_eq!(a.severity(), Degraded);
    }

    #[test]
    fn terminal_states_are_valid_machine_targets() {
        use crate::state_machine::RecoveryStateMachine;
        let mut m = RecoveryStateMachine::new();
        m.transition(RecoveryState::StorageOpened).unwrap();
        m.transition(RecoveryState::IntegrityChecked).unwrap();
        assert!(m.transition(terminal_state_for(Fatal)).is_ok());

        let mut m = RecoveryStateMachine::new();
        for s in [
            RecoveryState::StorageOpened,
            RecoveryState::IntegrityChecked,
            RecoveryState::Reconciling,
        ] {
            m.transition(s).unwrap();
        }
        assert!(m.transition(terminal_state_for(ReadOnly)).is_ok());
    }
}
