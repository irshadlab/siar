//! §165 resource pressure notifications, §166 rate-limited diagnostics,
//! §139 event-log integration.
//!
//! Resource events are delivered in batches no more often than
//! `min_interval`, never one per packet:
//! - pressure changes are **coalesced per resource** and emitted only if
//!   the state actually differs from the last one emitted (a flap
//!   Normal→Critical→Normal inside one window emits nothing);
//! - repeated rejections / quota hits from the same owner become one
//!   event carrying a repeat count;
//! - the number of distinct owners tracked per window is capped, with the
//!   excess counted rather than stored.
//!
//! §139: only *meaningful* transitions reach the durable event log, and
//! each at most once per episode — never a permit acquisition.

use std::collections::HashMap;

use crate::admission::ResourceOwner;
use crate::storage::PressureState;
use crate::types::ResourceKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceEvent {
    PressureChanged(ResourceKind, PressureState),
    AdmissionRejected(ResourceOwner),
    QuotaExceeded(ResourceOwner),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emitted {
    pub event: ResourceEvent,
    /// How many raw occurrences this stands for (≥ 1).
    pub repeats: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlushReport {
    pub events: Vec<Emitted>,
    /// Owner-keyed events dropped because the window's owner cap was hit.
    pub suppressed_owner_events: u32,
}

#[derive(Debug)]
pub struct EventHub {
    min_interval_millis: u64,
    max_owners_per_window: usize,
    last_flush: Option<u64>,
    emitted_pressure: HashMap<ResourceKind, PressureState>,
    pending_pressure: HashMap<ResourceKind, PressureState>,
    rejected: Vec<(ResourceOwner, u32)>,
    quota: Vec<(ResourceOwner, u32)>,
    suppressed: u32,
}

fn bump(list: &mut Vec<(ResourceOwner, u32)>, owner: ResourceOwner, cap: usize) -> bool {
    if let Some((_, n)) = list.iter_mut().find(|(o, _)| *o == owner) {
        *n = n.saturating_add(1);
        return true;
    }
    if list.len() >= cap {
        return false;
    }
    list.push((owner, 1));
    true
}

impl EventHub {
    pub fn new(min_interval_millis: u64, max_owners_per_window: usize) -> Self {
        Self {
            min_interval_millis,
            max_owners_per_window: max_owners_per_window.max(1),
            last_flush: None,
            emitted_pressure: HashMap::new(),
            pending_pressure: HashMap::new(),
            rejected: Vec::new(),
            quota: Vec::new(),
            suppressed: 0,
        }
    }

    pub fn push(&mut self, event: ResourceEvent) {
        let cap = self.max_owners_per_window;
        match event {
            ResourceEvent::PressureChanged(k, s) => {
                self.pending_pressure.insert(k, s); // latest wins
            }
            ResourceEvent::AdmissionRejected(o) => {
                if !bump(&mut self.rejected, o, cap) {
                    self.suppressed = self.suppressed.saturating_add(1);
                }
            }
            ResourceEvent::QuotaExceeded(o) => {
                if !bump(&mut self.quota, o, cap) {
                    self.suppressed = self.suppressed.saturating_add(1);
                }
            }
        }
    }

    /// Emits the coalesced batch if the interval has elapsed; otherwise
    /// returns an empty report and keeps accumulating.
    pub fn flush(&mut self, now_millis: u64) -> FlushReport {
        if let Some(last) = self.last_flush {
            if now_millis.saturating_sub(last) < self.min_interval_millis {
                return FlushReport::default();
            }
        }
        self.last_flush = Some(now_millis);
        let mut events = Vec::new();

        let mut kinds: Vec<_> = self.pending_pressure.drain().collect();
        kinds.sort_by_key(|(k, _)| *k);
        for (k, s) in kinds {
            let prev = self.emitted_pressure.get(&k).copied().unwrap_or_default();
            if prev != s {
                self.emitted_pressure.insert(k, s);
                events.push(Emitted {
                    event: ResourceEvent::PressureChanged(k, s),
                    repeats: 1,
                });
            }
        }
        for (o, n) in self.rejected.drain(..) {
            events.push(Emitted {
                event: ResourceEvent::AdmissionRejected(o),
                repeats: n,
            });
        }
        for (o, n) in self.quota.drain(..) {
            events.push(Emitted {
                event: ResourceEvent::QuotaExceeded(o),
                repeats: n,
            });
        }
        FlushReport {
            events,
            suppressed_owner_events: std::mem::take(&mut self.suppressed),
        }
    }
}

// ------------------------------------------------------------- §139

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferCause {
    Storage,
    Network,
    Slots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseCause {
    ResourcePressure,
    UserRequest,
    Mode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceTransition {
    PermitAcquired,
    PermitReleased,
    TransferDeferred(DeferCause),
    TransferPaused(PauseCause),
    TransferResumed,
}

/// The event-log record kinds this subsystem is allowed to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalKind {
    TransferDeferredStorage,
    TransferPausedResourcePressure,
}

pub fn journal_kind(t: &ResourceTransition) -> Option<JournalKind> {
    match t {
        ResourceTransition::TransferDeferred(DeferCause::Storage) => {
            Some(JournalKind::TransferDeferredStorage)
        }
        ResourceTransition::TransferPaused(PauseCause::ResourcePressure) => {
            Some(JournalKind::TransferPausedResourcePressure)
        }
        // permits, other defer/pause reasons, resumes: runtime state only
        _ => None,
    }
}

/// Journals each episode once: while a transfer stays deferred/paused for
/// the same reason, repeated notifications are not re-journaled. A resume
/// ends the episode so a later one is recorded again.
#[derive(Debug)]
pub struct TransitionJournal {
    open: HashMap<u64, JournalKind>,
    max_tracked: usize,
}

impl TransitionJournal {
    pub fn new(max_tracked: usize) -> Self {
        Self {
            open: HashMap::new(),
            max_tracked: max_tracked.max(1),
        }
    }

    pub fn should_journal(&mut self, transfer: u64, t: &ResourceTransition) -> Option<JournalKind> {
        if matches!(t, ResourceTransition::TransferResumed) {
            self.open.remove(&transfer);
            return None;
        }
        let kind = journal_kind(t)?;
        if self.open.get(&transfer) == Some(&kind) {
            return None;
        }
        if !self.open.contains_key(&transfer) && self.open.len() >= self.max_tracked {
            // Bounded: forget an arbitrary old episode rather than grow.
            // (Worst case an episode is journaled twice — harmless.)
            if let Some(k) = self.open.keys().next().copied() {
                self.open.remove(&k);
            }
        }
        self.open.insert(transfer, kind);
        Some(kind)
    }

    pub fn open_episodes(&self) -> usize {
        self.open.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PressureState::*;

    #[test]
    fn pressure_flapping_inside_a_window_emits_nothing() {
        let mut h = EventHub::new(1_000, 8);
        h.push(ResourceEvent::PressureChanged(
            ResourceKind::Memory,
            Critical,
        ));
        h.push(ResourceEvent::PressureChanged(ResourceKind::Memory, Normal));
        assert!(h.flush(0).events.is_empty(), "net change is none");
    }

    #[test]
    fn pressure_is_coalesced_per_resource_latest_wins_and_not_repeated() {
        let mut h = EventHub::new(1_000, 8);
        h.push(ResourceEvent::PressureChanged(
            ResourceKind::Memory,
            Elevated,
        ));
        h.push(ResourceEvent::PressureChanged(
            ResourceKind::Memory,
            Critical,
        ));
        h.push(ResourceEvent::PressureChanged(
            ResourceKind::Storage,
            Elevated,
        ));
        let r = h.flush(0);
        assert_eq!(
            r.events,
            vec![
                Emitted {
                    event: ResourceEvent::PressureChanged(ResourceKind::Memory, Critical),
                    repeats: 1
                },
                Emitted {
                    event: ResourceEvent::PressureChanged(ResourceKind::Storage, Elevated),
                    repeats: 1
                },
            ]
        );
        h.push(ResourceEvent::PressureChanged(
            ResourceKind::Memory,
            Critical,
        ));
        assert!(
            h.flush(5_000).events.is_empty(),
            "same state as last emitted"
        );
    }

    #[test]
    fn a_packet_storm_of_rejections_becomes_one_event_with_a_count() {
        let mut h = EventHub::new(1_000, 8);
        for _ in 0..10_000 {
            h.push(ResourceEvent::AdmissionRejected(ResourceOwner::Messaging));
        }
        h.push(ResourceEvent::QuotaExceeded(ResourceOwner::Files));
        let r = h.flush(0);
        assert_eq!(r.events.len(), 2);
        assert_eq!(
            r.events[0],
            Emitted {
                event: ResourceEvent::AdmissionRejected(ResourceOwner::Messaging),
                repeats: 10_000
            }
        );
        assert_eq!(r.events[1].repeats, 1);
    }

    #[test]
    fn flushes_are_rate_limited_and_accumulate_meanwhile() {
        let mut h = EventHub::new(1_000, 8);
        h.push(ResourceEvent::AdmissionRejected(ResourceOwner::Dtn));
        assert_eq!(h.flush(0).events.len(), 1);
        h.push(ResourceEvent::AdmissionRejected(ResourceOwner::Dtn));
        assert!(h.flush(500).events.is_empty(), "too soon");
        h.push(ResourceEvent::AdmissionRejected(ResourceOwner::Dtn));
        let r = h.flush(1_000);
        assert_eq!(
            r.events[0].repeats, 2,
            "both accumulated occurrences delivered together"
        );
    }

    #[test]
    fn distinct_owner_count_per_window_is_capped_and_the_excess_is_counted() {
        let mut h = EventHub::new(0, 2);
        for o in [
            ResourceOwner::Messaging,
            ResourceOwner::Files,
            ResourceOwner::Dtn,
            ResourceOwner::Calls,
        ] {
            h.push(ResourceEvent::AdmissionRejected(o));
        }
        h.push(ResourceEvent::AdmissionRejected(ResourceOwner::Messaging)); // known owner still counts
        let r = h.flush(0);
        assert_eq!(r.events.len(), 2);
        assert_eq!(r.suppressed_owner_events, 2);
        assert_eq!(r.events[0].repeats, 2);
        assert_eq!(h.flush(1).suppressed_owner_events, 0, "counter resets");
    }

    #[test]
    fn only_the_two_meaningful_transitions_are_journalable() {
        use ResourceTransition::*;
        assert_eq!(
            journal_kind(&TransferDeferred(DeferCause::Storage)),
            Some(JournalKind::TransferDeferredStorage)
        );
        assert_eq!(
            journal_kind(&TransferPaused(PauseCause::ResourcePressure)),
            Some(JournalKind::TransferPausedResourcePressure)
        );
        for t in [
            PermitAcquired,
            PermitReleased,
            TransferResumed,
            TransferDeferred(DeferCause::Network),
            TransferDeferred(DeferCause::Slots),
            TransferPaused(PauseCause::UserRequest),
            TransferPaused(PauseCause::Mode),
        ] {
            assert_eq!(journal_kind(&t), None, "{t:?}");
        }
    }

    #[test]
    fn an_episode_is_journaled_once_and_again_only_after_a_resume() {
        use ResourceTransition::*;
        let mut j = TransitionJournal::new(8);
        let d = TransferDeferred(DeferCause::Storage);
        assert_eq!(
            j.should_journal(1, &d),
            Some(JournalKind::TransferDeferredStorage)
        );
        assert_eq!(j.should_journal(1, &d), None, "still the same episode");
        assert_eq!(
            j.should_journal(2, &d),
            Some(JournalKind::TransferDeferredStorage),
            "other transfer"
        );
        assert_eq!(j.should_journal(1, &TransferResumed), None);
        assert_eq!(
            j.should_journal(1, &d),
            Some(JournalKind::TransferDeferredStorage),
            "new episode"
        );
        // a different journalable reason on the same transfer is a new record
        assert_eq!(
            j.should_journal(1, &TransferPaused(PauseCause::ResourcePressure)),
            Some(JournalKind::TransferPausedResourcePressure)
        );
    }

    #[test]
    fn journal_tracking_is_bounded() {
        use ResourceTransition::*;
        let mut j = TransitionJournal::new(3);
        for id in 0..100 {
            j.should_journal(id, &TransferDeferred(DeferCause::Storage));
        }
        assert!(j.open_episodes() <= 3);
    }
}
