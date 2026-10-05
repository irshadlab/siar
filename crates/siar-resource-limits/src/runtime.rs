//! §187 core public API, §188 feature integration API.
//!
//! [`ResourceRuntime`] is the one object the rest of the system holds. It
//! owns and wires the policy pieces (budget, pools, slots, storage
//! reservations and class ledger, outbox, per-peer quotas and inbound
//! gates, pressure/mode control, metrics) so feature code states *what it
//! wants* and gets a typed permit or a typed [`ResourceError`] — never
//! manual plumbing:
//!
//! ```text
//! let permit = rt.admit(&request, now)?;                       // §187
//! let msg    = rt.messaging().acquire_message(...)?;           // §188
//! let xfer   = rt.files().acquire_transfer(...)?;
//! let bundle = rt.dtn().reserve_bundle(...)?;
//! ```
//!
//! Everything is non-blocking: a refusal is an error value, and
//! [`ResourceError::Deferred`] means "park this durably and retry when
//! capacity frees" (an async wrapper is a loop around the call plus the
//! caller's own wake-up; this crate stays executor-agnostic). Every permit
//! is RAII — dropping it releases everything it holds, including on
//! cancellation or panic, and acquisition is ordered so a late failure
//! automatically unwinds the earlier steps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::abuse::authorize_priority;
use crate::admission::{AdmissionResult, ResourceOwner, ResourceRequest, WorkPriority};
use crate::budget::{BudgetError, BudgetPermit, GlobalBudget, Subsystem};
use crate::connections::PeerKey;
use crate::dtn_intake::{relay_intake, IntakeDecision, IntakeReject, RelayIntake};
use crate::error::ResourceError;
use crate::event_log_pressure::{OutboxAccounting, OutboxClass, OutboxLimits, OutboxReject};
use crate::feature_limits::{FeatureLimitError, FeatureLimits};
use crate::inbound::{InboundGate, InboundLimits, InboundReject};
use crate::metrics::{Labels, ResourceMetrics};
use crate::os_memory::{HoldTimes, MemoryPressureAggregator};
use crate::peer_quota::{PeerQuota, PeerUsageCounters, PeerUsageDelta, TrustClass};
use crate::policy_layers::HardLimits;
use crate::pressure::{shed, ModeController, OverloadMode, PressureSignals, ShedDecision};
use crate::profile::{ConfigError, ResourceConfig, ResourceProfile};
use crate::reservation::{PermitBundle, ReservationError, ResourcePools};
use crate::slots::{SlotPermit, TransferKind, TransferSlots};
use crate::snapshot::{CountUsage, ResourceSnapshot, ResourceUsage, StorageUsage};
use crate::storage::{CriticalStorageReserve, PressureState, ReservationId, StorageReservations};
use crate::storage_classes::{ChargeOutcome, StorageClass, StorageLedger};
use crate::types::ResourceKind;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long a storage reservation lives before it must be committed or
/// is reclaimed as stale.
pub const STORAGE_LEASE_MILLIS: u64 = 10 * 60 * 1000;
/// How long an outbox entry may wait for delivery before it expires.
pub const OUTBOX_RETENTION_MILLIS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("inconsistent resource config: {0:?}")]
    Config(Vec<ConfigError>),
    #[error("inconsistent feature limits: {0:?}")]
    Features(Vec<FeatureLimitError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub cpu_permits: u64,
    pub min_calm_millis: u64,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            cpu_permits: 4,
            min_calm_millis: 5_000,
        }
    }
}

struct Inner {
    cfg: ResourceConfig,
    features: FeatureLimits,
    pools: ResourcePools,
    budget: GlobalBudget,
    slots: TransferSlots,
    storage: Mutex<StorageReservations>,
    classes: Mutex<StorageLedger>,
    outbox: Mutex<OutboxAccounting>,
    peers: Mutex<HashMap<PeerKey, PeerUsageCounters>>,
    gates: Mutex<HashMap<PeerKey, InboundGate>>,
    control: Mutex<(ModeController, MemoryPressureAggregator)>,
    mode: Mutex<OverloadMode>,
    metrics: Arc<ResourceMetrics>,
}

