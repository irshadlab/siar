//! §107 resource metrics, §108 cardinality discipline, §109 per-peer
//! diagnostics, §110 resource leak detection.
//!
//! Cardinality discipline is enforced by the *types*: metric recording
//! functions accept only enums with a fixed number of values (trust
//! class, priority, resource kind) or an [`ExtLabel`] drawn from a small
//! bounded allowlist. There is no argument through which a raw peer ID
//! could become a label, so the metric set has a compile-time-fixed size
//! and cannot be inflated by whoever connects. Per-peer detail lives in a
//! separate, opt-in, local-only [`PeerDiagnostics`] that has no
//! serialization and is never fed to telemetry.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::admission::WorkPriority;
use crate::connections::PeerKey;
use crate::peer_quota::TrustClass;
use crate::types::ResourceKind;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

const TRUST: usize = 6;
const PRIO: usize = 6;
const KINDS: usize = ResourceKind::COUNT;

fn trust_index(t: TrustClass) -> usize {
    match t {
        TrustClass::Unknown => 0,
        TrustClass::Known => 1,
        TrustClass::Verified => 2,
        TrustClass::Organization => 3,
        TrustClass::Authority => 4,
        TrustClass::LocalOwnDevice => 5,
    }
}

/// The only dimensions a resource metric may carry (§108).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    pub trust: TrustClass,
    pub priority: WorkPriority,
    pub kind: ResourceKind,
}

impl Labels {
    fn index(&self) -> usize {
        (trust_index(self.trust) * PRIO + self.priority as usize) * KINDS + self.kind.index()
    }
}

#[derive(Debug, Default)]
struct Counters {
    admission_rejects: AtomicU64,
    deferred: AtomicU64,
    evictions: AtomicU64,
    shed: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CounterValues {
    pub admission_rejects: u64,
    pub deferred: u64,
    pub evictions: u64,
    pub shed: u64,
}

pub const HISTOGRAM_BUCKETS: usize = 16;

/// Log2-millisecond histogram: bucket 0 = 0 ms, bucket n = [2^(n-1), 2^n),
/// last bucket open-ended (≥ 16.4 s). Fixed size, lock-free.
#[derive(Debug, Default)]
pub struct WaitHistogram {
    buckets: [AtomicU64; HISTOGRAM_BUCKETS],
    sum_ms: AtomicU64,
    max_ms: AtomicU64,
}

impl WaitHistogram {
    pub fn record(&self, millis: u64) {
        let b = if millis == 0 {
            0
        } else {
            ((64 - millis.leading_zeros()) as usize).min(HISTOGRAM_BUCKETS - 1)
        };
        self.buckets[b].fetch_add(1, Ordering::Relaxed);
        // saturating accumulate
        let _ = self
            .sum_ms
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
                Some(s.saturating_add(millis))
            });
        self.max_ms.fetch_max(millis, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    pub fn max_ms(&self) -> u64 {
        self.max_ms.load(Ordering::Relaxed)
    }

    /// Upper bound (ms) of the bucket containing the q-th quantile.
    pub fn quantile_upper_bound_ms(&self, q_percent: u64) -> u64 {
        let total = self.count();
        if total == 0 {
            return 0;
        }
        let target = (total * q_percent.min(100)).div_ceil(100).max(1);
        let mut acc = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            acc += b.load(Ordering::Relaxed);
            if acc >= target {
                return if i == 0 { 0 } else { (1u64 << i) - 1 };
            }
        }
        u64::MAX
    }
}

/// Bounded extension label (§108 allows "extension" as a label — but only
/// from a registered, size-capped set; everything else is `OTHER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExtLabel(u8);

impl ExtLabel {
    pub const OTHER: ExtLabel = ExtLabel(0);
    pub const MAX_REGISTERED: usize = 32;
}

#[derive(Debug, Default)]
pub struct ExtensionLabeler {
    names: Mutex<HashMap<String, u8>>,
}

impl ExtensionLabeler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a known extension at load time. Past the cap, further
    /// extensions share the `OTHER` label.
    pub fn register(&self, name: &str) -> ExtLabel {
        let mut m = lock(&self.names);
        if let Some(&i) = m.get(name) {
            return ExtLabel(i);
        }
        if m.len() >= ExtLabel::MAX_REGISTERED {
            return ExtLabel::OTHER;
        }
        let i = (m.len() + 1) as u8;
        m.insert(name.to_owned(), i);
        ExtLabel(i)
    }

    /// Looking up never registers: an arbitrary runtime string cannot
    /// grow the label set.
    pub fn label(&self, name: &str) -> ExtLabel {
        lock(&self.names)
            .get(name)
            .map_or(ExtLabel::OTHER, |&i| ExtLabel(i))
    }
}

