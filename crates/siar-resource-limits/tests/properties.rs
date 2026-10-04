//! §173 property tests. Random operation sequences against the public
//! API; each property is one of the spec's five invariants.

use proptest::prelude::*;

use siar_resource_limits::admission::{AdmissionResult, ResourceOwner};
use siar_resource_limits::budget::{BudgetPermit, GlobalBudget, Subsystem};
use siar_resource_limits::peer_quota::{PeerQuota, PeerUsageCounters, PeerUsageDelta, TrustClass};
use siar_resource_limits::profile::{ResourceConfig, ResourceProfile};
use siar_resource_limits::reservation::ResourcePools;
use siar_resource_limits::storage::{CriticalStorageReserve, StorageReservations};
use siar_resource_limits::types::ResourceKind;

#[derive(Debug, Clone)]
enum BudgetOp {
    Acquire(usize, u64),
    Drop(usize),
    Shrink(usize, u64),
    ToggleBorrowing,
}

fn budget_ops() -> impl Strategy<Value = Vec<BudgetOp>> {
    prop::collection::vec(
        prop_oneof![
            (0usize..5, 0u64..400).prop_map(|(s, n)| BudgetOp::Acquire(s, n)),
            (0usize..64).prop_map(BudgetOp::Drop),
            (0usize..64, 0u64..400).prop_map(|(i, n)| BudgetOp::Shrink(i, n)),
            Just(BudgetOp::ToggleBorrowing),
        ],
        1..200,
    )
}

proptest! {
    /// "usage never exceeds hard budget" — and the books always balance:
    /// total_used equals the sum of live permits, per subsystem too.
    #[test]
    fn global_budget_never_exceeds_the_ceiling_and_books_balance(ops in budget_ops()) {
        const MAX: u64 = 1_000;
        let b = GlobalBudget::new(MAX, [200; 5], 400);
        let mut live: Vec<BudgetPermit> = Vec::new();
        let mut borrowing = true;
        for op in ops {
            match op {
                BudgetOp::Acquire(s, n) => {
                    if let Ok(p) = b.try_acquire(Subsystem::ALL[s], n) { live.push(p); }
                }
                BudgetOp::Drop(i) => if !live.is_empty() { drop(live.remove(i % live.len())); },
                BudgetOp::Shrink(i, n) => if !live.is_empty() { let k = i % live.len(); live[k].shrink_by(n); },
                BudgetOp::ToggleBorrowing => { borrowing = !borrowing; b.set_borrowing_enabled(borrowing); }
            }
            prop_assert!(b.total_used() <= MAX);
            prop_assert_eq!(b.total_used(), live.iter().map(BudgetPermit::amount).sum::<u64>());
            for s in Subsystem::ALL {
                prop_assert_eq!(b.used(s), live.iter().filter(|p| p.subsystem() == s).map(BudgetPermit::amount).sum::<u64>());
            }
        }
        drop(live);
        prop_assert_eq!(b.total_used(), 0);
    }

    /// "released permit returns capacity": whatever succeeded, dropping it
    /// in any order restores every pool to exactly empty, and in-use never
    /// exceeded the configured maximum meanwhile.
    #[test]
    fn released_permits_return_all_capacity(
        reqs in prop::collection::vec(prop::collection::vec((0usize..10, 0u64..60), 1..4), 1..60),
        order in prop::collection::vec(any::<u32>(), 60),
    ) {
        let limits: Vec<(ResourceKind, u64)> = ResourceKind::ALL.iter().map(|k| (*k, 100)).collect();
        let pools = ResourcePools::new(&limits);
        let mut held = Vec::new();
        for r in reqs {
            let req: Vec<_> = r.into_iter().map(|(k, n)| (ResourceKind::ALL[k], n)).collect();
            if let Ok(b) = pools.acquire(&req, None) { held.push(b); }
            for k in ResourceKind::ALL { prop_assert!(pools.in_use(k) <= 100); }
        }
        for o in order {
            if held.is_empty() { break; }
            drop(held.swap_remove(o as usize % held.len()));
        }
        drop(held);
        for k in ResourceKind::ALL { prop_assert_eq!(pools.in_use(k), 0, "{:?}", k); }
    }

    /// A failed multi-resource acquire leaves nothing behind (no partial hold).
    #[test]
    fn failed_acquires_leave_no_trace(
        pre in 0u64..100,
        want in prop::collection::vec((0usize..10, 1u64..150), 1..5),
    ) {
        let limits: Vec<_> = ResourceKind::ALL.iter().map(|k| (*k, 100)).collect();
        let pools = ResourcePools::new(&limits);
        let _squat = pools.acquire(&[(ResourceKind::Streams, pre)], None).unwrap();
        let before: Vec<u64> = ResourceKind::ALL.iter().map(|k| pools.in_use(*k)).collect();
        let req: Vec<_> = want.into_iter().map(|(k, n)| (ResourceKind::ALL[k], n)).collect();
        if pools.acquire(&req, None).is_err() {
            let after: Vec<u64> = ResourceKind::ALL.iter().map(|k| pools.in_use(*k)).collect();
            prop_assert_eq!(before, after);
        }
    }
}

