//! §89-§90, §140-§141: per-subsystem health.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubsystemHealth {
    pub name: String,
    pub state: HealthState,
    /// Short, non-secret reason ("restarting 3/5", "storage read-only").
    pub detail: String,
    /// Failing this makes the whole runtime `Degraded`; non-critical failure
    /// only degrades that subsystem (§140).
    pub critical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub overall: HealthState,
    pub subsystems: Vec<SubsystemHealth>,
}

/// Shared registry; cloning shares state.
#[derive(Debug, Clone, Default)]
pub struct HealthRegistry {
    inner: Arc<Mutex<BTreeMap<String, SubsystemHealth>>>,
}

impl HealthRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, SubsystemHealth>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set(&self, name: &str, state: HealthState, detail: impl Into<String>, critical: bool) {
        self.lock().insert(
            name.to_owned(),
            SubsystemHealth {
                name: name.to_owned(),
                state,
                detail: detail.into(),
                critical,
            },
        );
    }

    pub fn remove(&self, name: &str) {
        self.lock().remove(name);
    }

    #[must_use]
    pub fn report(&self) -> HealthReport {
        let subsystems: Vec<_> = self.lock().values().cloned().collect();
        let overall = subsystems.iter().fold(HealthState::Healthy, |acc, s| {
            // A non-critical failure is at worst "Degraded" for the whole.
            let contrib = if s.critical {
                s.state
            } else {
                s.state.min(HealthState::Degraded)
            };
            acc.max(contrib)
        });
        HealthReport {
            overall,
            subsystems,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noncritical_failure_only_degrades() {
        let h = HealthRegistry::new();
        h.set("storage", HealthState::Healthy, "", true);
        h.set("thumbnails", HealthState::Unavailable, "crashed", false);
        assert_eq!(h.report().overall, HealthState::Degraded);
    }

    #[test]
    fn critical_failure_is_unavailable() {
        let h = HealthRegistry::new();
        h.set("storage", HealthState::Unavailable, "disk", true);
        assert_eq!(h.report().overall, HealthState::Unavailable);
        h.set("storage", HealthState::Healthy, "", true);
        assert_eq!(h.report().overall, HealthState::Healthy);
        h.remove("storage");
        assert!(h.report().subsystems.is_empty());
    }
}