#[derive(Debug)]
pub struct ResourceMetrics {
    by_label: Box<[Counters]>,
    ext_rejects: Box<[AtomicU64]>,
    current: [AtomicU64; KINDS],
    peak: [AtomicU64; KINDS],
    queue_wait: [WaitHistogram; PRIO],
    permit_wait: [WaitHistogram; KINDS],
}

impl Default for ResourceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceMetrics {
    pub fn new() -> Self {
        Self {
            by_label: (0..TRUST * PRIO * KINDS)
                .map(|_| Counters::default())
                .collect(),
            ext_rejects: (0..=ExtLabel::MAX_REGISTERED)
                .map(|_| AtomicU64::new(0))
                .collect(),
            current: Default::default(),
            peak: Default::default(),
            queue_wait: Default::default(),
            permit_wait: Default::default(),
        }
    }

    fn c(&self, l: Labels) -> &Counters {
        &self.by_label[l.index()]
    }

    pub fn record_reject(&self, l: Labels) {
        self.c(l).admission_rejects.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_deferred(&self, l: Labels) {
        self.c(l).deferred.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_eviction(&self, l: Labels) {
        self.c(l).evictions.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_shed(&self, l: Labels) {
        self.c(l).shed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn record_extension_reject(&self, ext: ExtLabel) {
        self.ext_rejects[ext.0 as usize].fetch_add(1, Ordering::Relaxed);
    }

    /// Current usage gauge; peak is tracked monotonically.
    pub fn observe_usage(&self, kind: ResourceKind, current: u64) {
        let i = kind.index();
        self.current[i].store(current, Ordering::Relaxed);
        self.peak[i].fetch_max(current, Ordering::Relaxed);
    }

    pub fn record_queue_wait(&self, p: WorkPriority, millis: u64) {
        self.queue_wait[p as usize].record(millis);
    }

    pub fn record_permit_wait(&self, kind: ResourceKind, millis: u64) {
        self.permit_wait[kind.index()].record(millis);
    }

    pub fn counters(&self, l: Labels) -> CounterValues {
        let c = self.c(l);
        CounterValues {
            admission_rejects: c.admission_rejects.load(Ordering::Relaxed),
            deferred: c.deferred.load(Ordering::Relaxed),
            evictions: c.evictions.load(Ordering::Relaxed),
            shed: c.shed.load(Ordering::Relaxed),
        }
    }

    pub fn extension_rejects(&self, ext: ExtLabel) -> u64 {
        self.ext_rejects[ext.0 as usize].load(Ordering::Relaxed)
    }

    pub fn current(&self, kind: ResourceKind) -> u64 {
        self.current[kind.index()].load(Ordering::Relaxed)
    }

    pub fn peak(&self, kind: ResourceKind) -> u64 {
        self.peak[kind.index()].load(Ordering::Relaxed)
    }

    pub fn queue_wait(&self, p: WorkPriority) -> &WaitHistogram {
        &self.queue_wait[p as usize]
    }

    pub fn permit_wait(&self, kind: ResourceKind) -> &WaitHistogram {
        &self.permit_wait[kind.index()]
    }

    /// The whole metric set's label cardinality, fixed at compile time.
    pub const fn series_upper_bound() -> usize {
        TRUST * PRIO * KINDS + ExtLabel::MAX_REGISTERED + 1
    }
}

// ------------------------------------------------------------ §109

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerEvent {
    StreamOpened,
    StreamClosed,
    BytesIn(u64),
    Rejected,
}

/// Local-debug view of one peer. Deliberately **not** `Serialize` and not
/// part of any telemetry export path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PeerDebugView {
    pub open_streams: u32,
    pub bytes_in: u64,
    pub rejects: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("already inspecting the maximum of {0} peers")]
pub struct TooManyDebugPeers(pub usize);

/// Opt-in: nothing is recorded for a peer until `enable` is called, so by
/// default there is no per-peer state at all.
#[derive(Debug)]
pub struct PeerDiagnostics {
    peers: HashMap<PeerKey, PeerDebugView>,
    max_peers: usize,
}

impl PeerDiagnostics {
    pub fn new(max_peers: usize) -> Self {
        Self {
            peers: HashMap::new(),
            max_peers: max_peers.max(1),
        }
    }

    pub fn enable(&mut self, peer: PeerKey) -> Result<(), TooManyDebugPeers> {
        if !self.peers.contains_key(&peer) && self.peers.len() >= self.max_peers {
            return Err(TooManyDebugPeers(self.max_peers));
        }
        self.peers.entry(peer).or_default();
        Ok(())
    }

    pub fn disable(&mut self, peer: &PeerKey) -> bool {
        self.peers.remove(peer).is_some()
    }

    /// Returns whether the event was recorded (false = peer not enabled).
    pub fn record(&mut self, peer: &PeerKey, e: PeerEvent) -> bool {
        let Some(v) = self.peers.get_mut(peer) else {
            return false;
        };
        match e {
            PeerEvent::StreamOpened => v.open_streams = v.open_streams.saturating_add(1),
            PeerEvent::StreamClosed => v.open_streams = v.open_streams.saturating_sub(1),
            PeerEvent::BytesIn(n) => v.bytes_in = v.bytes_in.saturating_add(n),
            PeerEvent::Rejected => v.rejects = v.rejects.saturating_add(1),
        }
        true
    }

    pub fn inspect(&self, peer: &PeerKey) -> Option<PeerDebugView> {
        self.peers.get(peer).copied()
    }
}

// ------------------------------------------------------------ §110

/// Debug/test leak detector. Disabled trackers cost one `None` check and
/// hand out zero-work guards, so it can stay wired into production paths.
#[derive(Debug, Clone, Default)]
pub struct LeakTracker {
    inner: Option<Arc<Mutex<HashMap<&'static str, u64>>>>,
}

#[derive(Debug)]
#[must_use = "dropping the guard marks the tracked resource as released"]
pub struct LeakGuard {
    inner: Option<Arc<Mutex<HashMap<&'static str, u64>>>>,
    kind: &'static str,
}

impl LeakTracker {
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    pub fn enabled() -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(HashMap::new()))),
        }
    }

    pub fn track(&self, kind: &'static str) -> LeakGuard {
        if let Some(m) = &self.inner {
            *lock(m).entry(kind).or_insert(0) += 1;
        }
        LeakGuard {
            inner: self.inner.clone(),
            kind,
        }
    }

    /// Kinds with unreleased items, sorted by name.
    pub fn outstanding(&self) -> Vec<(&'static str, u64)> {
        let Some(m) = &self.inner else {
            return Vec::new();
        };
        let mut v: Vec<_> = lock(m)
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(k, n)| (*k, *n))
            .collect();
        v.sort_unstable();
        v
    }

    pub fn assert_clean(&self) -> Result<(), Vec<(&'static str, u64)>> {
        let o = self.outstanding();
        if o.is_empty() {
            Ok(())
        } else {
            Err(o)
        }
    }
}

