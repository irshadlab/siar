//! §137 accounting across process restart, §138 crash-recovery
//! integration, §140 resource state as projection.
//!
//! What survives a restart and what does not (§137):
//! - **Durable**: storage reservations — they stand for bytes promised on
//!   disk, so they are reconstructed;
//! - **Rebuilt empty**: memory, stream and CPU permits — they belong to
//!   the dead process and simply start at zero.
//!
//! Durable records must be keyed by a *durable* operation id.
//! [`crate::storage::ReservationId`] is a per-process counter that
//! restarts at 1, so persisting it would alias unrelated reservations
//! after a crash; recovery therefore matches on [`OpId`] and mints fresh
//! in-memory ids when it restores.
//!
//! [`plan_recovery`] is pure: it reads what was found on disk and in the
//! work queue and returns what to do, never touching anything itself.
//! Part 09's reconciler applies the plan.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::admission::ResourceOwner;
use crate::dynamic_config::{apply_update, ApplyReport, ConfigUpdate, CurrentUsage};
use crate::policy_layers::HardLimits;
use crate::profile::{ConfigError, ResourceConfig};
use crate::storage::{ReservationId, StorageReservations};
use crate::types::ResourceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OpId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DurableResId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Reconstructed after restart.
    Durable,
    /// Belongs to the dead process; starts empty.
    Rebuilt,
}

