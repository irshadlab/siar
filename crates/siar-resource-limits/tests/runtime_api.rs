//! §187-188: the facade, driven the way feature code drives it.

use std::sync::Arc;
use std::thread;

use siar_protocol_ext::TrafficPriority::*;
use siar_resource_limits::admission::{BandwidthClass, ResourceOwner, ResourceRequest};
use siar_resource_limits::error::{ResourceError, RetryClass};
use siar_resource_limits::peer_quota::TrustClass;
use siar_resource_limits::pressure::{OverloadMode, PressureSignals};
use siar_resource_limits::profile::ResourceProfile;
use siar_resource_limits::runtime::{ResourceRuntime, RuntimeError};
use siar_resource_limits::storage::PressureState;
use siar_resource_limits::types::ResourceKind;

const MIB: u64 = 1 << 20;

fn peer(i: u32) -> [u8; 32] {
    let mut k = [0u8; 32];
    k[..4].copy_from_slice(&i.to_le_bytes());
    k
}

fn rt() -> ResourceRuntime {
    ResourceRuntime::new(ResourceProfile::MobileNormal).unwrap()
}

#[test]
fn every_profile_builds_a_validated_runtime() {
    for p in [
        ResourceProfile::Embedded,
        ResourceProfile::MobileLow,
        ResourceProfile::MobileNormal,
        ResourceProfile::Desktop,
        ResourceProfile::Server,
    ] {
        assert!(ResourceRuntime::new(p).is_ok(), "{p:?}");
    }
}

#[test]
fn an_inconsistent_config_cannot_become_a_runtime() {
    use siar_resource_limits::feature_limits::FeatureLimits;
    use siar_resource_limits::policy_layers::HardLimits;
    use siar_resource_limits::profile::ResourceConfig;
    use siar_resource_limits::runtime::RuntimeOptions;
    let p = ResourceProfile::Desktop;
    let mut cfg = ResourceConfig::for_profile(p);
    cfg.storage.critical_reserve_bytes = cfg.storage.quota_bytes + 1;
    let r = ResourceRuntime::with_config(
        cfg,
        HardLimits::for_profile(p),
        FeatureLimits::for_profile(p),
        RuntimeOptions::default(),
    );
    assert!(matches!(r, Err(RuntimeError::Config(_))));
}

fn req(memory: u64, storage: u64, streams: u32, durable: bool) -> ResourceRequest {
    ResourceRequest {
        owner: ResourceOwner::Files,
        priority: Normal,
        memory,
        storage,
        streams,
        connections: 0,
        bandwidth_class: BandwidthClass::Unmetered,
        durable,
    }
}

#[test]
fn generic_admit_grants_releases_and_classifies_refusals() {
    let rt = rt();
    let cfg = *rt.config();
    let a = rt.admit(&req(8 * MIB, 10 * MIB, 2, true), 0).unwrap();
    assert_eq!(rt.snapshot().streams.used, 2);
    assert!(rt.snapshot().memory.used >= 8 * MIB);
    drop(a);
    assert_eq!(
        (rt.snapshot().streams.used, rt.snapshot().memory.used),
        (0, 0)
    );

    // beyond the device's absolute limits: permanent for this request
    let e = rt
        .admit(&req(cfg.memory.global_bytes + 1, 0, 0, true), 0)
        .unwrap_err();
    assert_eq!(
        (e, e.retry_class()),
        (
            ResourceError::HardLimitExceeded,
            RetryClass::PermanentForRequest
        )
    );
    // streams exhausted: retryable, typed
    let _hold = rt
        .admit(&req(0, 0, cfg.network.max_streams, true), 0)
        .unwrap();
    let e = rt.admit(&req(0, 0, 1, true), 0).unwrap_err();
    assert_eq!(
        (e, e.retry_class()),
        (ResourceError::TooManyStreams, RetryClass::Retryable)
    );
    assert_eq!(rt.snapshot().streams.used, cfg.network.max_streams);
}

#[test]
fn a_failed_late_step_unwinds_the_early_ones() {
    let rt = rt();
    let cfg = *rt.config();
    // Ask for storage more than currently available but within hard limits.
    let big = cfg.storage.quota_bytes - cfg.storage.critical_reserve_bytes;
    let _fill = rt.admit(&req(0, big, 0, true), 0).unwrap();
    let before = rt.snapshot();
    let e = rt.admit(&req(4 * MIB, 1, 3, false), 1).unwrap_err();
    assert_eq!(
        e,
        ResourceError::StorageFull,
        "non-durable work is not parked"
    );
    let after = rt.snapshot();
    assert_eq!(
        (before.memory.used, before.streams.used),
        (after.memory.used, after.streams.used),
        "memory and streams taken before the failure were returned"
    );
    let e = rt.admit(&req(4 * MIB, 1, 3, true), 1).unwrap_err();
    assert_eq!(e, ResourceError::Deferred, "durable work is deferred");
}

