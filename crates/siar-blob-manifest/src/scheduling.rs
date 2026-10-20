//! §174, §179-185: how transfers share the machine, and how they are
//! observed.
//!
//! * [`RoutingInputs`] — §179, what the file service tells Part 03.
//! * [`TransferClass`] — §180's six classes.
//! * [`FairScheduler`] — §181-183. One 50 GB file must not block a voice
//!   note, one peer must not take every upload slot, one embedding
//!   product module must not starve another. Chunk-granular **deficit
//!   round robin** with weights per class, strict priority for
//!   `EmergencyCritical`, and slot caps per peer and per app namespace.
//!   Pure state: no clock, no thread, no allocation after registration
//!   beyond the entry list, so it costs nothing until it is asked.
//! * [`plan_path_change`] — §174-176: a path change is pause → rebind →
//!   resume-missing under the *same* `TransferId`/`BlobId`; this is the
//!   decision, not the transport.
//! * [`FileMetrics`] — §184, lock-free counters that cannot hold a
//!   content name because no field can.
//! * [`DiagnosticsSnapshot`] — §185.
//!
//! Part 08 will generalize resource fairness (§180); this is the file
//! subsystem's own, deliberately small version.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;

use crate::ids::TransferId;
use crate::store_error::Recoverability;

// ---------------------------------------------------------------------
// §179 routing inputs
// ---------------------------------------------------------------------

/// §179's six inputs, verbatim. The file service fills this in; routing
/// ([`siar_routing_policy`]'s job, not this crate's) returns a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingInputs {
    pub estimated_bytes: u64,
    pub bulk: bool,
    /// Offer/transfer expiry (§154), milliseconds.
    pub deadline_millis: Option<u64>,
    pub metered_allowed: bool,
    pub dtn_allowed: bool,
    pub multipath_allowed: bool,
}

impl RoutingInputs {
    /// §176: only small, critical things may enter DTN; bulk waits.
    pub fn dtn_eligible(&self, class: TransferClass, max_dtn_bytes: u64) -> bool {
        self.dtn_allowed
            && !self.bulk
            && self.estimated_bytes <= max_dtn_bytes
            && matches!(
                class,
                TransferClass::EmergencyCritical | TransferClass::InteractiveAttachment
            )
    }
}

// ---------------------------------------------------------------------
// §180 classes
// ---------------------------------------------------------------------

/// Declared best-first, so the derived order is the priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TransferClass {
    EmergencyCritical,
    InteractiveAttachment,
    UserRequested,
    BackgroundSync,
    Bulk,
    Relay,
}

impl TransferClass {
    /// DRR weight: bytes of credit per quantum, as a multiple.
    pub const fn weight(self) -> u64 {
        match self {
            Self::EmergencyCritical => 16,
            Self::InteractiveAttachment => 8,
            Self::UserRequested => 4,
            Self::BackgroundSync => 2,
            Self::Bulk | Self::Relay => 1,
        }
    }
}

// ---------------------------------------------------------------------
// §181-183 fair scheduler
// ---------------------------------------------------------------------

/// §183: which product module owns a transfer. A plain number — the
/// embedding SDK decides what the values mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AppNamespace(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// Credit added per round, per unit of weight.
    pub quantum_bytes: u64,
    pub max_in_flight: usize,
    /// §182
    pub max_in_flight_per_peer: usize,
    /// §183
    pub max_in_flight_per_app: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            quantum_bytes: 64 * 1024,
            max_in_flight: 8,
            max_in_flight_per_peer: 3,
            max_in_flight_per_app: 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerEntry {
    pub transfer: TransferId,
    pub peer: DeviceId,
    pub app: AppNamespace,
    pub class: TransferClass,
    /// Chunks not yet granted.
    pub remaining_chunks: u32,
    /// Size of the next chunk, for crediting.
    pub next_chunk_bytes: u32,
}

