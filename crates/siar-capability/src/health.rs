//! §44 "Codec Runtime Failure", §108-111 lifecycle/invalidation/health.
//!
//! A capability can be *negotiated* yet fail at runtime (a hardware codec
//! that errors, an extension that crashes). Health is local, ephemeral
//! state that gates selection — it never changes what was advertised.
//!
//! State machine (half-open breaker):
//! `Healthy → Suspended(until)` after `failure_threshold` consecutive
//! failures; when the cooldown lapses the capability is `Probing` (one
//! trial allowed); success → `Healthy`, failure → `Suspended` again with
//! a doubled cooldown (capped).

use std::collections::HashMap;

use crate::id::CapabilityId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    /// Cooldown elapsed; the next use is a trial.
    Probing,
    Suspended {
        until_millis: u64,
    },
}

impl Health {
    pub fn is_usable(self) -> bool {
        !matches!(self, Self::Suspended { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPolicy {
    pub failure_threshold: u32,
    pub base_cooldown_millis: u64,
    pub max_cooldown_millis: u64,
    pub max_entries: usize,
}

impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            base_cooldown_millis: 30_000,
            max_cooldown_millis: 30 * 60_000,
            max_entries: 1024,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    consecutive_failures: u32,
    suspensions: u32,
    suspended_until: Option<u64>,
}

#[derive(Debug)]
pub struct HealthTracker {
    policy: HealthPolicy,
    entries: HashMap<CapabilityId, Entry>,
}

impl HealthTracker {
    pub fn new(policy: HealthPolicy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
        }
    }

    pub fn status(&self, id: &CapabilityId, now_millis: u64) -> Health {
        match self.entries.get(id).and_then(|e| e.suspended_until) {
            None => Health::Healthy,
            Some(until) if now_millis < until => Health::Suspended {
                until_millis: until,
            },
            Some(_) => Health::Probing,
        }
    }

    pub fn record_success(&mut self, id: &CapabilityId) {
        self.entries.remove(id);
    }

    pub fn record_failure(&mut self, id: CapabilityId, now_millis: u64) -> Health {
        if !self.entries.contains_key(&id) && self.entries.len() >= self.policy.max_entries {
            // Bounded: forget the healthiest (fewest failures) entry.
            if let Some(k) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| (e.suspended_until.is_some(), e.consecutive_failures))
                .map(|(k, _)| *k)
            {
                self.entries.remove(&k);
            }
        }
        let was_probing = matches!(self.status(&id, now_millis), Health::Probing);
        let e = self.entries.entry(id).or_insert(Entry {
            consecutive_failures: 0,
            suspensions: 0,
            suspended_until: None,
        });
        e.consecutive_failures = e.consecutive_failures.saturating_add(1);
        if was_probing || e.consecutive_failures >= self.policy.failure_threshold {
            let shift = e.suspensions.min(20);
            let cooldown = self
                .policy
                .base_cooldown_millis
                .saturating_mul(1u64 << shift)
                .min(self.policy.max_cooldown_millis);
            e.suspensions = e.suspensions.saturating_add(1);
            e.suspended_until = Some(now_millis.saturating_add(cooldown));
            e.consecutive_failures = 0;
        }
        self.status(&id, now_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::CapabilityNamespace;

    const ID: CapabilityId = CapabilityId::new(CapabilityNamespace::Media, 1);

    fn t() -> HealthTracker {
        HealthTracker::new(HealthPolicy {
            failure_threshold: 2,
            base_cooldown_millis: 100,
            max_cooldown_millis: 350,
            max_entries: 2,
        })
    }

    #[test]
    fn suspends_after_threshold_then_probes_after_cooldown() {
        let mut h = t();
        assert_eq!(h.record_failure(ID, 0), Health::Healthy);
        assert_eq!(
            h.record_failure(ID, 1),
            Health::Suspended { until_millis: 101 }
        );
        assert!(!h.status(&ID, 50).is_usable());
        assert_eq!(h.status(&ID, 101), Health::Probing);
        assert!(h.status(&ID, 101).is_usable());
    }

    #[test]
    fn probe_failure_re_suspends_with_doubled_capped_cooldown() {
        let mut h = t();
        h.record_failure(ID, 0);
        h.record_failure(ID, 0); // until 100
        assert_eq!(
            h.record_failure(ID, 100),
            Health::Suspended { until_millis: 300 }
        );
        assert_eq!(
            h.record_failure(ID, 300),
            Health::Suspended { until_millis: 650 }
        ); // capped at 350
    }

    #[test]
    fn success_fully_heals() {
        let mut h = t();
        h.record_failure(ID, 0);
        h.record_failure(ID, 0);
        h.record_success(&ID);
        assert_eq!(h.status(&ID, 1), Health::Healthy);
    }

    #[test]
    fn tracker_is_bounded() {
        let mut h = t();
        for c in 0..10 {
            h.record_failure(CapabilityId::new(CapabilityNamespace::Media, c), 0);
        }
        assert!(h.entries.len() <= 2);
    }
}
