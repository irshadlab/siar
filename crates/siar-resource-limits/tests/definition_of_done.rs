//! §191 Definition of Done, executable. One test per bullet, named after
//! it, so `cargo test --test definition_of_done` is the completion
//! checklist. (§189's "implement first" list is exercised here too.)

use std::path::PathBuf;
use std::sync::Arc;

use siar_protocol_ext::TrafficPriority::*;
use siar_resource_limits::accounting::{ChargeClass, PoolError, TenantId, TenantPools};
use siar_resource_limits::admission::{AdmissionResult, ResourceOwner};
use siar_resource_limits::admission_pipeline::{ConnectionAdmission, ConnectionAdmissionLimits};
use siar_resource_limits::aging::{AgingPolicy, AgingQueue};
use siar_resource_limits::budget::{GlobalBudget, Subsystem};
use siar_resource_limits::cancel::OperationScope;
use siar_resource_limits::coalesce::{KeyedCoalescer, RealtimeRing};
use siar_resource_limits::event_log_pressure::{
    OutboxAccounting, OutboxClass, OutboxLimits, WriterLimits, WriterQueue,
};
use siar_resource_limits::extension_quota::{
    ExtensionResourceLimits, ExtensionUsageCounters, ExtensionUsageDelta,
};
use siar_resource_limits::fanout::{ClientQueue, Update};
use siar_resource_limits::feature_limits::FeatureLimits;
use siar_resource_limits::feedback::{developer_lines, hot_pools, NamedUsage};
use siar_resource_limits::frame::FrameDecoder;
use siar_resource_limits::metrics::{Labels, ResourceMetrics};
use siar_resource_limits::modes::{apply_modes, ModeFlags, ModeTunables};
use siar_resource_limits::negotiation_limits::{NegotiationGate, NegotiationLimits};
use siar_resource_limits::os_memory::{HoldTimes, MemoryPressureAggregator};
use siar_resource_limits::peer_quota::{PeerQuota, PeerUsageCounters, PeerUsageDelta, TrustClass};
use siar_resource_limits::policy_layers::HardLimits;
use siar_resource_limits::pressure::{
    shed, ModeController, OverloadMode, PressureSignals, ShedDecision,
};
use siar_resource_limits::profile::{ResourceConfig, ResourceProfile};
use siar_resource_limits::queue::QueueCapacities;
use siar_resource_limits::reservation::ResourcePools;
use siar_resource_limits::restart::{
    plan_recovery, restore_reservations, DurableResId, DurableReservation, OpId, RecoveryInput,
    WorkEntry,
};
use siar_resource_limits::routing_limits::DiscoveryTable;
use siar_resource_limits::safety::DecompressionGuard;
use siar_resource_limits::scheduler::{Job, Scheduler, ShardKind, SubmitOutcome};
use siar_resource_limits::slots::{TransferKind, TransferSlots};
use siar_resource_limits::snapshot::{CountUsage, ResourceSnapshot, ResourceUsage};
use siar_resource_limits::storage::{CriticalStorageReserve, PressureState, StorageReservations};
use siar_resource_limits::storage_classes::{ChargeOutcome, StorageClass, StorageLedger};
use siar_resource_limits::storm::SourceClass;
use siar_resource_limits::tasks::TaskRegistry;
use siar_resource_limits::types::ResourceKind;
use siar_resource_limits::verification::{VerificationBudget, VerifyLimits};

const PROFILES: [ResourceProfile; 5] = [
    ResourceProfile::Embedded,
    ResourceProfile::MobileLow,
    ResourceProfile::MobileNormal,
    ResourceProfile::Desktop,
    ResourceProfile::Server,
];

fn peer(i: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..4].copy_from_slice(&i.to_le_bytes());
    k
}