/// One chunk's worth of permission. Hand it back via
/// [`FairScheduler::complete`] when the chunk is done (or
/// [`FairScheduler::abandon`] if it was not sent), or its slot stays
/// taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a grant holds a slot until completed or abandoned"]
pub struct Grant {
    pub transfer: TransferId,
    pub peer: DeviceId,
    pub app: AppNamespace,
    pub chunk_bytes: u32,
}

struct Slot {
    e: SchedulerEntry,
    deficit: u64,
    paused: bool,
    in_flight: u32,
}

pub struct FairScheduler {
    cfg: SchedulerConfig,
    /// §42: a peer on a slow path (Bluetooth) gets its own, lower cap.
    peer_caps: std::collections::HashMap<DeviceId, usize>,
    slots: Vec<Slot>,
    cursor: usize,
    total_in_flight: usize,
}

impl FairScheduler {
    pub fn new(cfg: SchedulerConfig) -> Self {
        Self {
            cfg: SchedulerConfig {
                quantum_bytes: cfg.quantum_bytes.max(1),
                ..cfg
            },
            peer_caps: std::collections::HashMap::new(),
            slots: Vec::new(),
            cursor: 0,
            total_in_flight: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Registers (or replaces) a transfer.
    pub fn register(&mut self, e: SchedulerEntry) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.transfer == e.transfer) {
            s.e = e;
        } else {
            self.slots.push(Slot {
                e,
                deficit: 0,
                paused: false,
                in_flight: 0,
            });
        }
    }

    /// §174: paused transfers keep their place but are never granted.
    pub fn set_paused(&mut self, t: TransferId, paused: bool) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.transfer == t) {
            s.paused = paused;
        }
    }

    pub fn set_next_chunk_bytes(&mut self, t: TransferId, bytes: u32) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.transfer == t) {
            s.e.next_chunk_bytes = bytes;
        }
    }

    /// Drops a transfer; its in-flight slots are released.
    pub fn remove(&mut self, t: TransferId) {
        if let Some(i) = self.slots.iter().position(|s| s.e.transfer == t) {
            self.total_in_flight -= self.slots[i].in_flight as usize;
            self.slots.remove(i);
            if self.cursor > i {
                self.cursor -= 1;
            }
            if self.cursor >= self.slots.len() {
                self.cursor = 0;
            }
        }
    }

    fn in_flight_for(&self, f: impl Fn(&SchedulerEntry) -> bool) -> usize {
        self.slots
            .iter()
            .filter(|s| f(&s.e))
            .map(|s| s.in_flight as usize)
            .sum()
    }

    /// §42: overrides the per-peer slot cap for one peer (derived from
    /// [`crate::parallelism::effective_parallelism`] for its path).
    pub fn set_peer_cap(&mut self, peer: DeviceId, cap: usize) {
        self.peer_caps.insert(peer, cap.max(1));
    }

    pub fn peer_cap(&self, peer: DeviceId) -> usize {
        self.peer_caps
            .get(&peer)
            .copied()
            .unwrap_or(self.cfg.max_in_flight_per_peer)
    }

    fn eligible(&self, i: usize) -> bool {
        let s = &self.slots[i];
        if s.paused || s.e.remaining_chunks == 0 || s.e.next_chunk_bytes == 0 {
            return false;
        }
        let (peer, app) = (s.e.peer, s.e.app);
        self.total_in_flight < self.cfg.max_in_flight
            && self.in_flight_for(|e| e.peer == peer) < self.peer_cap(peer)
            && self.in_flight_for(|e| e.app == app) < self.cfg.max_in_flight_per_app
    }

    fn grant(&mut self, i: usize) -> Grant {
        self.cursor = (i + 1) % self.slots.len();
        self.total_in_flight += 1;
        let s = &mut self.slots[i];
        s.e.remaining_chunks -= 1;
        s.in_flight += 1;
        Grant {
            transfer: s.e.transfer,
            peer: s.e.peer,
            app: s.e.app,
            chunk_bytes: s.e.next_chunk_bytes,
        }
    }

    /// The next chunk to send, or `None` if nothing may be sent now
    /// (nothing registered, all paused, or every slot cap reached).
    pub fn next_grant(&mut self) -> Option<Grant> {
        let n = self.slots.len();
        if n == 0 {
            return None;
        }
        // Strict priority: emergencies first, round-robin among them.
        for k in 0..n {
            let i = (self.cursor + k) % n;
            if self.slots[i].e.class == TransferClass::EmergencyCritical && self.eligible(i) {
                return Some(self.grant(i));
            }
        }
        let eligible: Vec<usize> = (0..n).filter(|&i| self.eligible(i)).collect();
        if eligible.is_empty() {
            return None;
        }
        // Enough top-up rounds that the largest chunk can be afforded by
        // the lightest class.
        let biggest = eligible
            .iter()
            .map(|&i| u64::from(self.slots[i].e.next_chunk_bytes))
            .max()
            .unwrap_or(0);
        let rounds = biggest / self.cfg.quantum_bytes + 2;
        for _ in 0..=rounds {
            for k in 0..n {
                let i = (self.cursor + k) % n;
                if eligible.contains(&i)
                    && self.slots[i].deficit >= u64::from(self.slots[i].e.next_chunk_bytes)
                {
                    let cost = u64::from(self.slots[i].e.next_chunk_bytes);
                    self.slots[i].deficit -= cost;
                    return Some(self.grant(i));
                }
            }
            for &i in &eligible {
                let s = &mut self.slots[i];
                let add = self.cfg.quantum_bytes.saturating_mul(s.e.class.weight());
                // Capped: a transfer held back by a slot cap does not
                // bank an unbounded burst for later.
                let cap = u64::from(s.e.next_chunk_bytes).saturating_add(add);
                s.deficit = s.deficit.saturating_add(add).min(cap);
            }
        }
        None
    }

    /// The chunk was sent. Frees its slot; a finished transfer leaves.
    pub fn complete(&mut self, g: Grant) {
        self.release(g);
        if let Some(s) = self.slots.iter().find(|s| s.e.transfer == g.transfer) {
            if s.e.remaining_chunks == 0 && s.in_flight == 0 {
                self.remove(g.transfer);
            }
        }
    }

    /// The chunk was *not* sent (path dropped, send failed): the slot is
    /// freed and the chunk goes back on the transfer's remaining count.
    pub fn abandon(&mut self, g: Grant) {
        self.release(g);
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.transfer == g.transfer) {
            s.e.remaining_chunks = s.e.remaining_chunks.saturating_add(1);
        }
    }

    fn release(&mut self, g: Grant) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.e.transfer == g.transfer) {
            if s.in_flight > 0 {
                s.in_flight -= 1;
                self.total_in_flight -= 1;
            }
        }
    }

    pub fn in_flight(&self) -> usize {
        self.total_in_flight
    }
}

