//! §75 daemon IPC backpressure, §76 FFI backpressure, §77 plugin
//! backpressure — one mechanism, three hosts.
//!
//! A slow consumer (GUI over IPC, a Kotlin/Swift callback, a plugin) must
//! never make the core's memory grow or block the core. Per consumer there
//! is a bounded [`ClientQueue`]:
//! - `Critical` updates are never silently lost;
//! - `Coalesce` updates replace an earlier update with the same key;
//! - `Droppable` updates are the first to go.
//!
//! If even the critical updates overflow, the queue is cleared and the
//! consumer is told to **resync from a fresh snapshot** — correctness is
//! restored by state transfer, not by pretending nothing was lost.

use std::collections::VecDeque;

use crate::token_bucket::TokenBucket;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update<T> {
    Critical(T),
    Coalesce { key: u64, value: T },
    Droppable(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    Queued,
    /// Replaced an earlier update with the same key in place.
    Coalesced,
    /// Queue full: this (droppable) update was discarded.
    DroppedIncoming,
    /// Queue full: older noncritical updates were evicted to make room.
    EvictedNoncritical,
    /// Could not keep critical updates: queue cleared, resync required.
    ResyncRequired,
}

#[derive(Debug)]
struct Entry<T> {
    key: Option<u64>,
    critical: bool,
    droppable: bool,
    value: T,
}

#[derive(Debug)]
pub struct ClientQueue<T> {
    q: VecDeque<Entry<T>>,
    capacity: usize,
    resync: bool,
}

impl<T> ClientQueue<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            q: VecDeque::new(),
            capacity: capacity.max(1),
            resync: false,
        }
    }

    /// While a resync is pending, incoming updates are discarded: the
    /// snapshot the consumer is about to fetch already includes them.
    pub fn push(&mut self, update: Update<T>) -> PushOutcome {
        if self.resync {
            return PushOutcome::DroppedIncoming;
        }
        let (key, critical, droppable, value) = match update {
            Update::Critical(v) => (None, true, false, v),
            Update::Coalesce { key, value } => {
                if let Some(e) = self.q.iter_mut().find(|e| e.key == Some(key)) {
                    e.value = value;
                    return PushOutcome::Coalesced;
                }
                (Some(key), false, false, value)
            }
            Update::Droppable(v) => (None, false, true, v),
        };
        if self.q.len() < self.capacity {
            self.q.push_back(Entry {
                key,
                critical,
                droppable,
                value,
            });
            return PushOutcome::Queued;
        }
        if droppable {
            return PushOutcome::DroppedIncoming;
        }
        // Make room: oldest droppable first, then oldest coalescible.
        let victim = self
            .q
            .iter()
            .position(|e| e.droppable)
            .or_else(|| self.q.iter().position(|e| !e.critical));
        match victim {
            Some(i) => {
                self.q.remove(i);
                self.q.push_back(Entry {
                    key,
                    critical,
                    droppable,
                    value,
                });
                PushOutcome::EvictedNoncritical
            }
            None => {
                // Full of critical updates: stop pretending, resync.
                self.q.clear();
                self.resync = true;
                PushOutcome::ResyncRequired
            }
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.q.pop_front().map(|e| e.value)
    }

    /// Consumer asks "do I need a fresh snapshot?" — clears the flag.
    pub fn take_resync(&mut self) -> bool {
        std::mem::take(&mut self.resync)
    }

    pub fn resync_pending(&self) -> bool {
        self.resync
    }

    pub fn len(&self) -> usize {
        self.q.len()
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }
}

/// §76: a push-style FFI callback must not fire infinitely fast. Hosts
/// that can't poll get callbacks through this limiter; excess events stay
/// in the (coalescing) queue and are delivered on the next allowed tick.
#[derive(Debug)]
pub struct CallbackLimiter {
    bucket: TokenBucket,
}

impl CallbackLimiter {
    pub fn new(burst: u64, per_sec: u64, now_millis: u64) -> Self {
        Self {
            bucket: TokenBucket::new(burst, per_sec, now_millis),
        }
    }

    pub fn allow(&mut self, now_millis: u64) -> bool {
        self.bucket.try_consume(1, now_millis)
    }
}

/// §77: a plugin's mailbox. A handler that overruns its timeout gets a
/// strike; enough strikes and the plugin should be suspended. Meanwhile
/// the bounded queue keeps shedding noncritical work, so a slow plugin
/// never blocks or bloats the core.
#[derive(Debug)]
pub struct PluginMailbox<T> {
    queue: ClientQueue<T>,
    handler_timeout_millis: u64,
    in_flight_since: Option<u64>,
    strikes: u32,
    max_strikes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginHealth {
    Ok,
    Slow,
    /// Too many overruns: stop delivering, let the supervisor decide.
    Suspend,
}

impl<T> PluginMailbox<T> {
    pub fn new(capacity: usize, handler_timeout_millis: u64, max_strikes: u32) -> Self {
        Self {
            queue: ClientQueue::new(capacity),
            handler_timeout_millis,
            in_flight_since: None,
            strikes: 0,
            max_strikes,
        }
    }

    pub fn deliver(&mut self, u: Update<T>) -> PushOutcome {
        self.queue.push(u)
    }