/// - no unbounded queue exists on external/high-volume data paths
#[test]
fn dod_01_no_unbounded_queue_on_external_paths() {
    const N: u64 = 200_000;
    let mut c: ClientQueue<u64> = ClientQueue::new(64);
    let mut w: WriterQueue<u64> = WriterQueue::new(WriterLimits {
        max_batch_items: 8,
        max_batch_bytes: 1 << 20,
        queue_items: 64,
        queue_bytes: 1 << 30,
    });
    let mut a: AgingQueue<u64> = AgingQueue::new(
        QueueCapacities {
            critical: 64,
            control: 64,
            interactive: 64,
            normal: 64,
            bulk: 64,
            background: 64,
        },
        AgingPolicy::default(),
    );
    let mut k: KeyedCoalescer<u64, u64> = KeyedCoalescer::new(64);
    let mut r: RealtimeRing<u64> = RealtimeRing::new(64, 1_000);
    let mut d = DiscoveryTable::new(64);
    let reg = TaskRegistry::new(64);
    let mut guards = Vec::new();
    for i in 0..N {
        c.push(Update::Droppable(i));
        let _ = w.try_enqueue(i, 1);
        a.enqueue(Bulk, i, true, i);
        k.push(i, i);
        r.push(i, i);
        d.observe(peer(i as u32), i, i);
        if let Ok(g) = reg.register("flood", Normal, i) {
            guards.push(g);
        }
    }
    assert!(
        c.len() <= 64
            && w.len() <= 64
            && a.len() <= 6 * 64
            && k.len() <= 64
            && r.len() <= 64
            && d.len() <= 64
            && reg.active() <= 64
    );
}

/// - every large allocation is preceded by validated limits
#[test]
fn dod_02_allocations_follow_validated_limits() {
    let mut f = FrameDecoder::new(1 << 20);
    assert!(f.feed(&(100u32 << 20).to_be_bytes()).is_err());
    assert_eq!(f.buffered_bytes(), 0);
    let files = FeatureLimits::for_profile(ResourceProfile::MobileNormal).files;
    assert!(files.check_offer(u64::MAX, 1).is_err());
    assert!(NegotiationGate::new(NegotiationLimits::default())
        .check_offer(&[1 << 30])
        .is_err());
    let mut g = DecompressionGuard::new(1 << 20, 100, 4096);
    assert!(g.record(1, 10 << 20).is_err());
}

/// - files cannot consume all memory/storage
#[test]
fn dod_03_files_cannot_consume_all_memory_or_storage() {
    for p in PROFILES {
        let cfg = ResourceConfig::for_profile(p);
        let fl = FeatureLimits::for_profile(p);
        assert_eq!(
            fl.validate(&cfg, &HardLimits::for_profile(p)),
            Ok(()),
            "{p:?}"
        );
        assert!(
            fl.files.worst_case_chunk_memory()
                <= u128::from(cfg.memory.sub_budgets[Subsystem::Files as usize])
        );
        assert!(
            fl.files.max_staging_bytes
                < cfg.storage.quota_bytes - cfg.storage.critical_reserve_bytes
        );
    }
}

/// - one peer cannot monopolize streams or bandwidth
#[test]
fn dod_04_one_peer_cannot_monopolize() {
    let cfg = ResourceConfig::for_profile(ResourceProfile::Embedded);
    let q = PeerQuota::for_trust_class(TrustClass::LocalOwnDevice).clamped_to(&cfg);
    assert!(q.max_active_streams <= cfg.network.max_streams);
    let mut u = PeerUsageCounters::default();
    let granted = (0..1_000)
        .filter(|_| {
            u.try_charge(
                &q,
                PeerUsageDelta {
                    active_streams: 1,
                    ..Default::default()
                },
                false,
            ) == AdmissionResult::Accepted
        })
        .count() as u32;
    assert!(granted <= cfg.network.max_streams);
    let slots = TransferSlots::new(10, 2, 0);
    let _a = slots
        .try_acquire(peer(1), TransferKind::Background)
        .unwrap();
    let _b = slots
        .try_acquire(peer(1), TransferKind::Background)
        .unwrap();
    assert!(
        slots
            .try_acquire(peer(1), TransferKind::Interactive)
            .is_err(),
        "per-peer cap"
    );
    assert!(
        slots.try_acquire(peer(2), TransferKind::Background).is_ok(),
        "others unaffected"
    );
}