// ---------------------------------------------------------------------
// §174-176 path change
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    IrohRelay,
    Lan,
    WifiDirect,
    WifiAware,
    Ble,
    Dtn,
}

impl PathKind {
    /// Rough ordering used only to tell an upgrade from a downgrade.
    const fn speed_rank(self) -> u8 {
        match self {
            Self::Dtn => 0,
            Self::Ble => 1,
            Self::IrohRelay => 2,
            Self::WifiAware | Self::WifiDirect => 3,
            Self::Lan => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathChangeStep {
    /// Stop granting chunks; abandon what was in flight.
    PauseScheduler,
    /// Authenticate the new path before trusting it with chunks.
    AuthenticateNewPath(PathKind),
    /// §31: ask only for what is missing, same `TransferId`/`BlobId`.
    ResumeMissing { ranges: Vec<(u32, u32)> },
    /// §176: bulk waits; nothing is sent until a real path returns.
    HoldBulk,
    /// §176: a small critical transfer may be handed to DTN.
    HandToDtn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChangePlan {
    pub upgrade: bool,
    pub steps: Vec<PathChangeStep>,
}

/// The decision for "the path under a live transfer changed". `to` of
/// `None` means the link disappeared. Never restarts: every plan that
/// continues ends in `ResumeMissing` for the ranges still missing.
pub fn plan_path_change(
    from: PathKind,
    to: Option<PathKind>,
    class: TransferClass,
    inputs: &RoutingInputs,
    max_dtn_bytes: u64,
    missing: Vec<(u32, u32)>,
) -> PathChangePlan {
    match to {
        Some(new) => PathChangePlan {
            upgrade: new.speed_rank() > from.speed_rank(),
            steps: vec![
                PathChangeStep::PauseScheduler,
                PathChangeStep::AuthenticateNewPath(new),
                PathChangeStep::ResumeMissing { ranges: missing },
            ],
        },
        None => {
            let mut steps = vec![PathChangeStep::PauseScheduler];
            if inputs.dtn_eligible(class, max_dtn_bytes) {
                steps.push(PathChangeStep::HandToDtn);
            } else {
                steps.push(PathChangeStep::HoldBulk);
            }
            PathChangePlan {
                upgrade: false,
                steps,
            }
        }
    }
}

// ---------------------------------------------------------------------
// §184 metrics
// ---------------------------------------------------------------------

/// Counters only: no field can hold a file name, a path or a peer, so
/// "do not expose content names in telemetry" is a property of the type.
#[derive(Debug, Default)]
pub struct FileMetrics {
    hash_bytes: AtomicU64,
    hash_nanos: AtomicU64,
    encrypt_bytes: AtomicU64,
    encrypt_nanos: AtomicU64,
    disk_read_bytes: AtomicU64,
    disk_write_bytes: AtomicU64,
    network_bytes: AtomicU64,
    verified_bytes: AtomicU64,
    transfers_started: AtomicU64,
    transfers_resumed: AtomicU64,
    dedup_lookups: AtomicU64,
    dedup_hits: AtomicU64,
    gc_reclaimed_bytes: AtomicU64,
}

macro_rules! bump {
    ($($name:ident => $field:ident),* $(,)?) => {
        $(pub fn $name(&self, by: u64) {
            // Saturating, relaxed: a counter must never wrap or block.
            let _ = self.$field.fetch_update(
                Ordering::Relaxed, Ordering::Relaxed, |v| Some(v.saturating_add(by)));
        })*
    };
}

impl FileMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    bump!(
        add_hashed_nanos => hash_nanos,
        add_encrypted_nanos => encrypt_nanos,
        add_disk_read => disk_read_bytes,
        add_disk_write => disk_write_bytes,
        add_network => network_bytes,
        add_verified => verified_bytes,
        add_gc_reclaimed => gc_reclaimed_bytes,
    );

    pub fn record_hash(&self, bytes: u64, nanos: u64) {
        self.add_bytes(&self.hash_bytes, bytes);
        self.add_hashed_nanos(nanos);
    }

    pub fn record_encrypt(&self, bytes: u64, nanos: u64) {
        self.add_bytes(&self.encrypt_bytes, bytes);
        self.add_encrypted_nanos(nanos);
    }

    pub fn record_transfer_started(&self, resumed: bool) {
        self.add_bytes(&self.transfers_started, 1);
        if resumed {
            self.add_bytes(&self.transfers_resumed, 1);
        }
    }

    pub fn record_dedup_lookup(&self, hit: bool) {
        self.add_bytes(&self.dedup_lookups, 1);
        if hit {
            self.add_bytes(&self.dedup_hits, 1);
        }
    }

    fn add_bytes(&self, f: &AtomicU64, by: u64) {
        let _ = f.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_add(by))
        });
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let l = |f: &AtomicU64| f.load(Ordering::Relaxed);
        MetricsSnapshot {
            hash_bytes_per_sec: rate(l(&self.hash_bytes), l(&self.hash_nanos)),
            encrypt_bytes_per_sec: rate(l(&self.encrypt_bytes), l(&self.encrypt_nanos)),
            disk_read_bytes: l(&self.disk_read_bytes),
            disk_write_bytes: l(&self.disk_write_bytes),
            network_bytes: l(&self.network_bytes),
            verified_bytes: l(&self.verified_bytes),
            transfers_started: l(&self.transfers_started),
            resume_rate_permille: permille(l(&self.transfers_resumed), l(&self.transfers_started)),
            dedup_hit_rate_permille: permille(l(&self.dedup_hits), l(&self.dedup_lookups)),
            gc_reclaimed_bytes: l(&self.gc_reclaimed_bytes),
        }
    }
}

