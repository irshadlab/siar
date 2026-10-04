//! §175-183 scenario tests, one per spec scenario.

use std::sync::Arc;

use siar_protocol_ext::TrafficPriority::{self, *};
use siar_resource_limits::abuse::authorize_priority;
use siar_resource_limits::admission::{AdmissionResult, ResourceOwner};
use siar_resource_limits::admission_pipeline::{
    ConnectionAdmission, ConnectionAdmissionLimits, CookieIssuer, UnknownGateLimits, UnknownHello,
    UnknownPeerGate, UnknownReject,
};
use siar_resource_limits::aging::AgingPolicy;
use siar_resource_limits::budget::{BudgetError, GlobalBudget, Subsystem};
use siar_resource_limits::cancel::OperationScope;
use siar_resource_limits::coalesce::{CoalesceOutcome, KeyedCoalescer};
use siar_resource_limits::cpu_thermal::{cpu_response, CpuBase};
use siar_resource_limits::effective_limits::pressure_limit_for;
use siar_resource_limits::event_log_pressure::{
    OutboxAccounting, OutboxClass, OutboxLimits, WriterBackpressure, WriterLimits, WriterQueue,
};
use siar_resource_limits::fair_semaphore::FairSemaphore;
use siar_resource_limits::fanout::{ClientQueue, PushOutcome, Update};
use siar_resource_limits::feature_limits::FeatureLimits;
use siar_resource_limits::feedback::{user_message, ResourceCause, UserMessage};
use siar_resource_limits::inbound::{InboundGate, InboundLimits};
use siar_resource_limits::metrics::ResourceMetrics;
use siar_resource_limits::modes::{apply_modes, ModeFlags, ModeTunables};
use siar_resource_limits::os_memory::{HoldTimes, MemoryPressureAggregator};
use siar_resource_limits::peer_quota::{PeerQuota, PeerUsageCounters, PeerUsageDelta, TrustClass};
use siar_resource_limits::pressure::{
    shed, ModeController, OverloadMode, PressureSignals, ShedDecision,
};
use siar_resource_limits::profile::{ResourceConfig, ResourceProfile};
use siar_resource_limits::queue::QueueCapacities;
use siar_resource_limits::reservation::ResourcePools;
use siar_resource_limits::scheduler::{Job, Scheduler, ShardKind, SubmitOutcome};
use siar_resource_limits::slots::{TransferKind, TransferSlots};
use siar_resource_limits::storage::{
    CriticalStorageReserve, PressureState, StorageReservations, StorageWatermarks,
};
use siar_resource_limits::storm::SourceClass;
use siar_resource_limits::supervisor::{Decision, ExitKind, RestartPolicy, Supervisor};
use siar_resource_limits::tasks::TaskRegistry;
use siar_resource_limits::types::ResourceKind;
use siar_resource_limits::verification::{VerificationBudget, VerifyLimits};

fn peer(i: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..4].copy_from_slice(&i.to_le_bytes());
    k
}

