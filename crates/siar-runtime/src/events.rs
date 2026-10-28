//! §23-§25, §144-§148, §169-§171: bounded event streams.
//!
//! Every subscriber has a **bounded** queue; a slow client can never grow
//! daemon memory or stall publishers. When it falls behind:
//!
//! 1. queued `Progress` events with the same coalesce key are replaced in
//!    place (§148) — a transfer percentage only ever needs its latest value;
//! 2. otherwise the oldest `Progress`, then oldest `Normal`, event is dropped;
//! 3. if only `Critical` events remain and the queue is still full, the queue
//!    is cleared and the subscriber is told about a **gap** (§170) and must
//!    re-request a snapshot (§25). Dropping a `Progress` event is *not* a gap:
//!    progress is lossy by definition.
//!
//! Sequence numbers are per daemon instance (`instance_id`, §168/§171): after
//! a daemon restart the sequence restarts, and a client that sees a different
//! instance id knows its cursor is meaningless and resyncs.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

/// Opaque topic number; the application defines the meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Topic(pub u16);

/// §147. Declared most→least important.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EventPriority {
    Critical,
    Normal,
    Progress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub topic: Topic,
    pub priority: EventPriority,
    /// Events with the same `Some(key)` and `Progress` priority replace each other.
    pub coalesce_key: Option<u64>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    Event(Event),
    /// Events `from..=to` were lost: fetch a fresh snapshot.
    Gap {
        from: u64,
        to: u64,
    },
}

#[derive(Debug)]
struct Queue {
    items: VecDeque<Event>,
    gap: Option<(u64, u64)>,
    closed: bool,
}

#[derive(Debug)]
struct Shared {
    cap: usize,
    topics: Vec<Topic>,
    q: Mutex<Queue>,
    notify: Notify,
}

impl Shared {
    fn wants(&self, t: Topic) -> bool {
        self.topics.is_empty() || self.topics.contains(&t)
    }

    fn push(&self, ev: &Event) {
        let mut q = self
            .q
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let (EventPriority::Progress, Some(k)) = (ev.priority, ev.coalesce_key) {
            if let Some(slot) = q
                .items
                .iter_mut()
                .find(|e| e.priority == EventPriority::Progress && e.coalesce_key == Some(k))
            {
                *slot = ev.clone();
                drop(q);
                self.notify.notify_one();
                return;
            }
        }
        if q.items.len() >= self.cap {
            let victim = [EventPriority::Progress, EventPriority::Normal]
                .into_iter()
                .find_map(|p| q.items.iter().position(|e| e.priority == p));
            match victim {
                Some(i) => {
                    let dropped = q.items.remove(i).expect("index from position");
                    if dropped.priority != EventPriority::Progress {
                        note_gap(&mut q, dropped.seq, dropped.seq);
                    }
                }
                None => {
                    // Only critical events queued: clear and resync.
                    let first = q.items.front().map_or(ev.seq, |e| e.seq);
                    q.items.clear();
                    note_gap(&mut q, first, ev.seq);
                    drop(q);
                    self.notify.notify_one();
                    return;
                }
            }
        }
        q.items.push_back(ev.clone());
        drop(q);
        self.notify.notify_one();
    }
}

fn note_gap(q: &mut Queue, from: u64, to: u64) {
    q.gap = Some(match q.gap {
        Some((f, t)) => (f.min(from), t.max(to)),
        None => (from, to),
    });
}

/// Publisher side. Cloning shares the same bus.
#[derive(Debug, Clone)]
pub struct EventBus {
    inner: Arc<BusInner>,
}

#[derive(Debug)]
struct BusInner {
    instance_id: u128,
    state: Mutex<BusState>,
}

#[derive(Debug, Default)]
struct BusState {
    next_seq: u64,
    subs: Vec<Weak<Shared>>,
}

impl EventBus {
    #[must_use]
    pub fn new(instance_id: u128) -> Self {
        Self {
            inner: Arc::new(BusInner {
                instance_id,
                state: Mutex::new(BusState {
                    next_seq: 1,
                    subs: Vec::new(),
                }),
            }),
        }
    }

    #[must_use]
    pub fn instance_id(&self) -> u128 {
        self.inner.instance_id
    }

    /// The sequence number the **next** event will get; a snapshot taken now
    /// is consistent with "deltas from this seq onward" (§25).
    #[must_use]
    pub fn next_seq(&self) -> u64 {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_seq
    }

    /// Subscribe to `topics` (empty = all) with a queue of `capacity` events.
    #[must_use]
    pub fn subscribe(&self, topics: Vec<Topic>, capacity: usize) -> Subscription {
        let shared = Arc::new(Shared {
            cap: capacity.max(1),
            topics,
            q: Mutex::new(Queue {
                items: VecDeque::new(),
                gap: None,
                closed: false,
            }),
            notify: Notify::new(),
        });
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .subs
            .push(Arc::downgrade(&shared));
        Subscription { shared }
    }

