//! §158 "Recovery of Third-Party Extensions" and §159 "Extension
//! Quarantine".
//!
//! An extension must declare its durable-state version, a recovery
//! hook and its resource cleanup before it is loaded. If its recovery
//! keeps failing, it is disabled and the core starts degraded — a
//! broken plugin never blocks messaging or files. Failure counting
//! reuses the supervisor's windowed [`RestartTracker`], so
//! "repeatedly" has exactly one definition in this crate.

use crate::recovery_mode::RecoverySeverity;
use crate::supervision::{RestartDecision, RestartPolicy, RestartTracker};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtensionId(pub &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criticality {
    /// Nothing core depends on it: safe to disable.
    Optional,
    /// Core behaviour (e.g. a mandatory policy extension) relies on
    /// it: disabling it silently would be unsafe.
    CoreAdjacent,
}

/// §158's three declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionDescriptor {
    pub id: ExtensionId,
    /// `None` = the extension did not declare a durable state version.
    pub durable_state_version: Option<u32>,
    pub has_recovery_hook: bool,
    pub declares_resource_cleanup: bool,
    pub criticality: Criticality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MissingDeclaration {
    #[error("extension did not declare a durable state version")]
    DurableStateVersion,
    #[error("extension has no recovery hook")]
    RecoveryHook,
    #[error("extension did not declare resource cleanup")]
    ResourceCleanup,
}

pub const fn validate_descriptor(d: &ExtensionDescriptor) -> Result<(), MissingDeclaration> {
    if d.durable_state_version.is_none() {
        Err(MissingDeclaration::DurableStateVersion)
    } else if !d.has_recovery_hook {
        Err(MissingDeclaration::RecoveryHook)
    } else if !d.declares_resource_cleanup {
        Err(MissingDeclaration::ResourceCleanup)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionState {
    Active,
    /// Recovery kept failing; quarantined.
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineOutcome {
    /// Try again (with this backoff).
    RetryAfter { delay_millis: u64 },
    /// Disabled; core continues.
    Disabled,
    /// Should be disabled but is core-adjacent: the node can't safely
    /// run as if it were fine.
    CannotDisableSafely,
}

#[derive(Debug, Clone)]
pub struct ExtensionSupervisor {
    policy: RestartPolicy,
    trackers: HashMap<ExtensionId, (RestartTracker, Criticality)>,
}

impl ExtensionSupervisor {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            trackers: HashMap::new(),
        }
    }

    pub fn register(&mut self, d: &ExtensionDescriptor) -> Result<(), MissingDeclaration> {
        validate_descriptor(d)?;
        self.trackers
            .insert(d.id, (RestartTracker::new(self.policy), d.criticality));
        Ok(())
    }

    /// Records a failed recovery attempt of `id`. Other extensions'
    /// trackers are untouched.
    pub fn report_recovery_failure(
        &mut self,
        id: ExtensionId,
        now_millis: u64,
    ) -> QuarantineOutcome {
        let Some((t, crit)) = self.trackers.get_mut(&id) else {
            return QuarantineOutcome::Disabled;
        };
        match t.on_failure(now_millis) {
            RestartDecision::RestartAfter { delay_millis } => {
                QuarantineOutcome::RetryAfter { delay_millis }
            }
            RestartDecision::MarkDegradedAndStop => match crit {
                Criticality::Optional => QuarantineOutcome::Disabled,
                Criticality::CoreAdjacent => QuarantineOutcome::CannotDisableSafely,
            },
        }
    }

    pub fn state(&self, id: ExtensionId) -> Option<ExtensionState> {
        self.trackers.get(&id).map(|(t, _)| {
            if t.is_stopped() {
                ExtensionState::Disabled
            } else {
                ExtensionState::Active
            }
        })
    }

    pub fn disabled(&self) -> Vec<ExtensionId> {
        let mut v: Vec<_> = self
            .trackers
            .iter()
            .filter(|(_, (t, _))| t.is_stopped())
            .map(|(id, _)| *id)
            .collect();
        v.sort();
        v
    }

    /// Startup plan: the core always starts. Disabled optional
    /// extensions make the start `Degraded`; a disabled core-adjacent
    /// one makes it `ReadOnly`.
    pub fn start_plan(&self) -> ExtensionStartPlan {
        let mut severity = RecoverySeverity::Clean;
        for (t, crit) in self.trackers.values() {
            if t.is_stopped() {
                severity = severity.max(match crit {
                    Criticality::Optional => RecoverySeverity::Degraded,
                    Criticality::CoreAdjacent => RecoverySeverity::ReadOnly,
                });
            }
        }
        ExtensionStartPlan {
            core_may_start: true,
            disabled: self.disabled(),
            severity,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionStartPlan {
    /// Always true: no extension can veto the core starting.
    pub core_may_start: bool,
    pub disabled: Vec<ExtensionId>,
    pub severity: RecoverySeverity,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(id: &'static str, crit: Criticality) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: ExtensionId(id),
            durable_state_version: Some(1),
            has_recovery_hook: true,
            declares_resource_cleanup: true,
            criticality: crit,
        }
    }

    fn policy() -> RestartPolicy {
        RestartPolicy {
            base_delay_millis: 100,
            max_delay_millis: 1_000,
            max_failures_in_window: 3,
            window_millis: 10_000,
        }
    }

    #[test]
    fn extensions_must_declare_all_three_things() {
        let mut d = desc("a", Criticality::Optional);
        assert_eq!(validate_descriptor(&d), Ok(()));
        d.durable_state_version = None;
        assert_eq!(
            validate_descriptor(&d),
            Err(MissingDeclaration::DurableStateVersion)
        );
        let mut d = desc("a", Criticality::Optional);
        d.has_recovery_hook = false;
        assert_eq!(
            validate_descriptor(&d),
            Err(MissingDeclaration::RecoveryHook)
        );
        let mut d = desc("a", Criticality::Optional);
        d.declares_resource_cleanup = false;
        assert_eq!(
            validate_descriptor(&d),
            Err(MissingDeclaration::ResourceCleanup)
        );
    }

    #[test]
    fn an_undeclared_extension_is_refused_registration() {
        let mut s = ExtensionSupervisor::new(policy());
        let mut d = desc("bad", Criticality::Optional);
        d.has_recovery_hook = false;
        assert!(s.register(&d).is_err());
        assert_eq!(s.state(ExtensionId("bad")), None);
    }

    #[test]
    fn repeated_failure_disables_an_optional_extension() {
        let mut s = ExtensionSupervisor::new(policy());
        s.register(&desc("plugin", Criticality::Optional)).unwrap();
        let id = ExtensionId("plugin");
        assert!(matches!(
            s.report_recovery_failure(id, 0),
            QuarantineOutcome::RetryAfter { .. }
        ));
        assert!(matches!(
            s.report_recovery_failure(id, 1),
            QuarantineOutcome::RetryAfter { .. }
        ));
        assert_eq!(
            s.report_recovery_failure(id, 2),
            QuarantineOutcome::Disabled
        );
        assert_eq!(s.state(id), Some(ExtensionState::Disabled));
    }

    #[test]
    fn a_broken_plugin_never_blocks_the_core() {
        let mut s = ExtensionSupervisor::new(policy());
        s.register(&desc("plugin", Criticality::Optional)).unwrap();
        for t in 0..3 {
            s.report_recovery_failure(ExtensionId("plugin"), t);
        }
        let plan = s.start_plan();
        assert!(plan.core_may_start);
        assert_eq!(plan.disabled, vec![ExtensionId("plugin")]);
        assert_eq!(plan.severity, RecoverySeverity::Degraded);
    }

    #[test]
    fn one_extensions_failures_do_not_disable_another() {
        let mut s = ExtensionSupervisor::new(policy());
        s.register(&desc("bad", Criticality::Optional)).unwrap();
        s.register(&desc("good", Criticality::Optional)).unwrap();
        for t in 0..3 {
            s.report_recovery_failure(ExtensionId("bad"), t);
        }
        assert_eq!(s.state(ExtensionId("good")), Some(ExtensionState::Active));
        assert_eq!(s.disabled(), vec![ExtensionId("bad")]);
    }

    #[test]
    fn a_core_adjacent_extension_cannot_be_silently_disabled() {
        let mut s = ExtensionSupervisor::new(policy());
        s.register(&desc("policy-ext", Criticality::CoreAdjacent))
            .unwrap();
        let id = ExtensionId("policy-ext");
        s.report_recovery_failure(id, 0);
        s.report_recovery_failure(id, 1);
        assert_eq!(
            s.report_recovery_failure(id, 2),
            QuarantineOutcome::CannotDisableSafely
        );
        let plan = s.start_plan();
        assert!(plan.core_may_start, "core still starts…");
        assert_eq!(
            plan.severity,
            RecoverySeverity::ReadOnly,
            "…but not as if nothing happened"
        );
    }

    #[test]
    fn healthy_extensions_leave_the_start_clean() {
        let mut s = ExtensionSupervisor::new(policy());
        s.register(&desc("a", Criticality::Optional)).unwrap();
        let plan = s.start_plan();
        assert_eq!(plan.severity, RecoverySeverity::Clean);
        assert!(plan.disabled.is_empty());
    }

    #[test]
    fn unknown_extension_failures_are_treated_as_disabled() {
        let mut s = ExtensionSupervisor::new(policy());
        assert_eq!(
            s.report_recovery_failure(ExtensionId("ghost"), 0),
            QuarantineOutcome::Disabled
        );
    }
}