#[derive(Debug, Clone)]
enum StorageOp {
    Reserve(u8, u64),
    Cancel(usize),
    CancelTwice(usize),
    Commit(usize),
}

fn owner(i: u8) -> ResourceOwner {
    [
        ResourceOwner::Files,
        ResourceOwner::Messaging,
        ResourceOwner::Core,
        ResourceOwner::Calls,
        ResourceOwner::Dtn,
    ][usize::from(i) % 5]
        .clone()
}

proptest! {
    /// "critical reserve cannot be consumed by bulk", plus "duplicate
    /// reservation release does not underflow".
    #[test]
    fn bulk_never_eats_the_critical_reserve_and_double_release_is_harmless(
        ops in prop::collection::vec(
            prop_oneof![
                (0u8..5, 0u64..400).prop_map(|(o, n)| StorageOp::Reserve(o, n)),
                (0usize..64).prop_map(StorageOp::Cancel),
                (0usize..64).prop_map(StorageOp::CancelTwice),
                (0usize..64).prop_map(StorageOp::Commit),
            ],
            1..200,
        )
    ) {
        const CAP: u64 = 1_000;
        const RESERVE: u64 = 300;
        let mut s = StorageReservations::new(CAP, CriticalStorageReserve { reserved_bytes: RESERVE });
        let mut live = Vec::new();
        for op in ops {
            match op {
                StorageOp::Reserve(o, n) => {
                    let ow = owner(o);
                    let is_bulk = matches!(ow, ResourceOwner::Files);
                    if let Ok(id) = s.reserve(n, ow, u64::MAX / 2, 0, true) {
                        live.push(id);
                        if is_bulk {
                            // the moment bulk succeeds, the reserve is intact
                            prop_assert!(s.reserved_bytes() <= CAP - RESERVE);
                        }
                    }
                }
                StorageOp::Cancel(i) => if !live.is_empty() { let id = live.swap_remove(i % live.len()); prop_assert!(s.cancel(id).is_some()); },
                StorageOp::CancelTwice(i) => if !live.is_empty() {
                    let id = live.swap_remove(i % live.len());
                    let before = s.reserved_bytes();
                    let first = s.cancel(id);
                    let after_first = s.reserved_bytes();
                    prop_assert!(first.is_some());
                    prop_assert_eq!(after_first, before - first.unwrap().bytes);
                    prop_assert!(s.cancel(id).is_none(), "second release is a no-op");
                    prop_assert_eq!(s.reserved_bytes(), after_first, "no underflow, no change");
                },
                StorageOp::Commit(i) => if !live.is_empty() { let id = live.swap_remove(i % live.len()); prop_assert!(s.commit(id).is_some()); },
            }
            prop_assert!(s.reserved_bytes() <= CAP);
        }
    }

    /// "peer quota <= global quota": for every profile, trust class, and
    /// random usage pattern, per-peer accounting never exceeds the
    /// (clamped) quota, which never exceeds the device-wide limit.
    #[test]
    fn peer_quota_never_exceeds_the_global_limit(
        profile in 0usize..5, trust in 0usize..6,
        deltas in prop::collection::vec((0u32..40, 0u32..600, 0u64..1_000_000_000, 0u32..40, 0u64..100_000_000), 1..80),
    ) {
        let p = [ResourceProfile::Embedded, ResourceProfile::MobileLow, ResourceProfile::MobileNormal, ResourceProfile::Desktop, ResourceProfile::Server][profile];
        let t = [TrustClass::Unknown, TrustClass::Known, TrustClass::Verified, TrustClass::Organization, TrustClass::Authority, TrustClass::LocalOwnDevice][trust];
        let cfg = ResourceConfig::for_profile(p);
        let q = PeerQuota::for_trust_class(t).clamped_to(&cfg);
        prop_assert!(q.max_active_streams <= cfg.network.max_streams);
        prop_assert!(q.max_dtn_relay_bytes <= cfg.storage.relay_quota_bytes);
        let mut u = PeerUsageCounters::default();
        for (st, msg, bytes, files, dtn) in deltas {
            let d = PeerUsageDelta { active_streams: st, pending_messages: msg, inbound_bytes: bytes, staged_files: files, dtn_relay_bytes: dtn };
            let r = u.try_charge(&q, d, true);
            prop_assert!(matches!(r, AdmissionResult::Accepted | AdmissionResult::Deferred(_)));
            prop_assert!(u.active_streams <= q.max_active_streams && u.active_streams <= cfg.network.max_streams);
            prop_assert!(u.dtn_relay_bytes <= q.max_dtn_relay_bytes);
            prop_assert!(u.inbound_bytes <= q.max_inbound_bytes && u.staged_files <= q.max_staged_files);
            if matches!(r, AdmissionResult::Accepted) && st % 3 == 0 { u.release(d); }
        }
    }
}