#[test]
fn file_transfers_one_hundred_offers_four_run_the_rest_defer_and_all_resources_return() {
    let rt = rt();
    let files = rt.files();
    let mut active = Vec::new();
    let mut deferred = 0;
    for i in 0..100u32 {
        match files.acquire_transfer(peer(i), TrustClass::Verified, 2 * 1024 * MIB, MIB, false, 0) {
            Ok(p) => active.push(p),
            Err(ResourceError::Deferred) => deferred += 1,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    assert_eq!(active.len() as u32 + deferred, 100);
    assert!(
        active.len() <= rt.config().network.max_transfers as usize,
        "{} active",
        active.len()
    );
    assert!(!active.is_empty() && deferred > 90);
    assert_eq!(files.active_transfers() as usize, active.len());
    assert_eq!(
        active[0].manifest_entries(),
        2 * 1024,
        "2 GiB / 1 MiB chunks"
    );
    // each held transfer pins its chunk buffers and staging
    assert!(rt.snapshot().memory.used >= active.len() as u64 * 8 * MIB);
    drop(active);
    let s = rt.snapshot();
    assert_eq!((s.memory.used, s.streams.used), (0, 0));
    assert_eq!(files.active_transfers(), 0);
    assert!(
        files
            .acquire_transfer(peer(1), TrustClass::Verified, MIB, MIB, false, 0)
            .is_ok(),
        "capacity is back"
    );
}

#[test]
fn offers_are_validated_from_their_declared_numbers() {
    let rt = rt();
    let f = rt.files();
    let e = f
        .acquire_transfer(peer(1), TrustClass::Known, u64::MAX, 1, false, 0)
        .unwrap_err();
    assert_eq!(e, ResourceError::HardLimitExceeded);
    let e = f
        .acquire_transfer(peer(1), TrustClass::Known, 4 << 30, 1, false, 0)
        .unwrap_err();
    assert_eq!(
        e,
        ResourceError::HardLimitExceeded,
        "tiny chunks would need a huge manifest"
    );
    let e = f
        .acquire_transfer(peer(1), TrustClass::Known, 10, 0, false, 0)
        .unwrap_err();
    assert!(matches!(e, ResourceError::MalformedRequest(_)));
    assert_eq!(f.active_transfers(), 0, "rejected offers hold nothing");
}

#[test]
fn an_unknown_peer_cannot_stage_more_than_its_quota_and_others_are_unaffected() {
    let rt = rt();
    let f = rt.files();
    let attacker = peer(666);
    let a = f.acquire_transfer(attacker, TrustClass::Unknown, MIB, MIB, false, 0);
    let b = f.acquire_transfer(attacker, TrustClass::Unknown, MIB, MIB, false, 0);
    let c = f.acquire_transfer(attacker, TrustClass::Unknown, MIB, MIB, false, 0);
    assert!(
        a.is_ok() && b.is_ok(),
        "within the Unknown staged-file quota"
    );
    assert_eq!(
        c.unwrap_err(),
        ResourceError::Deferred,
        "third is over quota"
    );
    assert!(f
        .acquire_transfer(peer(1), TrustClass::Verified, MIB, MIB, false, 0)
        .is_ok());
}

#[test]
fn messaging_outbox_bounds_and_a_critical_reserve() {
    let rt = rt();
    let m = rt.messaging();
    let max = rt.features().messaging;
    assert_eq!(
        m.acquire_message(
            TrustClass::LocalOwnDevice,
            max.max_message_bytes + 1,
            0,
            Normal,
            0
        )
        .unwrap_err(),
        ResourceError::HardLimitExceeded
    );
    let mut held = Vec::new();
    loop {
        match m.acquire_message(TrustClass::LocalOwnDevice, 100, 0, Normal, 0) {
            Ok(p) => held.push(p),
            Err(e) => {
                assert_eq!(e, ResourceError::Busy);
                break;
            }
        }
    }
    assert!(held.len() as u64 <= max.max_pending_outbound);
    let sos = m.acquire_message(TrustClass::LocalOwnDevice, 100, 0, Critical, 0);
    assert!(
        sos.is_ok(),
        "SOS still fits when normal traffic filled the outbox"
    );
    drop(held.pop());
    assert!(
        m.acquire_message(TrustClass::LocalOwnDevice, 100, 0, Normal, 0)
            .is_ok(),
        "delivery freed a slot"
    );
}

#[test]
fn inbound_messages_are_rate_limited_per_peer_and_by_trust() {
    let rt = rt();
    let m = rt.messaging();
    let p = peer(5);
    let mut ok = Vec::new();
    let mut limited = 0;
    for _ in 0..50 {
        match m.admit_inbound(p, TrustClass::Unknown, 100, 0) {
            Ok(g) => ok.push(g),
            Err(ResourceError::RateLimited | ResourceError::Busy) => limited += 1,
            Err(e) => panic!("{e:?}"),
        }
    }
    assert!(
        ok.len() <= 8 && limited > 40,
        "unknown peers are tightly limited, got {}",
        ok.len()
    );
    assert_eq!(
        m.admit_inbound(p, TrustClass::Unknown, 10 << 20, 0)
            .unwrap_err(),
        ResourceError::HardLimitExceeded
    );
}

#[test]
fn dtn_bundles_respect_peer_quota_and_release_on_drop() {
    let rt = rt();
    let d = rt.dtn();
    assert_eq!(
        d.reserve_bundle(peer(1), TrustClass::Known, 10 * MIB, 1, 1, Normal)
            .unwrap_err(),
        ResourceError::HardLimitExceeded,
        "bigger than any bundle"
    );
    assert_eq!(
        d.reserve_bundle(peer(1), TrustClass::Known, 100, 99, 1, Normal)
            .unwrap_err(),
        ResourceError::HardLimitExceeded,
        "replication budget"
    );
    // An Unknown peer's relay quota is small; push bundles until refused.
    let mut held = Vec::new();
    let err = loop {
        match d.reserve_bundle(peer(9), TrustClass::Unknown, 256 * 1024, 2, 4, Normal) {
            Ok(b) => held.push(b),
            Err(e) => break e,
        }
    };
    assert!(
        matches!(
            err,
            ResourceError::QuotaExceeded(_) | ResourceError::Deferred
        ),
        "{err:?}"
    );
    assert!(!held.is_empty() && held.len() < 20);
    // another peer is not affected
    assert!(d
        .reserve_bundle(peer(10), TrustClass::Known, 256 * 1024, 2, 4, Normal)
        .is_ok());
    drop(held);
    assert!(
        d.reserve_bundle(peer(9), TrustClass::Unknown, 256 * 1024, 2, 4, Normal)
            .is_ok(),
        "quota returned"
    );
}

#[test]
fn platform_memory_pressure_switches_the_whole_runtime_to_shedding() {
    let rt = rt();
    assert!(rt
        .files()
        .acquire_transfer(peer(1), TrustClass::Verified, MIB, MIB, false, 0)
        .is_ok());
    rt.report_android_trim(80, 0);
    let mode = rt.evaluate_pressure(PressureSignals::default(), 1);
    assert_eq!(mode, OverloadMode::CriticalOnly);
    // bulk is deferred (durable) ...
    assert_eq!(
        rt.files()
            .acquire_transfer(peer(2), TrustClass::Verified, MIB, MIB, false, 2)
            .unwrap_err(),
        ResourceError::Deferred
    );
    // ... ephemeral generic work is shed ...
    assert_eq!(
        rt.admit(&req(1, 0, 0, false), 2).unwrap_err(),
        ResourceError::Overloaded(OverloadMode::CriticalOnly)
    );
    // ... and an SOS still goes through.
    assert!(rt
        .messaging()
        .acquire_message(TrustClass::LocalOwnDevice, 100, 0, Critical, 2)
        .is_ok());
    assert_eq!(rt.snapshot().pressure, PressureState::Exhausted);
    // after the hold lapses and a calm window passes, service resumes
    let mut t = 100_000;
    for _ in 0..4 {
        rt.evaluate_pressure(PressureSignals::default(), t);
        t += 10_000;
    }
    assert_eq!(rt.mode(), OverloadMode::Normal);
    assert!(rt
        .files()
        .acquire_transfer(peer(3), TrustClass::Verified, MIB, MIB, false, t)
        .is_ok());
}

#[test]
fn refusals_are_counted_in_metrics() {
    use siar_resource_limits::metrics::Labels;
    let rt = rt();
    let _ = rt
        .files()
        .acquire_transfer(peer(1), TrustClass::Known, u64::MAX, 1, false, 0);
    let l = Labels {
        trust: TrustClass::Known,
        priority: Bulk,
        kind: ResourceKind::Storage,
    };
    assert_eq!(rt.metrics().counters(l).admission_rejects, 1);
}

#[test]
fn concurrent_feature_traffic_conserves_everything() {
    let rt = Arc::new(rt());
    let hs: Vec<_> = (0..8u32)
        .map(|t| {
            let rt = Arc::clone(&rt);
            thread::spawn(move || {
                for i in 0..300u32 {
                    let p = peer(t * 1000 + i % 7);
                    let _x = rt
                        .files()
                        .acquire_transfer(
                            p,
                            TrustClass::Verified,
                            8 * MIB,
                            MIB,
                            i % 5 == 0,
                            u64::from(i),
                        )
                        .ok();
                    let _m = rt
                        .messaging()
                        .acquire_message(TrustClass::Known, 200, 0, Normal, u64::from(i))
                        .ok();
                    let _d = rt
                        .dtn()
                        .reserve_bundle(p, TrustClass::Known, 4096, 2, 4, Normal)
                        .ok();
                    let _g = rt.admit(&req(MIB, MIB, 1, true), u64::from(i)).ok();
                    let _i = rt
                        .messaging()
                        .admit_inbound(p, TrustClass::Known, 100, u64::from(i))
                        .ok();
                }
            })
        })
        .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
    let s = rt.snapshot();
    assert_eq!(
        (s.memory.used, s.streams.used, s.connections.used),
        (0, 0, 0)
    );
    assert_eq!(s.storage.reserved, 0, "every staging lease was cancelled");
    assert_eq!(s.storage.used, 0, "relay ledger back to zero");
    assert_eq!(rt.files().active_transfers(), 0);
}