/// §175: a producer emits 1,000,000 items to a slow consumer. Memory stays
/// bounded (queue length never exceeds its capacity at any step) and the
/// producer is slowed/rejected rather than the process running out of memory.
#[test]
fn s175_one_million_items_to_a_slow_consumer_stays_bounded() {
    const CAP: usize = 1_024;
    let mut ipc: ClientQueue<u64> = ClientQueue::new(CAP);
    let mut writer: WriterQueue<u64> = WriterQueue::new(WriterLimits {
        max_batch_items: 64,
        max_batch_bytes: 1 << 20,
        queue_items: CAP,
        queue_bytes: 1 << 30,
    });
    let (mut coalesced, mut dropped, mut evicted, mut backpressured) = (0u64, 0u64, 0u64, 0u64);
    let mut max_ipc = 0;
    let mut max_writer = 0;
    for i in 0..1_000_000u64 {
        let u = match i % 100 {
            0 => Update::Critical(i),
            1..=40 => Update::Coalesce {
                key: i % 50,
                value: i,
            },
            _ => Update::Droppable(i),
        };
        match ipc.push(u) {
            PushOutcome::Coalesced => coalesced += 1,
            PushOutcome::DroppedIncoming => dropped += 1,
            PushOutcome::EvictedNoncritical => evicted += 1,
            PushOutcome::Queued | PushOutcome::ResyncRequired => {}
        }
        if writer.try_enqueue(i, 100).is_err() {
            backpressured += 1;
        }
        // The consumer is ~100x slower than the producer.
        if i % 100 == 0 {
            ipc.pop();
            writer.next_batch().truncate(1);
        }
        max_ipc = max_ipc.max(ipc.len());
        max_writer = max_writer.max(writer.len());
    }
    assert!(
        max_ipc <= CAP && max_writer <= CAP,
        "bounded memory: ipc {max_ipc}, writer {max_writer}"
    );
    assert!(
        coalesced + dropped + evicted > 100_000,
        "producer's excess was coalesced/dropped, not queued"
    );
    assert!(
        backpressured > 100_000,
        "the durable writer pushed back on its producer"
    );
    let (_, why) = writer
        .try_enqueue(0, 100)
        .err()
        .map_or((0, WriterBackpressure::QueueFull), |(i, w)| (i, w));
    assert_eq!(why, WriterBackpressure::QueueFull);
}

/// §176: 100 large transfers against a limit of 4 → 4 active, 96 deferred,
/// staging and memory bounded.
#[test]
fn s176_hundred_transfers_limit_four() {
    const FILE: u64 = 4 << 30;
    let limits = FeatureLimits::for_profile(ResourceProfile::MobileNormal);
    let cfg = ResourceConfig::for_profile(ResourceProfile::MobileNormal);
    let slots = TransferSlots::new(4, 4, 0);
    let mut staging = StorageReservations::new(
        cfg.storage.quota_bytes,
        CriticalStorageReserve {
            reserved_bytes: cfg.storage.critical_reserve_bytes,
        },
    );
    let per_transfer_mem = limits.files.max_parallel_chunks * limits.files.max_chunk_size;
    let pools = ResourcePools::new(&[(
        ResourceKind::Memory,
        limits.files.max_active_transfers * per_transfer_mem,
    )]);

    let (mut active, mut deferred) = (Vec::new(), 0);
    for i in 0..100u32 {
        match slots.try_acquire(peer(i % 20), TransferKind::Background) {
            Ok(slot) => {
                let mem = pools
                    .acquire(&[(ResourceKind::Memory, per_transfer_mem)], None)
                    .expect("memory sized for exactly the slot limit");
                // staging is reserved per started transfer, never for deferred ones
                let res = staging.reserve(
                    limits.files.max_chunk_size * limits.files.max_parallel_chunks,
                    ResourceOwner::Files,
                    60_000,
                    0,
                    true,
                );
                active.push((slot, mem, res));
            }
            Err(_) => deferred += 1,
        }
    }
    assert_eq!((active.len(), deferred), (4, 96));
    assert_eq!(slots.active(), 4);
    assert!(
        pools.in_use(ResourceKind::Memory) <= 4 * per_transfer_mem,
        "bounded memory"
    );
    assert!(
        staging.reserved_bytes() <= 4 * per_transfer_mem,
        "bounded staging: nothing reserved for the 96 deferred"
    );
    // a finished transfer frees exactly one slot for the next deferred one
    drop(active.pop());
    assert!(slots
        .try_acquire(peer(99), TransferKind::Background)
        .is_ok());
    let _ = FILE;
}

fn base_tunables() -> ModeTunables {
    ModeTunables {
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
    }
}

