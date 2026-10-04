//! §72 event-log backpressure, §73 projection lag, §74 outbox backpressure.

use std::collections::{HashMap, VecDeque};

// ------------------------------------------------------- §72 writer queue

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterLimits {
    pub max_batch_items: usize,
    pub max_batch_bytes: u64,
    pub queue_items: usize,
    pub queue_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriterBackpressure {
    #[error("a single append of {bytes} bytes exceeds the batch limit {max}")]
    TooLargeForBatch { bytes: u64, max: u64 },
    #[error("writer queue is full (items)")]
    QueueFull,
    #[error("writer queue is full (bytes)")]
    BytesFull,
}

/// Bounded writer queue in front of the append path. When storage can't
/// keep up the queue fills and *producers receive the error* — the queue
/// never grows to absorb the difference.
#[derive(Debug)]
pub struct WriterQueue<T> {
    limits: WriterLimits,
    q: VecDeque<(T, u64)>,
    bytes: u64,
}

impl<T> WriterQueue<T> {
    pub fn new(limits: WriterLimits) -> Self {
        Self {
            limits,
            q: VecDeque::new(),
            bytes: 0,
        }
    }

    pub fn try_enqueue(&mut self, item: T, bytes: u64) -> Result<(), (T, WriterBackpressure)> {
        if bytes > self.limits.max_batch_bytes {
            return Err((
                item,
                WriterBackpressure::TooLargeForBatch {
                    bytes,
                    max: self.limits.max_batch_bytes,
                },
            ));
        }
        if self.q.len() >= self.limits.queue_items {
            return Err((item, WriterBackpressure::QueueFull));
        }
        match self.bytes.checked_add(bytes) {
            Some(n) if n <= self.limits.queue_bytes => self.bytes = n,
            _ => return Err((item, WriterBackpressure::BytesFull)),
        }
        self.q.push_back((item, bytes));
        Ok(())
    }

    /// Next batch, FIFO, within both batch limits (always at least one
    /// item if any is queued, since each item individually fits).
    pub fn next_batch(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        let mut taken = 0u64;
        while let Some((_, b)) = self.q.front() {
            if out.len() >= self.limits.max_batch_items
                || taken.saturating_add(*b) > self.limits.max_batch_bytes
            {
                break;
            }
            taken += *b;
            let (item, b) = self.q.pop_front().expect("front exists");
            self.bytes -= b;
            out.push(item);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.q.len()
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    pub fn queued_bytes(&self) -> u64 {
        self.bytes
    }
}

// ----------------------------------------------------- §73 projection lag

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LagState {
    Healthy,
    Lagging,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LagPolicy {
    pub lagging_at: u64,
    pub critical_at: u64,
    /// Hard stop: beyond this many unprojected events, writes are refused
    /// (the projection backlog must never grow without bound).
    pub reject_at: u64,
    /// Recover to a better state only below this percent of its entry
    /// threshold — hysteresis against flapping at the edge.
    pub recover_percent: u64,
    pub critical_write_delay_millis: u64,
}

impl Default for LagPolicy {
    fn default() -> Self {
        Self {
            lagging_at: 1_000,
            critical_at: 10_000,
            reject_at: 100_000,
            recover_percent: 80,
            critical_write_delay_millis: 50,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LagResponse {
    pub state: LagState,
    /// Artificial delay applied to producers ("slow writes").
    pub write_delay_millis: u64,
    /// Stop doing secondary work (search indexing, previews).
    pub shed_secondary: bool,
    /// Refuse new writes outright.
    pub reject_writes: bool,
}

#[derive(Debug)]
pub struct ProjectionLag {
    policy: LagPolicy,
    state: LagState,
}

impl ProjectionLag {
    pub fn new(policy: LagPolicy) -> Self {
        Self {
            policy,
            state: LagState::Healthy,
        }
    }

    pub fn update(&mut self, events_behind: u64) -> LagResponse {
        let p = self.policy;
        let below =
            |threshold: u64| events_behind < threshold.saturating_mul(p.recover_percent) / 100;
        self.state = match self.state {
            _ if events_behind >= p.critical_at => LagState::Critical,
            LagState::Critical if !below(p.critical_at) => LagState::Critical,
            _ if events_behind >= p.lagging_at => LagState::Lagging,
            LagState::Lagging | LagState::Critical if !below(p.lagging_at) => LagState::Lagging,
            _ => LagState::Healthy,
        };
        LagResponse {
            state: self.state,
            write_delay_millis: if self.state == LagState::Critical {
                p.critical_write_delay_millis
            } else {
                0
            },
            shed_secondary: self.state == LagState::Critical,
            reject_writes: events_behind >= p.reject_at,
        }
    }

    pub fn state(&self) -> LagState {
        self.state
    }
}

// ------------------------------------------------------------ §74 outbox

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxClass {
    Critical,
    Normal,
    Bulk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxLimits {
    pub max_count: usize,
    pub max_bytes: u64,
    /// Count / bytes only Critical may use.
    pub critical_reserve_count: usize,
    pub critical_reserve_bytes: u64,
    /// Bulk may use at most this percent of the non-reserved space.
    pub bulk_percent: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OutboxReject {
    /// Bulk is deferred/rejected first so messaging keeps working.
    #[error("bulk share of the outbox is full")]
    BulkFull,
    #[error("outbox is full (count)")]
    CountFull,
    #[error("outbox is full (bytes)")]
    BytesFull,
    #[error("item already expired")]
    AlreadyExpired,
}

#[derive(Debug, Clone, Copy)]
struct OutboxEntry {
    class: OutboxClass,
    bytes: u64,
    expires_at: u64,
}

#[derive(Debug)]
pub struct OutboxAccounting {
    limits: OutboxLimits,
    next_id: u64,
    entries: HashMap<u64, OutboxEntry>,
    count: usize,
    bytes: u64,
    bulk_count: usize,
    bulk_bytes: u64,
}

impl OutboxAccounting {
    pub fn new(limits: OutboxLimits) -> Self {
        Self {
            limits,
            next_id: 0,
            entries: HashMap::new(),
            count: 0,
            bytes: 0,
            bulk_count: 0,
            bulk_bytes: 0,
        }
    }

    pub fn try_add(
        &mut self,
        class: OutboxClass,
        bytes: u64,
        expires_at_millis: u64,
        now_millis: u64,
    ) -> Result<u64, OutboxReject> {
        let l = self.limits;
        if expires_at_millis <= now_millis {
            return Err(OutboxReject::AlreadyExpired);
        }
        let open_count = l.max_count.saturating_sub(l.critical_reserve_count);
        let open_bytes = l.max_bytes.saturating_sub(l.critical_reserve_bytes);
        let (count_cap, bytes_cap) = match class {
            OutboxClass::Critical => (l.max_count, l.max_bytes),
            OutboxClass::Normal | OutboxClass::Bulk => (open_count, open_bytes),
        };
        let new_bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(OutboxReject::BytesFull)?;
        if class == OutboxClass::Bulk {
            let bulk_count_cap = (open_count as u64 * l.bulk_percent / 100) as usize;
            let bulk_bytes_cap = open_bytes.saturating_mul(l.bulk_percent) / 100;
            if self.bulk_count + 1 > bulk_count_cap
                || self.bulk_bytes.saturating_add(bytes) > bulk_bytes_cap
            {
                return Err(OutboxReject::BulkFull);
            }
        }
        if self.count + 1 > count_cap {
            return Err(OutboxReject::CountFull);
        }
        if new_bytes > bytes_cap {
            return Err(OutboxReject::BytesFull);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(
            id,
            OutboxEntry {
                class,
                bytes,
                expires_at: expires_at_millis,
            },
        );
        self.count += 1;
        self.bytes = new_bytes;
        if class == OutboxClass::Bulk {
            self.bulk_count += 1;
            self.bulk_bytes += bytes;
        }
        Ok(id)
    }

    /// Delivered or cancelled.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(e) = self.entries.remove(&id) else {
            return false;
        };
        self.count -= 1;
        self.bytes -= e.bytes;
        if e.class == OutboxClass::Bulk {
            self.bulk_count -= 1;
            self.bulk_bytes -= e.bytes;
        }
        true
    }

    /// Retention: drop expired entries; returns their ids.
    pub fn expire(&mut self, now_millis: u64) -> Vec<u64> {
        let dead: Vec<u64> = self
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at <= now_millis)
            .map(|(id, _)| *id)
            .collect();
        for id in &dead {
            self.remove(*id);
        }
        dead
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wl() -> WriterLimits {
        WriterLimits {
            max_batch_items: 3,
            max_batch_bytes: 100,
            queue_items: 5,
            queue_bytes: 250,
        }
    }

    #[test]
    fn producers_get_backpressure_not_an_ever_growing_queue() {
        let mut w = WriterQueue::new(wl());
        for i in 0..5 {
            w.try_enqueue(i, 10).unwrap();
        }
        let (item, why) = w.try_enqueue(99, 10).unwrap_err();
        assert_eq!(
            (item, why),
            (99, WriterBackpressure::QueueFull),
            "item handed back, not lost"
        );
        assert_eq!(w.len(), 5);
    }

    #[test]
    fn byte_cap_and_oversize_single_append() {
        let mut w = WriterQueue::new(wl());
        w.try_enqueue('a', 100).unwrap();
        w.try_enqueue('b', 100).unwrap();
        assert_eq!(
            w.try_enqueue('c', 100).unwrap_err().1,
            WriterBackpressure::BytesFull
        );
        assert!(matches!(
            w.try_enqueue('d', 101).unwrap_err().1,
            WriterBackpressure::TooLargeForBatch { .. }
        ));
        assert_eq!(w.queued_bytes(), 200);
    }

    #[test]
    fn batches_respect_item_and_byte_limits_and_stay_fifo() {
        let mut w = WriterQueue::new(wl());
        for (i, b) in [(0, 40), (1, 40), (2, 40), (3, 10)] {
            w.try_enqueue(i, b).unwrap();
        }
        assert_eq!(w.next_batch(), vec![0, 1], "third would make 120 > 100");
        assert_eq!(w.next_batch(), vec![2, 3]);
        assert!(w.next_batch().is_empty());
        assert_eq!(w.queued_bytes(), 0);
    }

    #[test]
    fn lag_states_have_hysteresis_and_critical_slows_and_sheds() {
        let mut p = ProjectionLag::new(LagPolicy::default());
        assert_eq!(p.update(10).state, LagState::Healthy);
        assert_eq!(p.update(1_000).state, LagState::Lagging);
        let r = p.update(10_000);
        assert_eq!(
            (r.state, r.write_delay_millis, r.shed_secondary),
            (LagState::Critical, 50, true)
        );
        // 9_000 is below critical_at but not below 80% of it: stay Critical.
        assert_eq!(p.update(9_000).state, LagState::Critical);
        assert_eq!(p.update(7_999).state, LagState::Lagging);
        assert_eq!(
            p.update(900).state,
            LagState::Lagging,
            "not yet below 80% of 1000"
        );
        assert_eq!(p.update(799).state, LagState::Healthy);
    }

    #[test]
    fn writes_are_refused_past_the_hard_backlog_bound() {
        let mut p = ProjectionLag::new(LagPolicy::default());
        assert!(!p.update(99_999).reject_writes);
        assert!(p.update(100_000).reject_writes);
    }

    fn ol() -> OutboxLimits {
        OutboxLimits {
            max_count: 10,
            max_bytes: 1_000,
            critical_reserve_count: 2,
            critical_reserve_bytes: 200,
            bulk_percent: 50,
        }
    }

    #[test]
    fn bulk_is_rejected_first_and_critical_keeps_its_reserve() {
        let mut o = OutboxAccounting::new(ol());
        // open space: 8 items / 800 bytes; bulk share: 4 / 400
        for _ in 0..4 {
            o.try_add(OutboxClass::Bulk, 100, 1_000, 0).unwrap();
        }
        assert_eq!(
            o.try_add(OutboxClass::Bulk, 1, 1_000, 0).unwrap_err(),
            OutboxReject::BulkFull
        );
        for _ in 0..4 {
            o.try_add(OutboxClass::Normal, 100, 1_000, 0).unwrap();
        }
        assert_eq!(
            o.try_add(OutboxClass::Normal, 1, 1_000, 0).unwrap_err(),
            OutboxReject::CountFull
        );
        // Normal is full but Critical still has its two reserved slots.
        o.try_add(OutboxClass::Critical, 100, 1_000, 0).unwrap();
        o.try_add(OutboxClass::Critical, 100, 1_000, 0).unwrap();
        assert_eq!(
            o.try_add(OutboxClass::Critical, 1, 1_000, 0).unwrap_err(),
            OutboxReject::CountFull
        );
        assert_eq!(o.len(), 10);
    }

    #[test]
    fn byte_cap_applies_independently() {
        let mut o = OutboxAccounting::new(ol());
        o.try_add(OutboxClass::Normal, 790, 1_000, 0).unwrap();
        assert_eq!(
            o.try_add(OutboxClass::Normal, 11, 1_000, 0).unwrap_err(),
            OutboxReject::BytesFull
        );
        assert!(o.try_add(OutboxClass::Critical, 200, 1_000, 0).is_ok());
        assert_eq!(
            o.try_add(OutboxClass::Normal, u64::MAX, 1_000, 0)
                .unwrap_err(),
            OutboxReject::BytesFull
        );
    }

    #[test]
    fn expiry_and_removal_return_capacity() {
        let mut o = OutboxAccounting::new(ol());
        let a = o.try_add(OutboxClass::Bulk, 100, 50, 0).unwrap();
        let b = o.try_add(OutboxClass::Normal, 100, 500, 0).unwrap();
        assert_eq!(
            o.try_add(OutboxClass::Normal, 1, 0, 10).unwrap_err(),
            OutboxReject::AlreadyExpired
        );
        assert_eq!(o.expire(60), vec![a]);
        assert_eq!((o.len(), o.bytes()), (1, 100));
        assert!(o.remove(b));
        assert!(!o.remove(b));
        assert!(o.is_empty());
        // bulk share accounting also unwound
        for _ in 0..4 {
            o.try_add(OutboxClass::Bulk, 100, 1_000, 60).unwrap();
        }
    }
}