/// - one extension cannot monopolize runtime resources
#[test]
fn dod_05_one_extension_cannot_monopolize() {
    let lim = ExtensionResourceLimits {
        max_memory_bytes: 1000,
        max_queued_ops: 10,
        max_streams: 2,
        max_storage_bytes: 1000,
    };
    let mut greedy = ExtensionUsageCounters::default();
    let mut other = ExtensionUsageCounters::default();
    let d = ExtensionUsageDelta {
        memory_bytes: 600,
        queued_ops: 1,
        streams: 1,
        storage_bytes: 0,
    };
    assert_eq!(greedy.try_charge(&lim, d, false), AdmissionResult::Accepted);
    assert!(
        !matches!(greedy.try_charge(&lim, d, false), AdmissionResult::Accepted),
        "capped"
    );
    assert_eq!(
        other.try_charge(&lim, d, false),
        AdmissionResult::Accepted,
        "independent counters"
    );
}

/// - bulk traffic cannot starve control/critical traffic
#[test]
fn dod_06_bulk_cannot_starve_control_or_critical() {
    let mut q: AgingQueue<&str> = AgingQueue::new(
        QueueCapacities::conservative_default(),
        AgingPolicy {
            step_millis: 10,
            floor: Interactive,
        },
    );
    q.enqueue(Background, "ancient bulk", true, 0);
    q.enqueue(Control, "ack", true, 1_000_000);
    q.enqueue(Critical, "sos", true, 1_000_000);
    assert_eq!(q.dequeue(1_000_000), Some("sos"));
    assert_eq!(q.dequeue(1_000_000), Some("ack"));
    assert_eq!(
        q.dequeue(1_000_000),
        Some("ancient bulk"),
        "and bulk is not starved forever either"
    );
}

/// - SOS/security events have reserved capacity
#[test]
fn dod_07_sos_has_reserved_capacity() {
    let mut s = StorageReservations::new(
        1_000,
        CriticalStorageReserve {
            reserved_bytes: 300,
        },
    );
    while s
        .reserve(100, ResourceOwner::Files, 60_000, 0, true)
        .is_ok()
    {}
    assert!(s
        .reserve(300, ResourceOwner::Messaging, 60_000, 0, true)
        .is_ok());
    let mut o = OutboxAccounting::new(OutboxLimits {
        max_count: 10,
        max_bytes: 1_000,
        critical_reserve_count: 2,
        critical_reserve_bytes: 200,
        bulk_percent: 50,
    });
    while o.try_add(OutboxClass::Normal, 10, 100, 0).is_ok() {}
    assert!(o.try_add(OutboxClass::Critical, 10, 100, 0).is_ok());
    let (a, b) = (TenantId(1), TenantId(2));
    let t = TenantPools::new(100, &[(a, 20), (b, 30)]).unwrap();
    let _fill = t.try_charge(a, ChargeClass::Normal, 50).unwrap();
    assert!(t.try_charge(b, ChargeClass::Critical, 30).is_ok());
    assert!(matches!(
        t.try_charge(TenantId(9), ChargeClass::Critical, 1),
        Err(PoolError::UnknownTenant)
    ));
}