pub fn durability_of(kind: ResourceKind) -> Durability {
    match kind {
        ResourceKind::Storage => Durability::Durable,
        _ => Durability::Rebuilt,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableReservation {
    pub id: DurableResId,
    /// The operation it was made for, if the record says.
    pub op: Option<OpId>,
    pub bytes: u64,
    pub owner: ResourceOwner,
    pub expires_at_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkEntry {
    pub op: OpId,
    pub needs_storage: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryInput {
    pub reservations: Vec<DurableReservation>,
    /// Operations that own a staging file on disk.
    pub staging_files: Vec<OpId>,
    pub work_queue: Vec<WorkEntry>,
    /// Operations whose persisted state says "active" at crash time.
    pub active_ops: Vec<OpId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryPlan {
    /// Live, unexpired reservation: rebuild its in-memory accounting.
    pub restore: Vec<DurableResId>,
    pub release_expired: Vec<DurableResId>,
    /// No operation owns it (or a duplicate of one that does).
    pub release_orphans: Vec<DurableResId>,
    pub delete_orphan_staging: Vec<OpId>,
    /// Was "active" in the dead process; nothing is running now, so put
    /// it back to queued.
    pub reset_active_to_queued: Vec<OpId>,
    /// Active in persisted state but absent from the work queue:
    /// inconsistent, drop the state.
    pub drop_stale_active: Vec<OpId>,
    /// Needs storage but holds no live reservation: re-reserve (or defer).
    pub needs_reservation: Vec<OpId>,
}

pub fn plan_recovery(input: &RecoveryInput, now_millis: u64) -> RecoveryPlan {
    let queued: HashSet<OpId> = input.work_queue.iter().map(|w| w.op).collect();
    let active: HashSet<OpId> = input.active_ops.iter().copied().collect();
    let known: HashSet<OpId> = queued.union(&active).copied().collect();

    let mut restore = BTreeSet::new();
    let mut expired = BTreeSet::new();
    let mut orphans = BTreeSet::new();
    let mut has_live_reservation: HashSet<OpId> = HashSet::new();

    let mut by_id: Vec<&DurableReservation> = input.reservations.iter().collect();
    by_id.sort_by_key(|r| r.id); // deterministic: lowest id wins duplicates
    for r in by_id {
        let Some(op) = r.op.filter(|op| known.contains(op)) else {
            orphans.insert(r.id);
            continue;
        };
        if r.expires_at_millis <= now_millis {
            expired.insert(r.id);
        } else if has_live_reservation.insert(op) {
            restore.insert(r.id);
        } else {
            orphans.insert(r.id); // second live reservation for the same op
        }
    }

    let staging_orphans: BTreeSet<OpId> = input
        .staging_files
        .iter()
        .copied()
        .filter(|op| !known.contains(op))
        .collect();

    let needs: BTreeSet<OpId> = input
        .work_queue
        .iter()
        .filter(|w| w.needs_storage && !has_live_reservation.contains(&w.op))
        .map(|w| w.op)
        .collect();

    let (reset, stale): (BTreeSet<OpId>, BTreeSet<OpId>) = {
        let mut r = BTreeSet::new();
        let mut s = BTreeSet::new();
        for op in &active {
            if queued.contains(op) {
                r.insert(*op)
            } else {
                s.insert(*op)
            };
        }
        (r, s)
    };

    RecoveryPlan {
        restore: restore.into_iter().collect(),
        release_expired: expired.into_iter().collect(),
        release_orphans: orphans.into_iter().collect(),
        delete_orphan_staging: staging_orphans.into_iter().collect(),
        reset_active_to_queued: reset.into_iter().collect(),
        drop_stale_active: stale.into_iter().collect(),
        needs_reservation: needs.into_iter().collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreOutcome {
    /// Durable record → the fresh in-memory id minted for it.
    pub restored: Vec<(DurableResId, ReservationId)>,
    /// No longer fits (quota shrank, disk filled): the operation goes back
    /// through normal admission instead of being force-fitted.
    pub did_not_fit: Vec<DurableResId>,
}

/// Rebuild in-memory storage accounting for the plan's `restore` set.
pub fn restore_reservations(
    store: &mut StorageReservations,
    records: &[DurableReservation],
    plan: &RecoveryPlan,
    now_millis: u64,
) -> RestoreOutcome {
    let by_id: HashMap<DurableResId, &DurableReservation> =
        records.iter().map(|r| (r.id, r)).collect();
    let mut out = RestoreOutcome::default();
    for id in &plan.restore {
        let Some(r) = by_id.get(id) else { continue };
        let remaining = r.expires_at_millis.saturating_sub(now_millis);
        match store.reserve(r.bytes, r.owner.clone(), remaining, now_millis, true) {
            Ok(rid) => out.restored.push((*id, rid)),
            Err(_) => out.did_not_fit.push(*id),
        }
    }
    out
}

// ------------------------------------------------------------- §140

/// The only resource state that is persisted: user/app choices. Live
/// counters, pressure, permits and queues are runtime projections and
/// have no persisted form (nothing here, or anywhere in this crate's
/// operational state, derives `Serialize`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PersistentResourceSettings {
    pub user_storage_quota_bytes: Option<u64>,
    pub relay_storage_quota_bytes: Option<u64>,
    pub max_parallel_transfers: Option<u32>,
    pub data_saver: bool,
}

impl PersistentResourceSettings {
    /// Applies the saved choices atomically on top of a fresh profile
    /// config at startup (or on settings change). Invalid saved values
    /// are rejected as a whole and leave `cfg` untouched.
    pub fn apply_to(
        &self,
        cfg: &mut ResourceConfig,
        hard: &HardLimits,
    ) -> Result<ApplyReport, Vec<ConfigError>> {
        let mut candidate = *cfg;
        if let Some(q) = self.user_storage_quota_bytes {
            candidate.storage.quota_bytes = q;
        }
        let report = apply_update(
            &mut candidate,
            hard,
            &ConfigUpdate {
                relay_storage_quota: self.relay_storage_quota_bytes,
                max_parallel_transfers: self.max_parallel_transfers,
                data_saver: Some(self.data_saver),
            },
            &CurrentUsage::default(),
        )?;
        *cfg = candidate;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ResourceProfile;
    use crate::storage::CriticalStorageReserve;

    fn res(id: u64, op: Option<u64>, bytes: u64, exp: u64) -> DurableReservation {
        DurableReservation {
            id: DurableResId(id),
            op: op.map(OpId),
            bytes,
            owner: ResourceOwner::Files,
            expires_at_millis: exp,
        }
    }

    fn work(op: u64, needs: bool) -> WorkEntry {
        WorkEntry {
            op: OpId(op),
            needs_storage: needs,
        }
    }

    #[test]
    fn spec_137_split_only_storage_is_durable() {
        assert_eq!(durability_of(ResourceKind::Storage), Durability::Durable);
        for k in [
            ResourceKind::Memory,
            ResourceKind::Streams,
            ResourceKind::Cpu,
            ResourceKind::Connections,
        ] {
            assert_eq!(durability_of(k), Durability::Rebuilt);
        }
    }

    #[test]
    fn orphan_reservations_and_staging_are_found_and_live_ones_restored() {
        let input = RecoveryInput {
            reservations: vec![
                res(1, Some(10), 100, 9_999), // live op, unexpired -> restore
                res(2, Some(99), 100, 9_999), // op unknown        -> orphan
                res(3, None, 100, 9_999),     // no op recorded    -> orphan
                res(4, Some(11), 100, 5),     // expired           -> release_expired
            ],
            staging_files: vec![OpId(10), OpId(77)],
            work_queue: vec![work(10, true), work(11, true)],
            active_ops: vec![],
        };
        let p = plan_recovery(&input, 1_000);
        assert_eq!(p.restore, vec![DurableResId(1)]);
        assert_eq!(p.release_orphans, vec![DurableResId(2), DurableResId(3)]);
        assert_eq!(p.release_expired, vec![DurableResId(4)]);
        assert_eq!(p.delete_orphan_staging, vec![OpId(77)]);
        assert_eq!(
            p.needs_reservation,
            vec![OpId(11)],
            "its only reservation expired"
        );
    }

    #[test]
    fn active_ops_from_the_dead_process_are_reset_or_dropped() {
        let input = RecoveryInput {
            work_queue: vec![work(1, false), work(2, false)],
            active_ops: vec![OpId(1), OpId(5)],
            ..Default::default()
        };
        let p = plan_recovery(&input, 0);
        assert_eq!(p.reset_active_to_queued, vec![OpId(1)]);
        assert_eq!(
            p.drop_stale_active,
            vec![OpId(5)],
            "active but not in the work queue"
        );
    }

    #[test]
    fn an_active_op_keeps_its_staging_even_if_it_left_the_queue() {
        let input = RecoveryInput {
            staging_files: vec![OpId(5)],
            active_ops: vec![OpId(5)],
            ..Default::default()
        };
        assert!(plan_recovery(&input, 0).delete_orphan_staging.is_empty());
    }

    #[test]
    fn duplicate_live_reservations_keep_only_the_lowest_id() {
        let input = RecoveryInput {
            reservations: vec![res(8, Some(1), 10, 9_999), res(3, Some(1), 10, 9_999)],
            work_queue: vec![work(1, true)],
            ..Default::default()
        };
        let p = plan_recovery(&input, 0);
        assert_eq!(p.restore, vec![DurableResId(3)]);
        assert_eq!(p.release_orphans, vec![DurableResId(8)]);
        assert!(p.needs_reservation.is_empty());
    }

    #[test]
    fn planning_is_deterministic_regardless_of_input_order() {
        let mut input = RecoveryInput {
            reservations: vec![
                res(5, None, 1, 9),
                res(2, None, 1, 9),
                res(9, Some(1), 1, 99_999),
            ],
            staging_files: vec![OpId(30), OpId(20), OpId(30)],
            work_queue: vec![work(2, true), work(1, true)],
            active_ops: vec![OpId(2), OpId(1)],
        };
        let a = plan_recovery(&input, 0);
        input.reservations.reverse();
        input.staging_files.reverse();
        input.work_queue.reverse();
        input.active_ops.reverse();
        assert_eq!(a, plan_recovery(&input, 0));
        assert_eq!(
            a.delete_orphan_staging,
            vec![OpId(20), OpId(30)],
            "sorted and deduplicated"
        );
    }

    #[test]
    fn empty_world_is_an_empty_plan() {
        assert_eq!(
            plan_recovery(&RecoveryInput::default(), 0),
            RecoveryPlan::default()
        );
    }

    #[test]
    fn restore_rebuilds_accounting_with_fresh_ids_and_remaining_ttl() {
        let mut store =
            StorageReservations::new(1_000, CriticalStorageReserve { reserved_bytes: 0 });
        let records = vec![res(1, Some(10), 400, 5_000), res(2, Some(11), 400, 5_000)];
        let input = RecoveryInput {
            reservations: records.clone(),
            work_queue: vec![work(10, true), work(11, true)],
            ..Default::default()
        };
        let plan = plan_recovery(&input, 1_000);
        let out = restore_reservations(&mut store, &records, &plan, 1_000);
        assert_eq!(out.restored.len(), 2);
        assert_ne!(out.restored[0].1, out.restored[1].1, "fresh in-memory ids");
        assert_eq!(store.reserved_bytes(), 800);
        // ttl was the *remaining* 4000ms, not a fresh full lease
        assert_eq!(store.expire_stale(4_999).len(), 0);
        assert_eq!(store.expire_stale(5_000).len(), 2);
    }

    #[test]
    fn a_reservation_that_no_longer_fits_goes_back_through_admission() {
        let mut store = StorageReservations::new(500, CriticalStorageReserve { reserved_bytes: 0 });
        let records = vec![res(1, Some(10), 400, 9_000), res(2, Some(11), 400, 9_000)];
        let input = RecoveryInput {
            reservations: records.clone(),
            work_queue: vec![work(10, true), work(11, true)],
            ..Default::default()
        };
        let plan = plan_recovery(&input, 0);
        let out = restore_reservations(&mut store, &records, &plan, 0);
        assert_eq!(out.restored.len(), 1);
        assert_eq!(out.did_not_fit, vec![DurableResId(2)]);
        assert!(
            store.reserved_bytes() <= 500,
            "never force-fitted past capacity"
        );
    }

    #[test]
    fn settings_apply_atomically_and_roundtrip() {
        let p = ResourceProfile::Desktop;
        let hard = HardLimits::for_profile(p);
        let mut cfg = ResourceConfig::for_profile(p);
        let s = PersistentResourceSettings {
            user_storage_quota_bytes: Some(cfg.storage.quota_bytes / 2),
            relay_storage_quota_bytes: Some(cfg.storage.relay_quota_bytes / 4),
            max_parallel_transfers: Some(4),
            data_saver: true,
        };
        let bytes = postcard::to_allocvec(&s).unwrap();
        let back: PersistentResourceSettings = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, s);
        back.apply_to(&mut cfg, &hard).unwrap();
        assert_eq!(cfg.network.max_transfers, 4);
        assert!(cfg.data_saver);
    }

    #[test]
    fn bad_saved_settings_are_rejected_whole_leaving_config_untouched() {
        let p = ResourceProfile::Desktop;
        let hard = HardLimits::for_profile(p);
        let mut cfg = ResourceConfig::for_profile(p);
        let before = cfg;
        let bad = PersistentResourceSettings {
            user_storage_quota_bytes: Some(1), // below the critical reserve
            max_parallel_transfers: Some(2),
            ..Default::default()
        };
        assert!(bad.apply_to(&mut cfg, &hard).is_err());
        assert_eq!(
            cfg, before,
            "even the valid transfer setting was not applied"
        );
    }
}