    /// Publish; returns the assigned sequence number. Never blocks on subscribers.
    pub fn publish(
        &self,
        topic: Topic,
        priority: EventPriority,
        coalesce_key: Option<u64>,
        payload: Vec<u8>,
    ) -> u64 {
        let mut st = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = st.next_seq;
        st.next_seq = st.next_seq.saturating_add(1);
        let ev = Event {
            seq,
            topic,
            priority,
            coalesce_key,
            payload,
        };
        st.subs.retain(|w| match w.upgrade() {
            Some(s) => {
                if s.wants(topic) {
                    s.push(&ev);
                }
                true
            }
            None => false,
        });
        seq
    }

    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .subs
            .iter()
            .filter(|w| w.strong_count() > 0)
            .count()
    }
}

/// Consumer side.
#[derive(Debug)]
pub struct Subscription {
    shared: Arc<Shared>,
}

impl Subscription {
    /// Non-blocking: a pending gap is reported before any further event.
    #[must_use]
    pub fn try_recv(&self) -> Option<Delivery> {
        let mut q = self
            .shared
            .q
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((from, to)) = q.gap.take() {
            return Some(Delivery::Gap { from, to });
        }
        q.items.pop_front().map(Delivery::Event)
    }

    /// Wait for the next delivery.
    pub async fn recv(&self) -> Delivery {
        loop {
            let notified = self.shared.notify.notified();
            if let Some(d) = self.try_recv() {
                return d;
            }
            notified.await;
        }
    }

    pub fn close(&self) {
        self.shared
            .q
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Topic = Topic(1);

    fn drain(s: &Subscription) -> Vec<Delivery> {
        std::iter::from_fn(|| s.try_recv()).collect()
    }

    #[test]
    fn sequence_is_monotonic_and_per_instance() {
        let a = EventBus::new(1);
        assert_eq!(a.publish(T, EventPriority::Normal, None, vec![]), 1);
        assert_eq!(a.publish(T, EventPriority::Normal, None, vec![]), 2);
        assert_eq!(a.next_seq(), 3);
        let b = EventBus::new(2);
        assert_eq!(
            b.publish(T, EventPriority::Normal, None, vec![]),
            1,
            "a new instance restarts at 1"
        );
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn topic_filter() {
        let bus = EventBus::new(1);
        let only2 = bus.subscribe(vec![Topic(2)], 8);
        let all = bus.subscribe(vec![], 8);
        bus.publish(Topic(1), EventPriority::Normal, None, vec![1]);
        bus.publish(Topic(2), EventPriority::Normal, None, vec![2]);
        assert_eq!(drain(&only2).len(), 1);
        assert_eq!(drain(&all).len(), 2);
    }

    #[test]
    fn progress_coalesces_to_the_latest_value() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 4);
        for pct in 0..100u8 {
            bus.publish(T, EventPriority::Progress, Some(7), vec![pct]);
        }
        let d = drain(&s);
        assert_eq!(
            d.len(),
            1,
            "100 progress ticks collapse to one queued event"
        );
        assert!(matches!(&d[0], Delivery::Event(e) if e.payload == vec![99]));
    }

    #[test]
    fn slow_client_is_bounded_and_progress_loss_is_not_a_gap() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 3);
        for k in 0..50u64 {
            bus.publish(T, EventPriority::Progress, Some(k), vec![]);
        }
        let d = drain(&s);
        assert_eq!(d.len(), 3);
        assert!(
            d.iter().all(|x| matches!(x, Delivery::Event(_))),
            "no gap for lossy progress"
        );
    }

    #[test]
    fn losing_a_normal_event_reports_a_gap_before_later_events() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 2);
        let a = bus.publish(T, EventPriority::Normal, None, vec![]);
        bus.publish(T, EventPriority::Normal, None, vec![]);
        bus.publish(T, EventPriority::Normal, None, vec![]); // evicts `a`
        let d = drain(&s);
        assert_eq!(d[0], Delivery::Gap { from: a, to: a });
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn progress_is_sacrificed_before_normal_and_critical_before_nothing() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 2);
        bus.publish(T, EventPriority::Normal, None, vec![]);
        bus.publish(T, EventPriority::Progress, Some(1), vec![]);
        bus.publish(T, EventPriority::Critical, None, vec![]);
        let d = drain(&s);
        assert!(d.iter().all(|x| !matches!(x, Delivery::Gap { .. })));
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn only_critical_overflow_clears_and_demands_resync() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 2);
        let first = bus.publish(T, EventPriority::Critical, None, vec![]);
        bus.publish(T, EventPriority::Critical, None, vec![]);
        let last = bus.publish(T, EventPriority::Critical, None, vec![]);
        let d = drain(&s);
        assert_eq!(
            d,
            vec![Delivery::Gap {
                from: first,
                to: last
            }]
        );
    }

    #[test]
    fn dropped_subscription_is_pruned_and_publish_never_blocks() {
        let bus = EventBus::new(1);
        {
            let _s = bus.subscribe(vec![], 1);
            assert_eq!(bus.subscriber_count(), 1);
        }
        bus.publish(T, EventPriority::Normal, None, vec![]);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn recv_waits_for_a_publish() {
        let bus = EventBus::new(1);
        let s = bus.subscribe(vec![], 4);
        let b2 = bus.clone();
        let t = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            b2.publish(T, EventPriority::Normal, None, vec![9]);
        });
        let d = tokio::time::timeout(std::time::Duration::from_secs(2), s.recv())
            .await
            .unwrap();
        assert!(matches!(d, Delivery::Event(e) if e.payload == vec![9]));
        t.await.unwrap();
    }
}