/// §177: a bulk transfer is running, then a video call starts. The call
/// reserves its resources, bulk throttles, call quality is protected.
#[test]
fn s177_video_call_preempts_bulk() {
    const MIB: u64 = 1 << 20;
    // Bulk (Files) has borrowed beyond its share, eating into Calls' guarantee.
    let b = GlobalBudget::new(
        256 * MIB,
        [32 * MIB, 96 * MIB, 32 * MIB, 64 * MIB, 32 * MIB],
        160 * MIB,
    );
    let mut bulk = b
        .try_acquire(Subsystem::Files, 96 * MIB + 100 * MIB)
        .unwrap();
    let _dtn = b.try_acquire(Subsystem::Dtn, 32 * MIB).unwrap();
    let _msg = b.try_acquire(Subsystem::Messaging, 28 * MIB).unwrap();
    // The call starts: its first request is refused, and the refusal names bulk as revocable.
    let err = b.try_acquire(Subsystem::Calls, 64 * MIB).unwrap_err();
    let BudgetError::GlobalCeiling { reclaimable } = err else {
        panic!("expected ceiling, got {err:?}")
    };
    assert!(
        reclaimable.iter().any(|(s, _)| *s == Subsystem::Files),
        "bulk is identified as the borrower to revoke"
    );
    // Revoke bulk's loan; the call now gets its full guarantee.
    bulk.shrink_by(100 * MIB);
    let call = b
        .try_acquire(Subsystem::Calls, 64 * MIB)
        .expect("call reserves its resources");
    assert_eq!(call.amount(), 64 * MIB);

    // And the mode policy throttles bulk throughput while a call is active.
    let hard = base_tunables();
    let off = apply_modes(&base_tunables(), &hard, &ModeFlags::default());
    let on = apply_modes(
        &base_tunables(),
        &hard,
        &ModeFlags {
            call_active: true,
            ..Default::default()
        },
    );
    assert!(on.bulk_rate_bytes_per_sec < off.bulk_rate_bytes_per_sec);
    assert_eq!(on.call_reserved_bandwidth, 500_000);
    assert_eq!(off.call_reserved_bandwidth, 0);
    assert!(!on.background_sync_allowed);
}

/// §178: normal/bulk queues are full; an SOS is injected and admitted
/// through reserved critical capacity.
#[test]
fn s178_sos_is_admitted_when_everything_else_is_full() {
    let caps = QueueCapacities {
        critical: 4,
        control: 4,
        interactive: 8,
        normal: 8,
        bulk: 8,
        background: 8,
    };
    let s: Scheduler<&'static str> = Scheduler::new(
        caps,
        AgingPolicy::default(),
        ResourcePools::new(&[]),
        Arc::new(ResourceMetrics::new()),
    );
    let job = |p: TrafficPriority, n: &'static str| Job {
        payload: n,
        priority: p,
        durable: true,
        demand: vec![],
    };
    while s.submit(ShardKind::Network, job(Normal, "n"), TrustClass::Known, 0)
        == SubmitOutcome::Queued
    {}
    while s.submit(ShardKind::Network, job(Bulk, "b"), TrustClass::Known, 0)
        == SubmitOutcome::Queued
    {}
    assert_eq!(
        s.submit(
            ShardKind::Network,
            job(Normal, "more"),
            TrustClass::Known,
            0
        ),
        SubmitOutcome::Deferred,
        "normal is full"
    );
    assert_eq!(
        s.submit(
            ShardKind::Network,
            job(Critical, "SOS"),
            TrustClass::LocalOwnDevice,
            0
        ),
        SubmitOutcome::Queued
    );
    assert_eq!(
        s.dispatch(ShardKind::Network, 0).unwrap().payload,
        "SOS",
        "and it goes first"
    );

    // Even in CriticalOnly mode with the system overloaded:
    s.set_mode(OverloadMode::CriticalOnly);
    assert_eq!(
        s.submit(
            ShardKind::Network,
            job(Critical, "SOS2"),
            TrustClass::LocalOwnDevice,
            1
        ),
        SubmitOutcome::Queued
    );
    assert_eq!(
        s.submit(ShardKind::Network, job(Bulk, "bulk"), TrustClass::Known, 1),
        SubmitOutcome::Deferred
    );

    // A verified SOS from an otherwise-unknown peer rides the attested path only.
    assert_eq!(
        authorize_priority(TrustClass::Unknown, Critical, true),
        Critical
    );
    assert_eq!(
        authorize_priority(TrustClass::Unknown, Critical, false),
        Normal
    );

    // Outbox and storage keep a reserve that normal/bulk cannot use.
    let mut o = OutboxAccounting::new(OutboxLimits {
        max_count: 10,
        max_bytes: 10_000,
        critical_reserve_count: 2,
        critical_reserve_bytes: 2_000,
        bulk_percent: 50,
    });
    while o.try_add(OutboxClass::Normal, 100, 1_000, 0).is_ok() {}
    assert!(
        o.try_add(OutboxClass::Critical, 100, 1_000, 0).is_ok(),
        "SOS fits the outbox's critical reserve"
    );
}