#[derive(Clone)]
pub struct ResourceRuntime {
    inner: Arc<Inner>,
}

fn subsystem_of(owner: &ResourceOwner) -> Subsystem {
    match owner {
        ResourceOwner::Messaging => Subsystem::Messaging,
        ResourceOwner::Files => Subsystem::Files,
        ResourceOwner::Dtn => Subsystem::Dtn,
        ResourceOwner::Calls => Subsystem::Calls,
        _ => Subsystem::Extensions,
    }
}

impl ResourceRuntime {
    pub fn new(profile: ResourceProfile) -> Result<Self, RuntimeError> {
        Self::with_config(
            ResourceConfig::for_profile(profile),
            HardLimits::for_profile(profile),
            FeatureLimits::for_profile(profile),
            RuntimeOptions::default(),
        )
    }

    /// Validates the whole configuration up front: a runtime never exists
    /// with an inconsistent policy.
    pub fn with_config(
        cfg: ResourceConfig,
        hard: HardLimits,
        features: FeatureLimits,
        opts: RuntimeOptions,
    ) -> Result<Self, RuntimeError> {
        cfg.validate(&hard).map_err(RuntimeError::Config)?;
        features
            .validate(&cfg, &hard)
            .map_err(RuntimeError::Features)?;

        let q = &cfg.queues.capacities;
        let queue_slots = [
            q.critical,
            q.control,
            q.interactive,
            q.normal,
            q.bulk,
            q.background,
        ]
        .iter()
        .map(|&n| n as u64)
        .sum();
        let pools = ResourcePools::new(&[
            (ResourceKind::Streams, u64::from(cfg.network.max_streams)),
            (
                ResourceKind::Connections,
                u64::from(cfg.network.max_sessions),
            ),
            (ResourceKind::Cpu, opts.cpu_permits),
            (ResourceKind::QueueSlots, queue_slots),
        ]);
        let budget = GlobalBudget::new(
            cfg.memory.global_bytes,
            cfg.memory.sub_budgets,
            cfg.memory.max_borrow_bytes,
        );
        let slots = TransferSlots::new(
            cfg.effective_max_transfers(),
            cfg.network.max_transfers_per_peer,
            // keep one slot for interactive sends when there is room for it
            u32::from(cfg.effective_max_transfers() > 1),
        );
        let storage = StorageReservations::new(
            cfg.storage.quota_bytes,
            CriticalStorageReserve {
                reserved_bytes: cfg.storage.critical_reserve_bytes,
            },
        );
        let outbox = OutboxAccounting::new(OutboxLimits {
            max_count: usize::try_from(features.messaging.max_pending_outbound)
                .unwrap_or(usize::MAX),
            max_bytes: features
                .messaging
                .max_pending_outbound
                .saturating_mul(features.messaging.max_message_bytes),
            critical_reserve_count: usize::try_from(
                (features.messaging.max_pending_outbound / 10).max(1),
            )
            .unwrap_or(1),
            critical_reserve_bytes: features
                .messaging
                .max_message_bytes
                .saturating_mul((features.messaging.max_pending_outbound / 10).max(1)),
            bulk_percent: 50,
        });
        Ok(Self {
            inner: Arc::new(Inner {
                classes: Mutex::new(StorageLedger::from_config(&cfg)),
                cfg,
                features,
                pools,
                budget,
                slots,
                storage: Mutex::new(storage),
                outbox: Mutex::new(outbox),
                peers: Mutex::new(HashMap::new()),
                gates: Mutex::new(HashMap::new()),
                control: Mutex::new((
                    ModeController::new(opts.min_calm_millis),
                    MemoryPressureAggregator::new(HoldTimes::default()),
                )),
                mode: Mutex::new(OverloadMode::Normal),
                metrics: Arc::new(ResourceMetrics::new()),
            }),
        })
    }

    pub fn config(&self) -> &ResourceConfig {
        &self.inner.cfg
    }