/// - durable work defers rather than disappears under overload
#[test]
fn dod_08_durable_work_defers_not_disappears() {
    assert_eq!(
        shed(OverloadMode::CriticalOnly, Bulk, true),
        ShedDecision::Defer
    );
    let mut s = StorageReservations::new(100, CriticalStorageReserve { reserved_bytes: 0 });
    assert!(matches!(
        s.reserve(101, ResourceOwner::Files, 1, 0, true),
        Err(AdmissionResult::Deferred(_))
    ));
    let mut u = PeerUsageCounters::default();
    let q = PeerQuota::for_trust_class(TrustClass::Unknown);
    assert!(matches!(
        u.try_charge(
            &q,
            PeerUsageDelta {
                staged_files: 99,
                ..Default::default()
            },
            true
        ),
        AdmissionResult::Deferred(_)
    ));
    let sched: Scheduler<u8> = Scheduler::new(
        QueueCapacities {
            critical: 1,
            control: 1,
            interactive: 1,
            normal: 1,
            bulk: 1,
            background: 1,
        },
        AgingPolicy::default(),
        ResourcePools::new(&[]),
        Arc::new(ResourceMetrics::new()),
    );
    let j = |d| Job {
        payload: 0u8,
        priority: Normal,
        durable: d,
        demand: vec![],
    };
    assert_eq!(
        sched.submit(ShardKind::Cpu, j(true), TrustClass::Known, 0),
        SubmitOutcome::Queued
    );
    assert_eq!(
        sched.submit(ShardKind::Cpu, j(true), TrustClass::Known, 0),
        SubmitOutcome::Deferred
    );
}

/// - stale ephemeral work can be dropped/coalesced
#[test]
fn dod_09_stale_ephemeral_work_is_dropped_or_coalesced() {
    let mut r = RealtimeRing::new(8, 100);
    r.push(0, "old frame");
    r.push(500, "fresh frame");
    assert_eq!(r.pop_fresh(550), Some("fresh frame"));
    let mut k = KeyedCoalescer::new(4);
    for i in 0..1000 {
        k.push("typing", i);
    }
    assert_eq!(k.drain(), vec![("typing", 999)]);
    assert_eq!(
        shed(OverloadMode::Overload, Bulk, false),
        ShedDecision::Drop
    );
}

/// - storage has separate durable/cache/staging/relay quotas
#[test]
fn dod_10_storage_has_separate_class_quotas() {
    let cfg = ResourceConfig::for_profile(ResourceProfile::MobileNormal);
    let mut l = StorageLedger::from_config(&cfg);
    let cache_quota = cfg.storage.quota_bytes / 8;
    assert!(l.try_charge(StorageClass::Cache, cache_quota).is_ok());
    assert_eq!(
        l.try_charge(StorageClass::Cache, 1),
        Err(ChargeOutcome::ClassQuotaExceeded)
    );
    assert!(l
        .try_charge(StorageClass::Staging, cfg.storage.quota_bytes / 4)
        .is_ok());
    assert!(l
        .try_charge(StorageClass::Relay, cfg.storage.relay_quota_bytes)
        .is_ok());
    assert_eq!(
        l.try_charge(StorageClass::Relay, 1),
        Err(ChargeOutcome::ClassQuotaExceeded)
    );
    assert!(
        l.try_charge(StorageClass::Durable, 1).is_ok(),
        "other classes do not exhaust durable"
    );
}

/// - active calls can reserve realtime resources
#[test]
fn dod_11_active_calls_reserve_realtime_resources() {
    let t = ModeTunables {
        transfer_concurrency: 4,
        discovery_concurrency: 2,
        scan_concurrency: 4,
        cpu_permits: 4,
        ui_cache_bytes: 64,
        background_cache_bytes: 100,
        queue_scale_percent: 100,
        bulk_rate_bytes_per_sec: 1_000_000,
        critical_queue_reserve: 10,
        dtn_critical_reserve: 20,
        unknown_peer_slots: 16,
        control_reserve: 8,
        call_reserved_bandwidth: 500_000,
        call_reserved_cpu_permits: 1,
        call_reserved_memory: 32,
        accept_new_bulk: true,
        background_sync_allowed: true,
    };
    let on = apply_modes(
        &t,
        &t,
        &ModeFlags {
            call_active: true,
            ..Default::default()
        },
    );
    assert_eq!(
        (
            on.call_reserved_bandwidth,
            on.call_reserved_cpu_permits,
            on.call_reserved_memory
        ),
        (500_000, 1, 32)
    );
    let b = GlobalBudget::new(100, [10, 10, 10, 50, 10], 0);
    assert!(
        b.try_acquire(Subsystem::Calls, 50).is_ok(),
        "the call's guaranteed share is always available to it"
    );
}