impl Drop for LeakGuard {
    fn drop(&mut self) {
        if let Some(m) = &self.inner {
            let mut g = lock(m);
            if let Some(n) = g.get_mut(self.kind) {
                *n = n.saturating_sub(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_protocol_ext::TrafficPriority::*;

    fn l(t: TrustClass, p: WorkPriority, k: ResourceKind) -> Labels {
        Labels {
            trust: t,
            priority: p,
            kind: k,
        }
    }

    #[test]
    fn counters_are_per_label_and_independent() {
        let m = ResourceMetrics::new();
        let a = l(TrustClass::Unknown, Bulk, ResourceKind::Memory);
        let b = l(TrustClass::Known, Bulk, ResourceKind::Memory);
        m.record_reject(a);
        m.record_reject(a);
        m.record_deferred(b);
        m.record_shed(a);
        m.record_eviction(b);
        assert_eq!(
            m.counters(a),
            CounterValues {
                admission_rejects: 2,
                shed: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            m.counters(b),
            CounterValues {
                deferred: 1,
                evictions: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn every_label_combination_has_a_distinct_slot() {
        let mut seen = std::collections::HashSet::new();
        for t in [
            TrustClass::Unknown,
            TrustClass::Known,
            TrustClass::Verified,
            TrustClass::Organization,
            TrustClass::Authority,
            TrustClass::LocalOwnDevice,
        ] {
            for p in [Critical, Control, Interactive, Normal, Bulk, Background] {
                for k in ResourceKind::ALL {
                    assert!(seen.insert(l(t, p, k).index()));
                }
            }
        }
        assert_eq!(seen.len(), TRUST * PRIO * KINDS);
        assert_eq!(seen.iter().max(), Some(&(TRUST * PRIO * KINDS - 1)));
    }

    #[test]
    fn extension_labels_are_bounded_and_lookup_never_registers() {
        let lab = ExtensionLabeler::new();
        let a = lab.register("calendar");
        assert_eq!(lab.register("calendar"), a);
        assert_eq!(lab.label("calendar"), a);
        assert_eq!(lab.label("never-registered"), ExtLabel::OTHER);
        for i in 0..100 {
            lab.register(&format!("ext-{i}"));
        }
        assert_eq!(lab.register("one-too-many"), ExtLabel::OTHER);
        let m = ResourceMetrics::new();
        m.record_extension_reject(lab.label("attacker-chosen-name-1"));
        m.record_extension_reject(lab.label("attacker-chosen-name-2"));
        assert_eq!(
            m.extension_rejects(ExtLabel::OTHER),
            2,
            "arbitrary names collapse into OTHER"
        );
    }

    #[test]
    fn series_count_is_a_compile_time_constant() {
        assert_eq!(ResourceMetrics::series_upper_bound(), 6 * 6 * 10 + 33);
    }

    #[test]
    fn usage_tracks_current_and_monotonic_peak() {
        let m = ResourceMetrics::new();
        m.observe_usage(ResourceKind::Streams, 5);
        m.observe_usage(ResourceKind::Streams, 14);
        m.observe_usage(ResourceKind::Streams, 3);
        assert_eq!(
            (
                m.current(ResourceKind::Streams),
                m.peak(ResourceKind::Streams)
            ),
            (3, 14)
        );
    }

    #[test]
    fn histogram_buckets_and_quantiles() {
        let h = WaitHistogram::default();
        assert_eq!(h.quantile_upper_bound_ms(95), 0);
        for _ in 0..95 {
            h.record(3); // bucket 2: [2,4)
        }
        for _ in 0..5 {
            h.record(1_000); // bucket 10: [512,1024)
        }
        assert_eq!(h.count(), 100);
        assert_eq!(h.quantile_upper_bound_ms(50), 3);
        assert_eq!(h.quantile_upper_bound_ms(99), 1023);
        assert_eq!(h.max_ms(), 1_000);
        h.record(u64::MAX); // lands in the open-ended last bucket, sum saturates
        assert_eq!(h.max_ms(), u64::MAX);
        let m = ResourceMetrics::new();
        m.record_queue_wait(Bulk, 0);
        assert_eq!(m.queue_wait(Bulk).count(), 1);
        m.record_permit_wait(ResourceKind::Memory, 7);
        assert_eq!(m.permit_wait(ResourceKind::Memory).count(), 1);
    }

    #[test]
    fn metrics_are_safe_to_hammer_from_many_threads() {
        let m = Arc::new(ResourceMetrics::new());
        let lab = l(TrustClass::Known, Normal, ResourceKind::Cpu);
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let m = Arc::clone(&m);
                std::thread::spawn(move || {
                    for i in 0..10_000 {
                        m.record_reject(lab);
                        m.observe_usage(ResourceKind::Cpu, i);
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(m.counters(lab).admission_rejects, 80_000);
        assert_eq!(m.peak(ResourceKind::Cpu), 9_999);
    }

    #[test]
    fn per_peer_diagnostics_are_opt_in_and_bounded() {
        let (a, b) = ([1; 32], [2; 32]);
        let mut d = PeerDiagnostics::new(1);
        assert!(
            !d.record(&a, PeerEvent::Rejected),
            "not enabled: nothing recorded"
        );
        assert_eq!(d.inspect(&a), None);
        d.enable(a).unwrap();
        assert_eq!(d.enable(b), Err(TooManyDebugPeers(1)));
        d.record(&a, PeerEvent::StreamOpened);
        d.record(&a, PeerEvent::StreamOpened);
        d.record(&a, PeerEvent::StreamClosed);
        d.record(&a, PeerEvent::BytesIn(100));
        d.record(&a, PeerEvent::Rejected);
        d.record(&a, PeerEvent::StreamClosed);
        d.record(&a, PeerEvent::StreamClosed); // extra close saturates
        assert_eq!(
            d.inspect(&a),
            Some(PeerDebugView {
                open_streams: 0,
                bytes_in: 100,
                rejects: 1
            })
        );
        assert!(d.disable(&a));
        assert_eq!(d.inspect(&a), None);
    }

    #[test]
    fn leak_tracker_reports_unreleased_items_by_kind() {
        let t = LeakTracker::enabled();
        let p1 = t.track("permit");
        let _p2 = t.track("permit");
        let task = t.track("task");
        assert_eq!(t.outstanding(), vec![("permit", 2), ("task", 1)]);
        drop(p1);
        drop(task);
        assert_eq!(t.assert_clean().unwrap_err(), vec![("permit", 1)]);
        drop(_p2);
        assert!(t.assert_clean().is_ok());
    }

    #[test]
    fn disabled_tracker_reports_nothing_and_costs_nothing() {
        let t = LeakTracker::disabled();
        let _g = t.track("permit");
        assert!(t.outstanding().is_empty());
        assert!(t.assert_clean().is_ok());
    }

    #[test]
    fn leak_guards_release_on_panic() {
        let t = LeakTracker::enabled();
        let t2 = t.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _g = t2.track("permit");
            panic!("boom");
        }));
        assert!(t.assert_clean().is_ok());
    }
}