    pub fn features(&self) -> &FeatureLimits {
        &self.inner.features
    }

    pub fn metrics(&self) -> &Arc<ResourceMetrics> {
        &self.inner.metrics
    }

    pub fn mode(&self) -> OverloadMode {
        *lock(&self.inner.mode)
    }

    pub fn messaging(&self) -> Messaging<'_> {
        Messaging { rt: self }
    }

    pub fn files(&self) -> Files<'_> {
        Files { rt: self }
    }

    pub fn dtn(&self) -> Dtn<'_> {
        Dtn { rt: self }
    }

    // ------------------------------------------------- pressure input

    /// Platform adapters report raw facts; policy stays here.
    pub fn report_android_trim(&self, level: i32, now_millis: u64) {
        lock(&self.inner.control)
            .1
            .report_android_trim(level, now_millis);
    }

    pub fn report_ios_memory_warning(&self, now_millis: u64) {
        lock(&self.inner.control).1.report_ios_warning(now_millis);
    }

    /// Re-evaluate pressure (call periodically and after platform
    /// reports): merges OS memory signals with `other` (CPU, thermal,
    /// storage, energy), advances the mode controller, and switches
    /// budget borrowing off whenever the runtime is not in Normal mode.
    pub fn evaluate_pressure(&self, other: PressureSignals, now_millis: u64) -> OverloadMode {
        let mut c = lock(&self.inner.control);
        let sig = c.1.apply_to(other, now_millis);
        let mode = c.0.observe(&sig, now_millis);
        drop(c);
        *lock(&self.inner.mode) = mode;
        self.inner
            .budget
            .set_borrowing_enabled(mode == OverloadMode::Normal);
        mode
    }

    fn labels(&self, trust: TrustClass, priority: WorkPriority, kind: ResourceKind) -> Labels {
        Labels {
            trust,
            priority,
            kind,
        }
    }

    /// Mode-based shedding shared by every entry point.
    fn gate_mode(
        &self,
        trust: TrustClass,
        priority: WorkPriority,
        durable: bool,
        kind: ResourceKind,
    ) -> Result<(), ResourceError> {
        let mode = self.mode();
        match shed(mode, priority, durable) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Defer => {
                self.inner
                    .metrics
                    .record_shed(self.labels(trust, priority, kind));
                Err(ResourceError::Deferred)
            }
            ShedDecision::Drop => {
                self.inner
                    .metrics
                    .record_shed(self.labels(trust, priority, kind));
                Err(ResourceError::Overloaded(mode))
            }
        }
    }

    fn reject<T>(
        &self,
        trust: TrustClass,
        priority: WorkPriority,
        kind: ResourceKind,
        e: ResourceError,
    ) -> Result<T, ResourceError> {
        let l = self.labels(trust, priority, kind);
        if e == ResourceError::Deferred {
            self.inner.metrics.record_deferred(l);
        } else {
            self.inner.metrics.record_reject(l);
        }
        Err(e)
    }

    fn map_budget(e: BudgetError) -> ResourceError {
        match e {
            BudgetError::GlobalCeiling { .. } | BudgetError::BorrowingDisabled => {
                ResourceError::MemoryPressure
            }
            BudgetError::BorrowCapExceeded => ResourceError::QuotaExceeded(ResourceKind::Memory),
            BudgetError::Overflow => ResourceError::MalformedRequest(ResourceKind::Memory),
        }
    }

    fn map_reservation(e: ReservationError) -> ResourceError {
        match e {
            ReservationError::Exhausted {
                kind: ResourceKind::Streams,
                ..
            } => ResourceError::TooManyStreams,
            ReservationError::Exhausted {
                kind: ResourceKind::Connections,
                ..
            } => ResourceError::TooManyConnections,
            ReservationError::Exhausted { kind, .. } => ResourceError::BudgetExhausted(kind),
            ReservationError::Overflow(k) => ResourceError::MalformedRequest(k),
        }
    }

    fn reserve_storage(
        &self,
        bytes: u64,
        owner: ResourceOwner,
        now: u64,
        durable: bool,
    ) -> Result<StorageHold, ResourceError> {
        if bytes == 0 {
            return Ok(StorageHold { rt: None, id: None });
        }
        match lock(&self.inner.storage).reserve(bytes, owner, STORAGE_LEASE_MILLIS, now, durable) {
            Ok(id) => Ok(StorageHold {
                rt: Some(self.clone()),
                id: Some(id),
            }),
            Err(AdmissionResult::Deferred(_)) => Err(ResourceError::Deferred),
            Err(_) => Err(ResourceError::StorageFull),
        }
    }

    // --------------------------------------------------- §187 admit

    /// The generic entry point. Checks the request against the device's
    /// absolute limits (→ `HardLimitExceeded`, permanent), applies mode
    /// shedding, then acquires memory (from the owner's sub-budget),
    /// streams, connections and storage together — all or nothing.
    pub fn admit(
        &self,
        req: &ResourceRequest,
        now_millis: u64,
    ) -> Result<ResourcePermit, ResourceError> {
        self.admit_for(TrustClass::LocalOwnDevice, req, now_millis)
    }

    pub fn admit_for(
        &self,
        trust: TrustClass,
        req: &ResourceRequest,
        now_millis: u64,
    ) -> Result<ResourcePermit, ResourceError> {
        let cfg = &self.inner.cfg;
        let priority = authorize_priority(trust, req.priority, false);
        let kind = ResourceKind::Memory;
        if req.memory > cfg.memory.global_bytes
            || req.storage
                > cfg
                    .storage
                    .quota_bytes
                    .saturating_sub(cfg.storage.critical_reserve_bytes)
            || req.streams > cfg.network.max_streams
            || req.connections > cfg.network.max_sessions
        {
            return self.reject(trust, priority, kind, ResourceError::HardLimitExceeded);
        }
        self.gate_mode(trust, priority, req.durable, kind)?;

        let memory = if req.memory > 0 {
            Some(
                self.inner
                    .budget
                    .try_acquire(subsystem_of(&req.owner), req.memory)
                    .map_err(Self::map_budget)
                    .or_else(|e| self.reject(trust, priority, kind, e))?,
            )
        } else {
            None
        };
        let bundle = self
            .inner
            .pools
            .acquire(
                &[
                    (ResourceKind::Streams, u64::from(req.streams)),
                    (ResourceKind::Connections, u64::from(req.connections)),
                ],
                None,
            )
            .map_err(Self::map_reservation)
            .or_else(|e| self.reject(trust, priority, kind, e))?;
        let storage = self
            .reserve_storage(req.storage, req.owner.clone(), now_millis, req.durable)
            .or_else(|e| self.reject(trust, priority, ResourceKind::Storage, e))?;
        Ok(ResourcePermit {
            _memory: memory,
            _bundle: bundle,
            storage,
            priority,
        })
    }

    // -------------------------------------------------- diagnostics

    pub fn snapshot(&self) -> ResourceSnapshot {
        let i = &self.inner;
        let (reserved, quota) = (lock(&i.storage).reserved_bytes(), i.cfg.storage.quota_bytes);
        let used_classes = lock(&i.classes).total_used();
        let mem_used = i.budget.total_used();
        i.metrics.observe_usage(ResourceKind::Memory, mem_used);
        ResourceSnapshot {
            memory: ResourceUsage::new(
                mem_used,
                i.cfg.memory.global_bytes,
                i.metrics.peak(ResourceKind::Memory),
            ),
            storage: StorageUsage {
                used: used_classes,
                reserved,
                quota,
                critical_reserve: i.cfg.storage.critical_reserve_bytes,
            },
            connections: CountUsage {
                used: i.pools.in_use(ResourceKind::Connections) as u32,
                limit: i.cfg.network.max_sessions,
            },
            streams: CountUsage {
                used: i.pools.in_use(ResourceKind::Streams) as u32,
                limit: i.cfg.network.max_streams,
            },
            network: Default::default(),
            pressure: match self.mode() {
                OverloadMode::Normal => PressureState::Normal,
                OverloadMode::Shedding => PressureState::Elevated,
                OverloadMode::Overload => PressureState::Critical,
                OverloadMode::CriticalOnly => PressureState::Exhausted,
            },
        }
    }
}