    /// Hand the plugin its next message; `None` if busy or empty.
    pub fn next_for_handler(&mut self, now_millis: u64) -> Option<T> {
        if self.in_flight_since.is_some() || self.strikes >= self.max_strikes {
            return None;
        }
        let v = self.queue.pop()?;
        self.in_flight_since = Some(now_millis);
        Some(v)
    }

    pub fn handler_finished(&mut self) {
        self.in_flight_since = None;
    }

    /// Call periodically. An in-flight handler past its deadline is
    /// abandoned (one strike) so the mailbox moves on.
    pub fn check_timeout(&mut self, now_millis: u64) -> PluginHealth {
        if let Some(since) = self.in_flight_since {
            if now_millis.saturating_sub(since) > self.handler_timeout_millis {
                self.in_flight_since = None;
                self.strikes += 1;
            }
        }
        if self.strikes >= self.max_strikes {
            PluginHealth::Suspend
        } else if self.strikes > 0 {
            PluginHealth::Slow
        } else {
            PluginHealth::Ok
        }
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Update::*;

    #[test]
    fn coalescing_replaces_in_place_without_growing() {
        let mut q = ClientQueue::new(4);
        assert_eq!(
            q.push(Coalesce {
                key: 1,
                value: "typing…"
            }),
            PushOutcome::Queued
        );
        for i in 0..100 {
            let v = if i % 2 == 0 { "a" } else { "b" };
            assert_eq!(
                q.push(Coalesce { key: 1, value: v }),
                PushOutcome::Coalesced
            );
        }
        assert_eq!(q.len(), 1);
        assert_eq!(q.pop(), Some("b"));
    }

    #[test]
    fn full_queue_drops_droppable_first_then_coalescible_never_critical() {
        let mut q = ClientQueue::new(3);
        q.push(Critical("c1"));
        q.push(Coalesce {
            key: 7,
            value: "co",
        });
        q.push(Droppable("d"));
        assert_eq!(q.push(Droppable("d2")), PushOutcome::DroppedIncoming);
        assert_eq!(q.push(Critical("c2")), PushOutcome::EvictedNoncritical); // evicts "d"
        assert_eq!(q.push(Critical("c3")), PushOutcome::EvictedNoncritical); // evicts "co"
        let drained: Vec<_> = std::iter::from_fn(|| q.pop()).collect();
        assert_eq!(drained, vec!["c1", "c2", "c3"]);
    }

    #[test]
    fn all_critical_overflow_forces_a_resync_instead_of_silent_loss() {
        let mut q = ClientQueue::new(2);
        q.push(Critical(1));
        q.push(Critical(2));
        assert_eq!(q.push(Critical(3)), PushOutcome::ResyncRequired);
        assert!(q.is_empty());
        assert!(q.resync_pending());
        // updates during the pending resync are covered by the snapshot
        assert_eq!(q.push(Critical(4)), PushOutcome::DroppedIncoming);
        assert!(q.take_resync());
        assert!(!q.take_resync());
        assert_eq!(
            q.push(Critical(5)),
            PushOutcome::Queued,
            "normal service resumes"
        );
    }

    #[test]
    fn memory_stays_bounded_under_a_stalled_consumer() {
        let mut q = ClientQueue::new(8);
        for i in 0..100_000u64 {
            let u = match i % 3 {
                0 => Droppable(i),
                1 => Coalesce {
                    key: i % 5,
                    value: i,
                },
                _ => Critical(i),
            };
            q.push(u);
            assert!(q.len() <= 8);
        }
    }

    #[test]
    fn callbacks_are_rate_limited() {
        let mut c = CallbackLimiter::new(3, 10, 0);
        assert_eq!((0..10).filter(|_| c.allow(0)).count(), 3);
        assert!(c.allow(100)); // 10/s -> one token per 100ms
        assert!(!c.allow(100));
    }

    #[test]
    fn a_slow_plugin_is_struck_abandoned_then_suspended_without_blocking_the_core() {
        let mut m = PluginMailbox::new(4, 100, 2);
        for i in 0..50 {
            m.deliver(Droppable(i)); // core never blocks, queue stays bounded
        }
        assert!(m.queued() <= 4);
        assert!(m.next_for_handler(0).is_some());
        assert!(m.next_for_handler(1).is_none(), "busy");
        assert_eq!(
            m.check_timeout(100),
            PluginHealth::Ok,
            "exactly at timeout is not over"
        );
        assert_eq!(m.check_timeout(101), PluginHealth::Slow);
        assert!(
            m.next_for_handler(102).is_some(),
            "mailbox moved on past the stuck handler"
        );
        assert_eq!(m.check_timeout(300), PluginHealth::Suspend);
        assert!(
            m.next_for_handler(301).is_none(),
            "suspended: nothing more delivered"
        );
    }

    #[test]
    fn a_fast_handler_never_accrues_strikes() {
        let mut m = PluginMailbox::new(4, 100, 2);
        m.deliver(Critical(1));
        m.next_for_handler(0);
        m.handler_finished();
        assert_eq!(m.check_timeout(10_000), PluginHealth::Ok);
    }
}