/// - Android/iOS pressure signals tighten shared Rust policy
#[test]
fn dod_12_platform_pressure_tightens_shared_policy() {
    for raw in [
        |a: &mut MemoryPressureAggregator| a.report_android_trim(80, 0),
        |a: &mut MemoryPressureAggregator| a.report_ios_warning(0),
    ] {
        let mut agg = MemoryPressureAggregator::new(HoldTimes::default());
        let mut ctl = ModeController::new(1_000);
        assert_eq!(
            ctl.observe(&agg.apply_to(PressureSignals::default(), 1), 1),
            OverloadMode::Normal
        );
        raw(&mut agg);
        let mode = ctl.observe(&agg.apply_to(PressureSignals::default(), 2), 2);
        assert!(mode >= OverloadMode::Overload, "{mode:?}");
        assert_eq!(shed(mode, Bulk, false), ShedDecision::Drop);
    }
}

/// - cancellation/panic releases permits safely
#[test]
fn dod_13_cancellation_and_panic_release_permits() {
    let pools = ResourcePools::new(&[(ResourceKind::Memory, 10)]);
    let mut scope = OperationScope::root();
    scope.adopt(pools.acquire(&[(ResourceKind::Memory, 10)], None).unwrap());
    drop(scope);
    assert_eq!(pools.in_use(ResourceKind::Memory), 0);
    let p = pools.clone();
    let r = std::thread::spawn(move || {
        let _b = p.acquire(&[(ResourceKind::Memory, 10)], None).unwrap();
        panic!("x");
    })
    .join();
    assert!(r.is_err());
    assert_eq!(pools.in_use(ResourceKind::Memory), 0);
}

/// - admission behavior is deterministic and observable
#[test]
fn dod_14_admission_is_deterministic_and_observable() {
    fn run() -> (Vec<SubmitOutcome>, u64, u64) {
        let m = Arc::new(ResourceMetrics::new());
        let s: Scheduler<u32> = Scheduler::new(
            QueueCapacities {
                critical: 2,
                control: 2,
                interactive: 2,
                normal: 2,
                bulk: 2,
                background: 2,
            },
            AgingPolicy::default(),
            ResourcePools::new(&[]),
            Arc::clone(&m),
        );
        let mut out = Vec::new();
        for i in 0..20u32 {
            let p = [Critical, Normal, Bulk, Interactive][(i % 4) as usize];
            let trust = [
                TrustClass::Unknown,
                TrustClass::Known,
                TrustClass::LocalOwnDevice,
            ][(i % 3) as usize];
            if i == 10 {
                s.set_mode(OverloadMode::Overload);
            }
            out.push(s.submit(
                ShardKind::Network,
                Job {
                    payload: i,
                    priority: p,
                    durable: i % 2 == 0,
                    demand: vec![],
                },
                trust,
                u64::from(i),
            ));
        }
        let l = Labels {
            trust: TrustClass::Known,
            priority: Normal,
            kind: ResourceKind::Network,
        };
        (
            out,
            m.counters(l).shed + m.counters(l).deferred + m.counters(l).admission_rejects,
            0,
        )
    }
    assert_eq!(run(), run(), "same inputs, same decisions, same counters");
    assert!(
        run().1 > 0 || run().0.iter().any(|o| *o != SubmitOutcome::Queued),
        "and refusals are visible in the counters/outcomes"
    );
}