// ------------------------------------------------------------ permits

#[derive(Debug)]
struct StorageHold {
    rt: Option<ResourceRuntime>,
    id: Option<ReservationId>,
}

impl std::fmt::Debug for ResourceRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRuntime")
            .field("profile", &self.inner.cfg.profile)
            .finish_non_exhaustive()
    }
}

impl StorageHold {
    fn commit(&mut self) {
        if let (Some(rt), Some(id)) = (self.rt.take(), self.id.take()) {
            lock(&rt.inner.storage).commit(id);
        }
    }
}

impl Drop for StorageHold {
    fn drop(&mut self) {
        if let (Some(rt), Some(id)) = (self.rt.take(), self.id.take()) {
            lock(&rt.inner.storage).cancel(id);
        }
    }
}

/// Everything a generic [`ResourceRuntime::admit`] granted.
#[derive(Debug)]
#[must_use = "dropping the permit releases everything it holds"]
pub struct ResourcePermit {
    _memory: Option<BudgetPermit>,
    _bundle: PermitBundle,
    storage: StorageHold,
    priority: WorkPriority,
}

impl ResourcePermit {
    /// The (trust-clamped) priority the request was admitted at.
    pub fn priority(&self) -> WorkPriority {
        self.priority
    }

    /// The staged bytes became permanent: stop treating the reservation
    /// as a lease that expires or is cancelled on drop.
    pub fn commit_storage(&mut self) {
        self.storage.commit();
    }
}

