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
        Self::summarise(subsystems)
    }

    /// Like [`report`](Self::report) but never blocks: `None` if another
    /// thread (possibly the one that is panicking) holds the registry.
    /// For crash handlers (§96), which must not wait on anything.
    #[must_use]
    pub fn try_report(&self) -> Option<HealthReport> {
        let guard = match self.inner.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let subsystems: Vec<_> = guard.values().cloned().collect();
        drop(guard);
        Some(Self::summarise(subsystems))
    }

    fn summarise(subsystems: Vec<SubsystemHealth>) -> HealthReport {
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
    fn try_report_never_blocks_and_matches_report_when_free() {
        let h = HealthRegistry::new();
        h.set("storage", HealthState::Healthy, "", true);
        assert_eq!(h.try_report(), Some(h.report()));
        // Held elsewhere (as it would be by a thread that panicked inside
        // `set`): the crash handler gets `None` instead of deadlocking.
        let guard = h.inner.lock().unwrap();
        assert_eq!(h.try_report(), None);
        drop(guard);
        assert!(h.try_report().is_some());
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