/// - unknown-peer floods remain bounded
#[test]
fn dod_15_unknown_peer_floods_remain_bounded() {
    let vb = VerificationBudget::new(VerifyLimits::default(), 0);
    let ok = (0..100_000u32)
        .filter_map(|i| vb.submit(peer(i), TrustClass::Unknown, 1, 0).ok())
        .map(drop)
        .count();
    assert!(ok <= 50);
    let adm = ConnectionAdmission::new(ConnectionAdmissionLimits::default(), 0);
    let held: Vec<_> = (0..100_000u32)
        .filter_map(|i| adm.try_admit(peer(i), SourceClass::Unknown, 0).ok())
        .collect();
    assert!(held.len() <= 8);
    assert_eq!(vb.tracked_peers(), 0);
}

/// - process restart can reconcile persistent reservations
#[test]
fn dod_16_restart_reconciles_persistent_reservations() {
    let recs = vec![
        DurableReservation {
            id: DurableResId(1),
            op: Some(OpId(1)),
            bytes: 100,
            owner: ResourceOwner::Files,
            expires_at_millis: 9_000,
        },
        DurableReservation {
            id: DurableResId(2),
            op: Some(OpId(99)),
            bytes: 100,
            owner: ResourceOwner::Files,
            expires_at_millis: 9_000,
        },
    ];
    let input = RecoveryInput {
        reservations: recs.clone(),
        staging_files: vec![OpId(1), OpId(77)],
        work_queue: vec![WorkEntry {
            op: OpId(1),
            needs_storage: true,
        }],
        active_ops: vec![OpId(1)],
    };
    let plan = plan_recovery(&input, 0);
    assert_eq!(
        (
            plan.restore.clone(),
            plan.release_orphans.clone(),
            plan.delete_orphan_staging.clone()
        ),
        (vec![DurableResId(1)], vec![DurableResId(2)], vec![OpId(77)])
    );
    let mut store = StorageReservations::new(1_000, CriticalStorageReserve { reserved_bytes: 0 });
    assert_eq!(
        restore_reservations(&mut store, &recs, &plan, 0)
            .restored
            .len(),
        1
    );
    assert_eq!(store.reserved_bytes(), 100);
}

/// - resource metrics and diagnostics are available
#[test]
fn dod_17_metrics_and_diagnostics_are_available() {
    let m = ResourceMetrics::new();
    m.record_reject(Labels {
        trust: TrustClass::Unknown,
        priority: Bulk,
        kind: ResourceKind::Memory,
    });
    assert_eq!(
        m.counters(Labels {
            trust: TrustClass::Unknown,
            priority: Bulk,
            kind: ResourceKind::Memory
        })
        .admission_rejects,
        1
    );
    const MIB: u64 = 1 << 20;
    let snap = ResourceSnapshot {
        memory: ResourceUsage::new(72 * MIB, 128 * MIB, 90 * MIB),
        streams: CountUsage {
            used: 14,
            limit: 32,
        },
        pressure: PressureState::Elevated,
        ..Default::default()
    };
    let lines = developer_lines(&snap, &[]);
    assert_eq!(lines[0], "Memory: 72/128 MiB");
    assert_eq!(hot_pools(&[NamedUsage::count("x", 1, 2)]), vec!["x 50%"]);
}

/// - property/concurrency/flood/storage-full tests exist
#[test]
fn dod_18_the_required_test_kinds_exist() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let read = |f: &str| {
        std::fs::read_to_string(dir.join(f)).unwrap_or_else(|_| panic!("missing tests/{f}"))
    };
    assert!(
        read("properties.rs").contains("proptest!"),
        "§173 property tests"
    );
    assert!(
        read("concurrency.rs").contains("cannot_deadlock"),
        "§174 concurrency tests"
    );
    let sc = read("scenarios.rs");
    for name in [
        "s175_", "s176_", "s177_", "s178_", "s179_", "s180_", "s181_", "s182_", "s183_",
    ] {
        assert!(sc.contains(&format!("fn {name}")), "scenario {name}");
    }
    assert!(
        read("fuzzing.rs").contains("proptest!"),
        "§184 fuzz targets"
    );
    let bench = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("benches")
        .join("resource_bench.rs");
    assert!(bench.exists(), "§185 benchmarks");
}