#[derive(Debug)]
struct PeerCharge {
    rt: ResourceRuntime,
    peer: PeerKey,
    delta: PeerUsageDelta,
}

impl Drop for PeerCharge {
    fn drop(&mut self) {
        let mut m = lock(&self.rt.inner.peers);
        if let Some(u) = m.get_mut(&self.peer) {
            u.release(self.delta);
        }
    }
}

impl ResourceRuntime {
    fn charge_peer(
        &self,
        peer: PeerKey,
        trust: TrustClass,
        delta: PeerUsageDelta,
        durable: bool,
    ) -> Result<PeerCharge, ResourceError> {
        let quota = PeerQuota::for_trust_class(trust).clamped_to(&self.inner.cfg);
        let mut m = lock(&self.inner.peers);
        if !m.contains_key(&peer) && m.len() >= self.inner.cfg.peers.max_tracked_peers as usize {
            // Bounded: forget a peer that currently holds nothing.
            let idle = m
                .iter()
                .find(|(_, u)| {
                    u.active_streams == 0
                        && u.staged_files == 0
                        && u.dtn_relay_bytes == 0
                        && u.pending_messages == 0
                        && u.inbound_bytes == 0
                })
                .map(|(k, _)| *k);
            match idle {
                Some(k) => {
                    m.remove(&k);
                }
                None => return Err(ResourceError::Busy),
            }
        }
        match m
            .entry(peer)
            .or_default()
            .try_charge(&quota, delta, durable)
        {
            AdmissionResult::Accepted => {}
            AdmissionResult::Deferred(_) => return Err(ResourceError::Deferred),
            _ => return Err(ResourceError::QuotaExceeded(ResourceKind::Streams)),
        }
        drop(m);
        Ok(PeerCharge {
            rt: self.clone(),
            peer,
            delta,
        })
    }
}

// --------------------------------------------------------- §188 messaging

pub struct Messaging<'a> {
    rt: &'a ResourceRuntime,
}

#[derive(Debug)]
#[must_use = "dropping the permit removes the message from the outbox accounting (delivered or cancelled)"]
pub struct MessagePermit {
    rt: ResourceRuntime,
    id: u64,
}

impl Drop for MessagePermit {
    fn drop(&mut self) {
        lock(&self.rt.inner.outbox).remove(self.id);
    }
}