fn rate(bytes: u64, nanos: u64) -> u64 {
    if nanos == 0 {
        return 0;
    }
    // u128: bytes * 1e9 overflows u64 at ~18 GB.
    u64::try_from(u128::from(bytes) * 1_000_000_000 / u128::from(nanos)).unwrap_or(u64::MAX)
}

fn permille(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        0
    } else {
        u64::try_from(u128::from(part.min(whole)) * 1000 / u128::from(whole)).unwrap_or(1000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub hash_bytes_per_sec: u64,
    pub encrypt_bytes_per_sec: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub network_bytes: u64,
    pub verified_bytes: u64,
    pub transfers_started: u64,
    pub resume_rate_permille: u64,
    pub dedup_hit_rate_permille: u64,
    pub gc_reclaimed_bytes: u64,
}

// ---------------------------------------------------------------------
// §185 diagnostics
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferDiagnostic {
    pub transfer: TransferId,
    pub path: Option<PathKind>,
    pub bytes_verified: u64,
    pub chunks_received: u32,
    pub chunks_total: u32,
}

/// §185's list. No names, paths or peers: ids and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsSnapshot {
    pub active: Vec<TransferDiagnostic>,
    pub storage_bytes: u64,
    pub cache_bytes: u64,
    pub staging_bytes: u64,
    /// §187's class, not the error text (which could carry detail).
    pub last_error: Option<Recoverability>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn entry(
        class: TransferClass,
        peer: DeviceId,
        app: u16,
        chunks: u32,
        size: u32,
    ) -> SchedulerEntry {
        SchedulerEntry {
            transfer: TransferId::new(),
            peer,
            app: AppNamespace(app),
            class,
            remaining_chunks: chunks,
            next_chunk_bytes: size,
        }
    }

    /// Runs the scheduler to exhaustion, one grant at a time, completing
    /// each immediately. Returns transfer ids in grant order.
    fn drain(s: &mut FairScheduler) -> Vec<TransferId> {
        let mut order = Vec::new();
        while let Some(g) = s.next_grant() {
            order.push(g.transfer);
            s.complete(g);
        }
        order
    }

    #[test]
    fn a_huge_bulk_file_does_not_block_small_attachments() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        let (p1, p2) = (DeviceId::new(), DeviceId::new());
        let big = entry(TransferClass::Bulk, p1, 1, 50_000, 1 << 20);
        let image = entry(TransferClass::InteractiveAttachment, p2, 1, 3, 64 * 1024);
        let voice = entry(TransferClass::InteractiveAttachment, p2, 1, 2, 32 * 1024);
        let (big_id, image_id, voice_id) = (big.transfer, image.transfer, voice.transfer);
        s.register(big);
        s.register(image);
        s.register(voice);

        let mut order = Vec::new();
        for _ in 0..40 {
            let g = s.next_grant().unwrap();
            order.push(g.transfer);
            s.complete(g);
        }
        let last_small = order
            .iter()
            .rposition(|t| *t == image_id || *t == voice_id)
            .expect("small transfers were served");
        assert!(
            last_small < 12,
            "all 5 small chunks done within the first 12 grants, got {last_small}"
        );
        assert!(order.contains(&big_id), "and the big file still moves");
    }

    #[test]
    fn emergency_has_strict_priority() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        let p = DeviceId::new();
        let bulk = entry(TransferClass::Bulk, p, 1, 10, 1000);
        let sos = entry(
            TransferClass::EmergencyCritical,
            DeviceId::new(),
            1,
            3,
            1000,
        );
        let sos_id = sos.transfer;
        s.register(bulk);
        s.register(sos);
        let first_three: Vec<_> = (0..3)
            .map(|_| {
                let g = s.next_grant().unwrap();
                let t = g.transfer;
                s.complete(g);
                t
            })
            .collect();
        assert!(first_three.iter().all(|t| *t == sos_id));
    }

    #[test]
    fn weights_give_heavier_classes_proportionally_more() {
        let mut s = FairScheduler::new(SchedulerConfig {
            max_in_flight: 100,
            max_in_flight_per_peer: 100,
            max_in_flight_per_app: 100,
            ..SchedulerConfig::default()
        });
        let a = entry(
            TransferClass::UserRequested,
            DeviceId::new(),
            1,
            10_000,
            64 * 1024,
        );
        let b = entry(TransferClass::Bulk, DeviceId::new(), 1, 10_000, 64 * 1024);
        let (a_id, b_id) = (a.transfer, b.transfer);
        s.register(a);
        s.register(b);
        let mut counts: HashMap<TransferId, u32> = HashMap::new();
        for _ in 0..500 {
            let g = s.next_grant().unwrap();
            *counts.entry(g.transfer).or_default() += 1;
            s.complete(g);
        }
        let (ca, cb) = (counts[&a_id], counts[&b_id]);
        assert!(cb > 0, "bulk is never starved");
        // weight 4 vs 1 → about 4:1
        assert!((3..=5).contains(&(ca / cb)), "ratio was {ca}:{cb}");
    }

    #[test]
    fn one_peer_cannot_take_every_slot() {
        let cfg = SchedulerConfig {
            max_in_flight: 8,
            max_in_flight_per_peer: 2,
            ..SchedulerConfig::default()
        };
        let mut s = FairScheduler::new(cfg);
        let greedy = DeviceId::new();
        for _ in 0..4 {
            s.register(entry(TransferClass::UserRequested, greedy, 1, 100, 1000));
        }
        let other = entry(TransferClass::UserRequested, DeviceId::new(), 1, 100, 1000);
        let other_id = other.transfer;
        s.register(other);

        let mut held = Vec::new();
        while let Some(g) = s.next_grant() {
            held.push(g);
        }
        assert_eq!(held.iter().filter(|g| g.peer == greedy).count(), 2);
        assert!(
            held.iter().any(|g| g.transfer == other_id),
            "the other peer got a slot"
        );
    }

    #[test]
    fn a_slow_path_peer_can_be_capped_below_the_default() {
        let mut s = FairScheduler::new(SchedulerConfig {
            max_in_flight: 8,
            max_in_flight_per_peer: 4,
            max_in_flight_per_app: 8,
            ..SchedulerConfig::default()
        });
        let ble = DeviceId::new();
        let lan = DeviceId::new();
        s.set_peer_cap(ble, 1);
        s.register(entry(TransferClass::UserRequested, ble, 1, 50, 1000));
        s.register(entry(TransferClass::UserRequested, lan, 1, 50, 1000));
        let mut held = Vec::new();
        while let Some(g) = s.next_grant() {
            held.push(g);
        }
        assert_eq!(held.iter().filter(|g| g.peer == ble).count(), 1);
        assert_eq!(held.iter().filter(|g| g.peer == lan).count(), 4);
        s.set_peer_cap(ble, 0);
        assert_eq!(s.peer_cap(ble), 1, "never zero");
    }

    #[test]
    fn app_namespaces_have_their_own_caps() {
        let cfg = SchedulerConfig {
            max_in_flight: 8,
            max_in_flight_per_peer: 8,
            max_in_flight_per_app: 2,
            ..SchedulerConfig::default()
        };
        let mut s = FairScheduler::new(cfg);
        for _ in 0..4 {
            s.register(entry(TransferClass::Bulk, DeviceId::new(), 1, 10, 1000));
        }
        s.register(entry(TransferClass::Bulk, DeviceId::new(), 2, 10, 1000));
        let mut held = Vec::new();
        while let Some(g) = s.next_grant() {
            held.push(g);
        }
        assert_eq!(held.iter().filter(|g| g.app == AppNamespace(1)).count(), 2);
        // App 2 is not crowded out by app 1, and gets its own cap of 2.
        assert_eq!(held.iter().filter(|g| g.app == AppNamespace(2)).count(), 2);
    }

    #[test]
    fn slots_are_released_by_complete_and_abandon() {
        let mut s = FairScheduler::new(SchedulerConfig {
            max_in_flight: 1,
            ..SchedulerConfig::default()
        });
        let e = entry(TransferClass::UserRequested, DeviceId::new(), 1, 2, 1000);
        s.register(e);
        let g = s.next_grant().unwrap();
        assert!(s.next_grant().is_none(), "the only slot is taken");
        s.abandon(g);
        assert_eq!(s.in_flight(), 0);
        let g = s.next_grant().unwrap();
        s.complete(g);
        let g = s.next_grant().unwrap();
        s.complete(g);
        assert!(s.next_grant().is_none());
        assert!(s.is_empty(), "a finished transfer leaves");
    }

    #[test]
    fn abandoning_a_chunk_puts_it_back_so_nothing_is_lost() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        s.register(entry(
            TransferClass::UserRequested,
            DeviceId::new(),
            1,
            3,
            1000,
        ));
        let g = s.next_grant().unwrap();
        s.abandon(g);
        assert_eq!(drain(&mut s).len(), 3);
    }

    #[test]
    fn paused_transfers_are_skipped_then_resume_in_place() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        let e = entry(TransferClass::UserRequested, DeviceId::new(), 1, 2, 1000);
        let id = e.transfer;
        s.register(e);
        s.set_paused(id, true);
        assert!(s.next_grant().is_none());
        s.set_paused(id, false);
        assert_eq!(drain(&mut s).len(), 2);
    }

    #[test]
    fn removing_a_transfer_releases_its_slots() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        let e = entry(TransferClass::UserRequested, DeviceId::new(), 1, 5, 1000);
        let id = e.transfer;
        s.register(e);
        let _g = s.next_grant().unwrap();
        assert_eq!(s.in_flight(), 1);
        s.remove(id);
        assert_eq!(s.in_flight(), 0);
        assert!(s.is_empty());
    }

    #[test]
    fn chunks_bigger_than_the_quantum_still_get_through() {
        let mut s = FairScheduler::new(SchedulerConfig {
            quantum_bytes: 1,
            ..SchedulerConfig::default()
        });
        s.register(entry(
            TransferClass::Bulk,
            DeviceId::new(),
            1,
            2,
            4 * 1024 * 1024,
        ));
        assert_eq!(drain(&mut s).len(), 2);
    }

    #[test]
    fn zero_sized_or_empty_entries_are_never_granted() {
        let mut s = FairScheduler::new(SchedulerConfig::default());
        s.register(entry(TransferClass::Bulk, DeviceId::new(), 1, 0, 1000));
        s.register(entry(TransferClass::Bulk, DeviceId::new(), 1, 5, 0));
        assert!(s.next_grant().is_none());
    }

    // ---- §174-176, §179 ----

    fn inputs(bulk: bool, bytes: u64) -> RoutingInputs {
        RoutingInputs {
            estimated_bytes: bytes,
            bulk,
            deadline_millis: None,
            metered_allowed: false,
            dtn_allowed: true,
            multipath_allowed: true,
        }
    }

    #[test]
    fn a_new_path_pauses_authenticates_and_resumes_only_the_missing() {
        let plan = plan_path_change(
            PathKind::IrohRelay,
            Some(PathKind::Lan),
            TransferClass::UserRequested,
            &inputs(false, 10),
            1024,
            vec![(4, 9)],
        );
        assert!(plan.upgrade);
        assert_eq!(
            plan.steps,
            vec![
                PathChangeStep::PauseScheduler,
                PathChangeStep::AuthenticateNewPath(PathKind::Lan),
                PathChangeStep::ResumeMissing {
                    ranges: vec![(4, 9)]
                },
            ]
        );
    }

    #[test]
    fn ble_to_wifi_is_an_upgrade_and_wifi_to_ble_is_not() {
        let up = plan_path_change(
            PathKind::Ble,
            Some(PathKind::WifiDirect),
            TransferClass::Bulk,
            &inputs(true, 1),
            0,
            vec![],
        );
        let down = plan_path_change(
            PathKind::WifiDirect,
            Some(PathKind::Ble),
            TransferClass::Bulk,
            &inputs(true, 1),
            0,
            vec![],
        );
        assert!(up.upgrade && !down.upgrade);
    }

    #[test]
    fn losing_the_link_hands_small_critical_things_to_dtn_and_holds_bulk() {
        let lost = |class, i: RoutingInputs| {
            plan_path_change(PathKind::Lan, None, class, &i, 4096, vec![(0, 1)]).steps
        };
        assert_eq!(
            lost(TransferClass::EmergencyCritical, inputs(false, 100)),
            vec![PathChangeStep::PauseScheduler, PathChangeStep::HandToDtn]
        );
        assert_eq!(
            lost(TransferClass::Bulk, inputs(true, 100)),
            vec![PathChangeStep::PauseScheduler, PathChangeStep::HoldBulk]
        );
        assert_eq!(
            lost(TransferClass::InteractiveAttachment, inputs(false, 1 << 30)),
            vec![PathChangeStep::PauseScheduler, PathChangeStep::HoldBulk],
            "too big for DTN"
        );
        let mut no_dtn = inputs(false, 10);
        no_dtn.dtn_allowed = false;
        assert_eq!(
            lost(TransferClass::EmergencyCritical, no_dtn),
            vec![PathChangeStep::PauseScheduler, PathChangeStep::HoldBulk]
        );
    }

    #[test]
    fn class_order_is_priority_order() {
        use TransferClass::*;
        let mut v = vec![
            Relay,
            Bulk,
            EmergencyCritical,
            UserRequested,
            InteractiveAttachment,
            BackgroundSync,
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                EmergencyCritical,
                InteractiveAttachment,
                UserRequested,
                BackgroundSync,
                Bulk,
                Relay
            ]
        );
        assert!(EmergencyCritical.weight() > Bulk.weight());
    }

    // ---- §184-185 ----

    #[test]
    fn metrics_report_rates_without_overflow_and_never_divide_by_zero() {
        let m = FileMetrics::new();
        assert_eq!(m.snapshot().hash_bytes_per_sec, 0);
        m.record_hash(1_000_000_000_000, 1_000_000_000); // 1 TB in 1 s
        m.record_encrypt(500, 1_000_000_000);
        m.record_transfer_started(false);
        m.record_transfer_started(true);
        m.record_dedup_lookup(true);
        m.record_dedup_lookup(false);
        m.record_dedup_lookup(false);
        m.add_verified(10);
        m.add_gc_reclaimed(7);
        let s = m.snapshot();
        assert_eq!(s.hash_bytes_per_sec, 1_000_000_000_000);
        assert_eq!(s.encrypt_bytes_per_sec, 500);
        assert_eq!(s.resume_rate_permille, 500);
        assert_eq!(s.dedup_hit_rate_permille, 333);
        assert_eq!((s.verified_bytes, s.gc_reclaimed_bytes), (10, 7));
    }

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let m = FileMetrics::new();
        m.add_network(u64::MAX);
        m.add_network(5);
        assert_eq!(m.snapshot().network_bytes, u64::MAX);
    }

    #[test]
    fn diagnostics_round_trip_and_carry_only_ids_and_numbers() {
        let d = DiagnosticsSnapshot {
            active: vec![TransferDiagnostic {
                transfer: TransferId::new(),
                path: Some(PathKind::Lan),
                bytes_verified: 10,
                chunks_received: 2,
                chunks_total: 9,
            }],
            storage_bytes: 1,
            cache_bytes: 2,
            staging_bytes: 3,
            last_error: Some(Recoverability::RefetchData),
        };
        let bytes = postcard::to_allocvec(&d).unwrap();
        assert_eq!(
            postcard::from_bytes::<DiagnosticsSnapshot>(&bytes).unwrap(),
            d
        );
    }
}