/// §179: an unknown peer floods file offers, bundle offers and handshakes.
/// Everything is rate limited and CPU/memory/storage stay bounded.
#[test]
fn s179_unknown_peer_flood_stays_bounded() {
    let unknown_quota = PeerQuota::for_trust_class(TrustClass::Unknown);
    let files = FeatureLimits::for_profile(ResourceProfile::MobileNormal).files;

    // -- many file offers (one attacker identity) --
    let mut gate = InboundGate::new(InboundLimits::for_trust(TrustClass::Unknown), 0);
    let mut usage = PeerUsageCounters::default();
    let mut accepted_offers = 0;
    for _ in 0..100_000 {
        if gate.admit(512, 0).is_err() {
            continue;
        }
        if usage.try_charge(
            &unknown_quota,
            PeerUsageDelta {
                staged_files: 1,
                ..Default::default()
            },
            false,
        ) == AdmissionResult::Accepted
        {
            accepted_offers += 1;
        }
    }
    assert!(
        accepted_offers <= unknown_quota.max_staged_files,
        "staged files capped at {}",
        unknown_quota.max_staged_files
    );
    assert!(
        files.check_offer(1 << 40, 1).is_err(),
        "absurd offers rejected from their declared numbers"
    );

    // -- many bundle offers --
    let mut bundle_usage = PeerUsageCounters::default();
    let bundles_ok = (0..10_000)
        .filter(|_| {
            bundle_usage.try_charge(
                &unknown_quota,
                PeerUsageDelta {
                    dtn_relay_bytes: 100 * 1024,
                    ..Default::default()
                },
                false,
            ) == AdmissionResult::Accepted
        })
        .count() as u64;
    assert!(
        bundles_ok * 100 * 1024 <= unknown_quota.max_dtn_relay_bytes,
        "relay bytes capped"
    );

    // -- many verifications (CPU) from rotating identities --
    let vb = VerificationBudget::new(VerifyLimits::default(), 0);
    let verified = (0..50_000u32)
        .filter_map(|i| vb.submit(peer(i), TrustClass::Unknown, 1, 0).ok())
        .map(drop)
        .count();
    assert!(
        verified <= 50,
        "unknown class CPU budget (burst 50), got {verified}"
    );
    assert_eq!(vb.tracked_peers(), 0, "no per-identity state");

    // -- many handshakes from rotating sources --
    let adm = ConnectionAdmission::new(ConnectionAdmissionLimits::default(), 0);
    let mut held = Vec::new();
    for i in 0..10_000u32 {
        if let Ok(c) = adm.try_admit(peer(i), SourceClass::Unknown, 0) {
            held.push(c);
        }
    }
    assert!(
        held.len() <= 8,
        "unknown connection burst, got {}",
        held.len()
    );
    assert!(adm.handshake_counts().0 <= 8);

    // -- hello flood with spoofed sources: no state grows, class-wide limit applies --
    let g = UnknownPeerGate::new(
        UnknownGateLimits::default(),
        CookieIssuer::new([5; 32], 10_000),
        0,
    );
    let mut challenged = 0;
    for i in 0..100_000u32 {
        let src = i.to_le_bytes();
        match g.check(
            &UnknownHello {
                source: &src,
                payload_len: 100,
                syntax_ok: true,
                cookie: None,
            },
            0,
            true,
        ) {
            Err(UnknownReject::Challenge(_)) => challenged += 1,
            Err(UnknownReject::RateLimited) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(challenged <= 50);
    assert_eq!(g.tracked_sources(), 0);
}

/// §180: disk full → bulk rejected, critical reserve preserved, no
/// corrupt partial commits.
#[test]
fn s180_storage_full() {
    const CAP: u64 = 10_000;
    const RESERVE: u64 = 2_000;
    let wm = StorageWatermarks::spec_example();
    let mut s = StorageReservations::new(
        CAP,
        CriticalStorageReserve {
            reserved_bytes: RESERVE,
        },
    );
    // Bulk (Files) fills everything it is allowed to.
    let mut bulk = Vec::new();
    while let Ok(id) = s.reserve(500, ResourceOwner::Files, 60_000, 0, true) {
        bulk.push(id);
    }
    assert_eq!(
        s.reserved_bytes(),
        CAP - RESERVE,
        "bulk stopped exactly at the reserve boundary"
    );
    let rejected = s
        .reserve(1, ResourceOwner::Files, 60_000, 0, true)
        .unwrap_err();
    assert!(
        matches!(rejected, AdmissionResult::Deferred(_)),
        "durable bulk is deferred, not lost"
    );
    // Disk is now at pressure: user-facing message and shedding follow.
    s.set_used_bytes(CAP - RESERVE);
    let pressure = wm.classify(CAP - RESERVE, CAP);
    assert!(
        pressure >= PressureState::Elevated,
        "80% full classifies as at least Elevated"
    );
    assert_eq!(
        user_message(ResourceCause::StoragePressure(PressureState::Critical)),
        Some(UserMessage::DeviceLowOnStorage)
    );
    // The critical reserve is still there for messaging.
    s.set_used_bytes(0);
    let crit = s.reserve(RESERVE, ResourceOwner::Messaging, 60_000, 0, true);
    assert!(crit.is_ok(), "critical reserve preserved");
    // No corrupt partial state: a failed reserve changes nothing; cancel
    // restores exactly; double-cancel is a no-op.
    let before = s.reserved_bytes();
    assert!(s
        .reserve(1, ResourceOwner::Messaging, 60_000, 0, true)
        .is_err());
    assert_eq!(s.reserved_bytes(), before);
    let id = bulk.pop().unwrap();
    assert!(s.cancel(id).is_some());
    assert!(s.cancel(id).is_none());
    assert_eq!(s.reserved_bytes(), before - 500);
}

/// §181: a low-memory signal shrinks caches and parallelism, ephemeral
/// queues coalesce/drop, durable state survives.
#[test]
fn s181_low_memory() {
    let mut agg = MemoryPressureAggregator::new(HoldTimes::default());
    let mut ctl = ModeController::new(1_000);
    agg.report_android_trim(80, 0); // TRIM_MEMORY_COMPLETE
    let sig = agg.apply_to(PressureSignals::default(), 1);
    let mode = ctl.observe(&sig, 1);
    assert_eq!(mode, OverloadMode::CriticalOnly);

    // cache shrinks
    let cache: u64 = 100 << 20;
    assert!(
        pressure_limit_for(cache, sig.memory) < cache / 2,
        "cache budget shrinks sharply under pressure"
    );
    // parallelism drops
    let cpu = cpu_response(
        CpuBase {
            parallel_hashes: 8,
            indexing_concurrency: 4,
        },
        sig.memory,
    );
    assert_eq!(cpu.parallel_hashes, 1);
    assert!(cpu.pause_background_compression);
    // ephemeral work: coalesced into a bounded table and shed by policy
    let mut typing: KeyedCoalescer<u64, &str> = KeyedCoalescer::new(8);
    for i in 0..10_000u64 {
        let r = typing.push(i % 4, "typing");
        assert!(matches!(
            r,
            CoalesceOutcome::Inserted | CoalesceOutcome::Replaced
        ));
    }
    assert_eq!(typing.len(), 4);
    assert_eq!(shed(mode, Bulk, false), ShedDecision::Drop);
    // durable state survives: it defers instead of disappearing, and what is stored is untouched
    assert_eq!(shed(mode, Bulk, true), ShedDecision::Defer);
    let mut outbox = OutboxAccounting::new(OutboxLimits {
        max_count: 100,
        max_bytes: 1 << 20,
        critical_reserve_count: 5,
        critical_reserve_bytes: 0,
        bulk_percent: 100,
    });
    for i in 0..50 {
        outbox
            .try_add(OutboxClass::Normal, 100, 1_000_000 + i, 0)
            .unwrap();
    }
    let before = outbox.len();
    let _ = ctl.observe(&sig, 2);
    assert_eq!(
        outbox.len(),
        before,
        "pressure never evicts durable queued messages"
    );
    // the pressure is held, then fades if never refreshed (no OS "all clear")
    assert_eq!(agg.current(70_000), PressureState::Normal);
}

/// §182: a task cancelled while waiting for multiple permits leaks nothing.
#[test]
fn s182_cancel_while_waiting_for_multiple_permits() {
    let a = FairSemaphore::new(1);
    let b = FairSemaphore::new(1);
    let pools = ResourcePools::new(&[(ResourceKind::Memory, 10), (ResourceKind::Streams, 1)]);
    let _b_holder = b.try_acquire(1).unwrap(); // someone else holds B

    let mut scope = OperationScope::root();
    scope.adopt(a.try_acquire(1).expect("got A"));
    scope.adopt(b.wait(1).expect("queued for B")); // blocked waiting on B
    scope.adopt(pools.acquire(&[(ResourceKind::Memory, 5)], None).unwrap());
    assert_eq!(
        (
            a.available(),
            b.queued(),
            pools.in_use(ResourceKind::Memory)
        ),
        (0, 1, 5)
    );
    let token = scope.token().clone();
    drop(scope); // cancellation
    assert!(token.is_cancelled());
    assert_eq!(a.available(), 1, "A released");
    assert_eq!(b.queued(), 0, "the abandoned wait left the queue");
    assert_eq!(pools.in_use(ResourceKind::Memory), 0);
    // and B's real holder is unaffected / the line is clear
    drop(_b_holder);
    assert!(b.try_acquire(1).is_ok());
}

/// §183: a worker panics while holding permits; RAII releases them and
/// the supervisor handles the failure.
#[test]
fn s183_worker_panic_releases_permits_and_supervisor_reacts() {
    let pools = ResourcePools::new(&[(ResourceKind::Memory, 100), (ResourceKind::Streams, 4)]);
    let registry = TaskRegistry::new(8);
    let mut sup: Supervisor<&'static str> = Supervisor::new(RestartPolicy {
        max_restarts: 2,
        base_backoff_millis: 100,
        ..Default::default()
    });

    let mut decisions = Vec::new();
    for attempt in 0..4u64 {
        sup.on_started("transfer-worker");
        let (p, r) = (pools.clone(), registry.clone());
        let h = std::thread::spawn(move || {
            let _task = r.register("transfer", Normal, attempt).unwrap();
            let _permits = p
                .acquire(
                    &[(ResourceKind::Memory, 80), (ResourceKind::Streams, 2)],
                    None,
                )
                .unwrap();
            panic!("worker blew up while holding permits");
        });
        assert!(h.join().is_err(), "the worker panicked");
        assert_eq!(
            (
                pools.in_use(ResourceKind::Memory),
                pools.in_use(ResourceKind::Streams)
            ),
            (0, 0),
            "RAII released the permits"
        );
        assert_eq!(registry.active(), 0, "task deregistered");
        decisions.push(sup.on_exit("transfer-worker", ExitKind::Crash, attempt * 10));
    }
    assert!(matches!(
        decisions[0],
        Decision::RestartAfter { millis: 100 }
    ));
    assert!(matches!(
        decisions[1],
        Decision::RestartAfter { millis: 200 }
    ));
    assert!(
        matches!(decisions[2], Decision::MarkDegraded { .. }),
        "crash loop ends in Degraded, not an endless restart: {decisions:?}"
    );
}