#[derive(Debug)]
#[must_use = "call is complete when this is dropped"]
pub struct InboundPermit {
    rt: ResourceRuntime,
    peer: PeerKey,
}

impl Drop for InboundPermit {
    fn drop(&mut self) {
        if let Some(g) = lock(&self.rt.inner.gates).get_mut(&self.peer) {
            g.completed();
        }
    }
}

impl Messaging<'_> {
    /// Reserve outbox space for a message about to be sent.
    pub fn acquire_message(
        &self,
        trust: TrustClass,
        bytes: u64,
        attachments: u64,
        priority: WorkPriority,
        now_millis: u64,
    ) -> Result<MessagePermit, ResourceError> {
        let rt = self.rt;
        let priority = authorize_priority(trust, priority, false);
        let kind = ResourceKind::QueueSlots;
        if rt
            .inner
            .features
            .messaging
            .check_message(bytes, attachments)
            .is_err()
        {
            return rt.reject(trust, priority, kind, ResourceError::HardLimitExceeded);
        }
        rt.gate_mode(trust, priority, true, kind)?;
        let class = if priority <= WorkPriority::Control {
            OutboxClass::Critical
        } else {
            OutboxClass::Normal
        };
        match lock(&rt.inner.outbox).try_add(
            class,
            bytes,
            now_millis.saturating_add(OUTBOX_RETENTION_MILLIS),
            now_millis,
        ) {
            Ok(id) => Ok(MessagePermit { rt: rt.clone(), id }),
            Err(OutboxReject::BulkFull | OutboxReject::CountFull | OutboxReject::BytesFull) => {
                rt.reject(trust, priority, kind, ResourceError::Busy)
            }
            Err(OutboxReject::AlreadyExpired) => {
                rt.reject(trust, priority, kind, ResourceError::DeadlineExpired)
            }
        }
    }

    /// Gate an incoming message from `peer` (size, rate, pending count).
    pub fn admit_inbound(
        &self,
        peer: PeerKey,
        trust: TrustClass,
        bytes: u64,
        now_millis: u64,
    ) -> Result<InboundPermit, ResourceError> {
        let rt = self.rt;
        let mut gates = lock(&rt.inner.gates);
        if !gates.contains_key(&peer)
            && gates.len() >= rt.inner.cfg.peers.max_tracked_peers as usize
        {
            match gates
                .iter()
                .find(|(_, g)| g.pending() == 0)
                .map(|(k, _)| *k)
            {
                Some(k) => {
                    gates.remove(&k);
                }
                None => {
                    return rt.reject(
                        trust,
                        WorkPriority::Normal,
                        ResourceKind::QueueSlots,
                        ResourceError::Busy,
                    )
                }
            }
        }
        let gate = gates
            .entry(peer)
            .or_insert_with(|| InboundGate::new(InboundLimits::for_trust(trust), now_millis));
        match gate.admit(bytes, now_millis) {
            Ok(()) => Ok(InboundPermit {
                rt: rt.clone(),
                peer,
            }),
            Err(e) => {
                drop(gates);
                let err = match e {
                    InboundReject::TooLarge { .. } => ResourceError::HardLimitExceeded,
                    InboundReject::RateLimited => ResourceError::RateLimited,
                    InboundReject::TooManyPending => ResourceError::Busy,
                };
                rt.reject(trust, WorkPriority::Normal, ResourceKind::QueueSlots, err)
            }
        }
    }
}

// ------------------------------------------------------------ §188 files

pub struct Files<'a> {
    rt: &'a ResourceRuntime,
}

/// A running transfer's reservations: its slot, chunk-buffer memory,
/// staging space and its share of the peer's quota.
#[derive(Debug)]
#[must_use = "dropping the permit frees the slot, memory, staging space and peer quota"]
pub struct TransferPermit {
    // field order = drop order: staging first, then memory, slot, peer quota
    staging: StorageHold,
    _memory: BudgetPermit,
    _slot: SlotPermit,
    _peer: PeerCharge,
    manifest_entries: u64,
    staged_bytes: u64,
}

