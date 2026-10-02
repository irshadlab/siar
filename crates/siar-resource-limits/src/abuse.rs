//! §132 abuse escalation, §133-134 queue-poisoning protection.

use std::collections::HashMap;
use std::hash::Hash;

use crate::admission::WorkPriority;
use crate::peer_quota::TrustClass;

/// §133-134: a peer must not fill a high-priority queue with fake
/// critical work. A *claimed* priority is clamped to what the peer's
/// trust class may use. `emergency_attested` is set only by the emergency
/// subsystem after it verified a signed SOS — it is the one path by which
/// an otherwise-untrusted source may use `Critical`, and it is never
/// derived from anything the peer merely asserted.
pub fn authorize_priority(
    trust: TrustClass,
    claimed: WorkPriority,
    emergency_attested: bool,
) -> WorkPriority {
    if emergency_attested {
        return claimed;
    }
    let best_allowed = match trust {
        TrustClass::Unknown => WorkPriority::Normal,
        TrustClass::Known | TrustClass::Verified => WorkPriority::Interactive,
        TrustClass::Organization => WorkPriority::Control,
        TrustClass::Authority | TrustClass::LocalOwnDevice => WorkPriority::Critical,
    };
    // Larger = less important, so the less-important of the two wins.
    claimed.max(best_allowed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    Normal,
    RateLimited,
    Quarantined,
    Blocked,
}

/// What the caller should *do* now (disconnect is an action, not a state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    None,
    Throttle,
    Quarantine { until_millis: u64 },
    Disconnect,
    Block { until_millis: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbusePolicy {
    pub throttle_at: u32,
    pub quarantine_at: u32,
    pub disconnect_at: u32,
    pub block_at: u32,
    /// Score halves every this many ms of good behaviour.
    pub half_life_millis: u64,
    pub quarantine_millis: u64,
    pub block_millis: u64,
    pub max_tracked: usize,
}

impl Default for AbusePolicy {
    fn default() -> Self {
        Self {
            throttle_at: 5,
            quarantine_at: 15,
            disconnect_at: 30,
            block_at: 60,
            half_life_millis: 10 * 60_000,
            quarantine_millis: 15 * 60_000,
            block_millis: 24 * 3_600_000,
            max_tracked: 4096,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    score: u32,
    scored_at: u64,
    hold_until: u64,
    hold: Standing,
}

#[derive(Debug)]
pub struct AbuseTracker<K> {
    policy: AbusePolicy,
    entries: HashMap<K, Entry>,
}

impl<K: Hash + Eq + Clone> AbuseTracker<K> {
    pub fn new(policy: AbusePolicy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
        }
    }

    fn decayed(&self, e: &Entry, now: u64) -> u32 {
        let halvings = now.saturating_sub(e.scored_at) / self.policy.half_life_millis.max(1);
        if halvings >= 32 {
            0
        } else {
            e.score >> halvings
        }
    }

    pub fn standing(&self, key: &K, now_millis: u64) -> Standing {
        let Some(e) = self.entries.get(key) else {
            return Standing::Normal;
        };
        if e.hold != Standing::Normal && now_millis < e.hold_until {
            return e.hold;
        }
        if self.decayed(e, now_millis) >= self.policy.throttle_at {
            Standing::RateLimited
        } else {
            Standing::Normal
        }
    }

    /// Records a violation worth `severity` points and returns the action
    /// to take *because of this violation* (only on crossing a threshold).
    pub fn record_violation(&mut self, key: K, severity: u32, now_millis: u64) -> Escalation {
        if !self.entries.contains_key(&key) && self.entries.len() >= self.policy.max_tracked {
            self.evict_calmest(now_millis);
        }
        let p = self.policy;
        let before = self
            .entries
            .get(&key)
            .map_or(0, |e| self.decayed(e, now_millis));
        let after = before.saturating_add(severity);
        let e = self.entries.entry(key).or_insert(Entry {
            score: 0,
            scored_at: now_millis,
            hold_until: 0,
            hold: Standing::Normal,
        });
        e.score = after;
        e.scored_at = now_millis;

        let crossed = |t: u32| before < t && after >= t;
        if crossed(p.block_at) || (after >= p.block_at && e.hold != Standing::Blocked) {
            e.hold = Standing::Blocked;
            e.hold_until = now_millis.saturating_add(p.block_millis);
            return Escalation::Block {
                until_millis: e.hold_until,
            };
        }
        if crossed(p.disconnect_at) {
            return Escalation::Disconnect;
        }
        if crossed(p.quarantine_at) {
            e.hold = Standing::Quarantined;
            e.hold_until = now_millis.saturating_add(p.quarantine_millis);
            return Escalation::Quarantine {
                until_millis: e.hold_until,
            };
        }
        if crossed(p.throttle_at) {
            return Escalation::Throttle;
        }
        Escalation::None
    }

    fn evict_calmest(&mut self, now: u64) {
        let victim = self
            .entries
            .iter()
            .filter(|(_, e)| !(e.hold != Standing::Normal && now < e.hold_until))
            .min_by_key(|(_, e)| self.decayed(e, now))
            .map(|(k, _)| k.clone());
        if let Some(k) = victim {
            self.entries.remove(&k);
        }
    }

    pub fn tracked(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    #[test]
    fn unknown_peers_cannot_claim_critical_or_control() {
        assert_eq!(
            authorize_priority(TrustClass::Unknown, Critical, false),
            Normal
        );
        assert_eq!(
            authorize_priority(TrustClass::Unknown, Control, false),
            Normal
        );
        assert_eq!(
            authorize_priority(TrustClass::Unknown, Bulk, false),
            Bulk,
            "never promoted"
        );
        assert_eq!(
            authorize_priority(TrustClass::Known, Critical, false),
            Interactive
        );
        assert_eq!(
            authorize_priority(TrustClass::Organization, Critical, false),
            Control
        );
        assert_eq!(
            authorize_priority(TrustClass::LocalOwnDevice, Critical, false),
            Critical
        );
    }

    #[test]
    fn attested_emergency_is_the_only_bypass() {
        assert_eq!(
            authorize_priority(TrustClass::Unknown, Critical, true),
            Critical
        );
    }

    #[test]
    fn authorization_never_improves_a_priority() {
        for t in [
            TrustClass::Unknown,
            TrustClass::Known,
            TrustClass::Authority,
        ] {
            for p in [Critical, Control, Interactive, Normal, Bulk, Background] {
                assert!(authorize_priority(t, p, false) >= p);
            }
        }
    }

    #[test]
    fn escalation_ladder_fires_once_per_threshold() {
        let mut t = AbuseTracker::new(AbusePolicy::default());
        let k = "peer";
        assert_eq!(t.record_violation(k, 4, 0), Escalation::None);
        assert_eq!(t.record_violation(k, 1, 0), Escalation::Throttle);
        assert_eq!(t.standing(&k, 0), Standing::RateLimited);
        assert_eq!(t.record_violation(k, 1, 0), Escalation::None);
        assert_eq!(
            t.record_violation(k, 9, 0),
            Escalation::Quarantine {
                until_millis: 900_000
            }
        );
        assert_eq!(t.standing(&k, 1), Standing::Quarantined);
        assert_eq!(t.record_violation(k, 15, 0), Escalation::Disconnect);
        assert!(matches!(
            t.record_violation(k, 30, 0),
            Escalation::Block { .. }
        ));
        assert_eq!(t.standing(&k, 1_000), Standing::Blocked);
    }

    #[test]
    fn good_behaviour_decays_the_score_and_holds_expire() {
        let mut t = AbuseTracker::new(AbusePolicy::default());
        t.record_violation("p", 16, 0); // quarantined
        assert_eq!(t.standing(&"p", 899_999), Standing::Quarantined);
        // after the hold and several half-lives the peer is clean again
        assert_eq!(t.standing(&"p", 3_000_000), Standing::Normal);
        assert_eq!(t.standing(&"never seen", 0), Standing::Normal);
    }

    #[test]
    fn tracker_is_bounded_and_spares_active_holds() {
        let p = AbusePolicy {
            max_tracked: 3,
            ..Default::default()
        };
        let mut t = AbuseTracker::new(p);
        t.record_violation(0u32, 70, 0); // blocked
        for i in 1..50u32 {
            t.record_violation(i, 1, 0);
        }
        assert!(t.tracked() <= 3);
        assert_eq!(
            t.standing(&0, 10),
            Standing::Blocked,
            "blocked peer not evicted to make room"
        );
    }
}