impl TransferPermit {
    pub fn manifest_entries(&self) -> u64 {
        self.manifest_entries
    }

    pub fn staged_bytes(&self) -> u64 {
        self.staged_bytes
    }

    /// The staged file became permanent.
    pub fn commit_storage(&mut self) {
        self.staging.commit();
    }
}

impl Files<'_> {
    /// Validate the offer from its declared numbers, then take — in this
    /// order, each step unwinding automatically if a later one fails — the
    /// peer's quota share, a transfer slot, chunk-buffer memory, and a
    /// staging reservation. `Deferred` means park it durably and retry.
    pub fn acquire_transfer(
        &self,
        peer: PeerKey,
        trust: TrustClass,
        file_size: u64,
        chunk_size: u64,
        interactive: bool,
        now_millis: u64,
    ) -> Result<TransferPermit, ResourceError> {
        let rt = self.rt;
        let f = &rt.inner.features.files;
        let priority = authorize_priority(
            trust,
            if interactive {
                WorkPriority::Interactive
            } else {
                WorkPriority::Bulk
            },
            false,
        );
        let kind = ResourceKind::Storage;
        let entries = match f.check_offer(file_size, chunk_size) {
            Ok(n) => n,
            Err(v) if v.field == "files.chunk_size_is_zero" => {
                return rt.reject(
                    trust,
                    priority,
                    kind,
                    ResourceError::MalformedRequest(ResourceKind::Memory),
                )
            }
            Err(_) => return rt.reject(trust, priority, kind, ResourceError::HardLimitExceeded),
        };
        rt.gate_mode(trust, priority, true, kind)?;

        let staged = file_size.min(f.max_staging_bytes);
        let peer_charge = rt
            .charge_peer(
                peer,
                trust,
                PeerUsageDelta {
                    staged_files: 1,
                    active_streams: 1,
                    ..Default::default()
                },
                true,
            )
            .or_else(|e| rt.reject(trust, priority, kind, e))?;
        let slot = rt
            .inner
            .slots
            .try_acquire(
                peer,
                if interactive {
                    TransferKind::Interactive
                } else {
                    TransferKind::Background
                },
            )
            .or_else(|_| rt.reject(trust, priority, kind, ResourceError::Deferred))?;
        let memory = rt
            .inner
            .budget
            .try_acquire(
                Subsystem::Files,
                f.max_parallel_chunks.saturating_mul(chunk_size),
            )
            .map_err(ResourceRuntime::map_budget)
            .or_else(|e| rt.reject(trust, priority, ResourceKind::Memory, e))?;
        let staging = rt
            .reserve_storage(staged, ResourceOwner::Files, now_millis, true)
            .or_else(|e| rt.reject(trust, priority, kind, e))?;
        Ok(TransferPermit {
            staging,
            _memory: memory,
            _slot: slot,
            _peer: peer_charge,
            manifest_entries: entries,
            staged_bytes: staged,
        })
    }

    pub fn active_transfers(&self) -> u32 {
        self.rt.inner.slots.active()
    }
}

// ------------------------------------------------------------- §188 dtn

pub struct Dtn<'a> {
    rt: &'a ResourceRuntime,
}

#[derive(Debug)]
#[must_use = "dropping the reservation releases the relay space and the peer's quota share"]
pub struct BundleReservation {
    rt: ResourceRuntime,
    bytes: u64,
    _peer: PeerCharge,
    /// If set, the relay store is over its quota: the store must evict
    /// bundles of *strictly lower* priority than this to make room, and
    /// call [`Self::report_evicted`] with the bytes it freed.
    pub evict_lower_than: Option<WorkPriority>,
    /// Cross-class eviction the storage ledger required (cache first).
    pub evicted_from_other_classes: Vec<(StorageClass, u64)>,
}

impl BundleReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn report_evicted(&self, bytes: u64) {
        lock(&self.rt.inner.classes).release(StorageClass::Relay, bytes);
    }
}

impl Drop for BundleReservation {
    fn drop(&mut self) {
        lock(&self.rt.inner.classes).release(StorageClass::Relay, self.bytes);
    }
}

impl Dtn<'_> {
    pub fn reserve_bundle(
        &self,
        peer: PeerKey,
        trust: TrustClass,
        bytes: u64,
        replication_budget: u64,
        hop_limit: u64,
        priority: WorkPriority,
    ) -> Result<BundleReservation, ResourceError> {
        let rt = self.rt;
        let d = &rt.inner.features.dtn;
        let priority = authorize_priority(trust, priority, false);
        let kind = ResourceKind::Storage;
        if d.check_bundle(bytes, replication_budget, hop_limit)
            .is_err()
        {
            return rt.reject(trust, priority, kind, ResourceError::HardLimitExceeded);
        }
        rt.gate_mode(trust, priority, true, kind)?;

        let quota = PeerQuota::for_trust_class(trust).clamped_to(&rt.inner.cfg);
        let peer_remaining = {
            let m = lock(&rt.inner.peers);
            quota
                .max_dtn_relay_bytes
                .saturating_sub(m.get(&peer).map_or(0, |u| u.dtn_relay_bytes))
        };
        let (relay_used, relay_quota) = {
            let l = lock(&rt.inner.classes);
            (
                l.used(StorageClass::Relay),
                rt.inner.cfg.storage.relay_quota_bytes,
            )
        };
        let decision = relay_intake(&RelayIntake {
            priority,
            bundle_bytes: bytes,
            relay_quota_remaining: relay_quota.saturating_sub(relay_used),
            peer_quota_remaining: peer_remaining,
            storage: rt.snapshot().effective_pressure(),
        });
        let evict_lower_than = match decision {
            IntakeDecision::Accept => None,
            IntakeDecision::AcceptEvictingBelow(p) => Some(p),
            IntakeDecision::Reject(IntakeReject::PeerQuota | IntakeReject::RelayQuotaFull) => {
                return rt.reject(
                    trust,
                    priority,
                    kind,
                    ResourceError::QuotaExceeded(ResourceKind::Storage),
                )
            }
            IntakeDecision::Reject(IntakeReject::StoragePressure) => {
                return rt.reject(trust, priority, kind, ResourceError::StorageFull)
            }
        };

        let peer_charge = rt
            .charge_peer(
                peer,
                trust,
                PeerUsageDelta {
                    dtn_relay_bytes: bytes,
                    ..Default::default()
                },
                true,
            )
            .or_else(|e| rt.reject(trust, priority, kind, e))?;
        let mut evicted = Vec::new();
        let mut classes = lock(&rt.inner.classes);
        // Relay-class quota is enforced by the intake decision above (the
        // store evicts its own lower-priority bundles when told to); the
        // ledger here keeps the *shared ceiling* honest across classes.
        let charged = match classes.try_charge(StorageClass::Relay, bytes) {
            Ok(()) => Ok(()),
            Err(ChargeOutcome::NeedsEviction(plan)) => {
                classes.apply_plan(&plan);
                evicted = plan;
                classes.try_charge(StorageClass::Relay, bytes)
            }
            Err(e) => Err(e),
        };
        drop(classes);
        match charged {
            Ok(()) => Ok(BundleReservation {
                rt: rt.clone(),
                bytes,
                _peer: peer_charge,
                evict_lower_than,
                evicted_from_other_classes: evicted,
            }),
            Err(ChargeOutcome::ClassQuotaExceeded) if evict_lower_than.is_some() => {
                // Relay is full but the store was told to evict: account
                // the bundle by forcing the class charge after eviction is
                // reported; here we conservatively defer instead of
                // over-committing the ledger.
                rt.reject(trust, priority, kind, ResourceError::Deferred)
            }
            Err(ChargeOutcome::ClassQuotaExceeded) => rt.reject(
                trust,
                priority,
                kind,
                ResourceError::QuotaExceeded(ResourceKind::Storage),
            ),
            Err(_) => rt.reject(trust, priority, kind, ResourceError::StorageFull),
        }
    }
}
