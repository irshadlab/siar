//! §37 "Routing Recovery", §38 "Resource Recovery", §39 "Storage
//! Reservation Recovery".
//!
//! §37: persisted route history only *bootstraps*; every candidate is
//! revalidated. Enforced with two types — an [`UnvalidatedCandidate`]
//! cannot be used for sending, and the only way to get a
//! [`ValidatedCandidate`] is [`UnvalidatedCandidate::validated`].
//! §38/§39: in-memory permits vanish on crash and are simply reset;
//! persistent reservations are reconciled against live operations.

use crate::chunk_recovery::TransferId;
use serde::{Deserialize, Serialize};
use siar_routing_policy::types::TransportKind;
use std::collections::HashSet;

// ---------------------------------------------------------------------
// §37
// ---------------------------------------------------------------------

/// What §37 says may be persisted.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RouteHints {
    pub last_successful_transport: Option<TransportKind>,
    /// Opaque endpoint hints (address strings, tickets, …).
    pub endpoint_hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnvalidatedCandidate {
    pub transport: Option<TransportKind>,
    pub endpoint: String,
}

/// A candidate that has passed a live reachability/identity check in
/// *this* runtime lifetime. Cannot be constructed outside this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCandidate {
    transport: Option<TransportKind>,
    endpoint: String,
}

impl ValidatedCandidate {
    pub fn transport(&self) -> Option<TransportKind> {
        self.transport
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl UnvalidatedCandidate {
    /// Consumes the hint; `live_check_passed` is the result of a real
    /// probe the caller just performed.
    pub fn validated(self, live_check_passed: bool) -> Option<ValidatedCandidate> {
        live_check_passed.then_some(ValidatedCandidate {
            transport: self.transport,
            endpoint: self.endpoint,
        })
    }
}

/// Bootstrap list: the last-good transport's endpoint hints first
/// (hints are stored in preference order), everything flagged
/// unvalidated.
pub fn bootstrap_candidates(h: &RouteHints) -> Vec<UnvalidatedCandidate> {
    h.endpoint_hints
        .iter()
        .map(|e| UnvalidatedCandidate {
            transport: h.last_successful_transport,
            endpoint: e.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------
// §38
// ---------------------------------------------------------------------

/// Resource classes by what happens to them on restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceClass {
    MemoryPermit,
    StreamPermit,
    CpuPermit,
    StorageReservation,
    StagingAllocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartTreatment {
    /// Died with the process; start from a clean budget.
    ResetToZero,
    /// Persisted; must go through [`reconcile_reservations`].
    Reconcile,
}

pub const fn restart_treatment(c: ResourceClass) -> RestartTreatment {
    match c {
        ResourceClass::MemoryPermit | ResourceClass::StreamPermit | ResourceClass::CpuPermit => {
            RestartTreatment::ResetToZero
        }
        ResourceClass::StorageReservation | ResourceClass::StagingAllocation => {
            RestartTreatment::Reconcile
        }
    }
}

// ---------------------------------------------------------------------
// §39
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReservationId(uuid::Uuid);

impl ReservationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for ReservationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageReservation {
    pub id: ReservationId,
    pub kind: ReservationKind,
    pub owner: Option<TransferId>,
    pub bytes: u64,
    pub created_at_millis: u64,
    /// Whether staged bytes for this reservation still exist on disk.
    pub staged_bytes_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReservationKind {
    Storage,
    StagingAllocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationAction {
    /// §39: active operation exists → restore.
    Restore,
    /// No active operation, but inside the safety window: leave it,
    /// look again later.
    HoldSafetyWindow,
    /// No active operation, but staged bytes remain: ownership of the
    /// bytes is §24's orphan cleanup's call; releasing the reservation
    /// first would let the space be double-booked.
    DeferToOrphanCleanup,
    /// No active operation, past the window, nothing staged: release.
    Release,
}

pub fn reconcile_reservation(
    r: &StorageReservation,
    active_ops: &HashSet<TransferId>,
    now_millis: u64,
    safety_window_millis: u64,
) -> ReservationAction {
    if r.owner.is_some_and(|o| active_ops.contains(&o)) {
        return ReservationAction::Restore;
    }
    if now_millis.saturating_sub(r.created_at_millis) < safety_window_millis {
        return ReservationAction::HoldSafetyWindow;
    }
    if r.staged_bytes_present {
        return ReservationAction::DeferToOrphanCleanup;
    }
    ReservationAction::Release
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationPlan {
    pub actions: Vec<(ReservationId, ReservationAction)>,
    /// Bytes that stay reserved after applying the plan (restored,
    /// held, or deferred) — the figure the allocator must start from.
    pub bytes_still_reserved: u64,
    pub bytes_released: u64,
}

pub fn reconcile_reservations(
    rs: &[StorageReservation],
    active_ops: &HashSet<TransferId>,
    now_millis: u64,
    safety_window_millis: u64,
) -> ReservationPlan {
    let mut plan = ReservationPlan {
        actions: Vec::with_capacity(rs.len()),
        bytes_still_reserved: 0,
        bytes_released: 0,
    };
    for r in rs {
        let a = reconcile_reservation(r, active_ops, now_millis, safety_window_millis);
        match a {
            ReservationAction::Release => {
                plan.bytes_released = plan.bytes_released.saturating_add(r.bytes)
            }
            _ => plan.bytes_still_reserved = plan.bytes_still_reserved.saturating_add(r.bytes),
        }
        plan.actions.push((r.id, a));
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_only_bootstrap_and_must_be_revalidated() {
        let h = RouteHints {
            last_successful_transport: Some(TransportKind::MeshRelay),
            endpoint_hints: vec!["a".into(), "b".into()],
        };
        let c = bootstrap_candidates(&h);
        assert_eq!(c.len(), 2);
        assert!(c[0].clone().validated(false).is_none());
        let ok = c[1].clone().validated(true).unwrap();
        assert_eq!(ok.endpoint(), "b");
        assert_eq!(ok.transport(), Some(TransportKind::MeshRelay));
    }

    #[test]
    fn empty_hints_give_no_candidates() {
        assert!(bootstrap_candidates(&RouteHints::default()).is_empty());
    }

    #[test]
    fn in_memory_permits_reset_and_persistent_reservations_reconcile() {
        use ResourceClass::*;
        for c in [MemoryPermit, StreamPermit, CpuPermit] {
            assert_eq!(restart_treatment(c), RestartTreatment::ResetToZero);
        }
        for c in [StorageReservation, StagingAllocation] {
            assert_eq!(restart_treatment(c), RestartTreatment::Reconcile);
        }
    }

    fn res(
        owner: Option<TransferId>,
        bytes: u64,
        created: u64,
        staged: bool,
    ) -> StorageReservation {
        StorageReservation {
            id: ReservationId::new(),
            kind: ReservationKind::Storage,
            owner,
            bytes,
            created_at_millis: created,
            staged_bytes_present: staged,
        }
    }

    #[test]
    fn all_four_reservation_outcomes() {
        let live = TransferId::new();
        let active: HashSet<_> = [live].into();
        let w = 1_000;
        let now = 10_000;
        assert_eq!(
            reconcile_reservation(&res(Some(live), 1, 0, false), &active, now, w),
            ReservationAction::Restore
        );
        assert_eq!(
            reconcile_reservation(
                &res(Some(TransferId::new()), 1, now - 1, false),
                &active,
                now,
                w
            ),
            ReservationAction::HoldSafetyWindow
        );
        assert_eq!(
            reconcile_reservation(&res(None, 1, 0, true), &active, now, w),
            ReservationAction::DeferToOrphanCleanup
        );
        assert_eq!(
            reconcile_reservation(&res(None, 1, 0, false), &active, now, w),
            ReservationAction::Release
        );
    }

    #[test]
    fn active_owner_wins_even_inside_or_outside_the_window() {
        let live = TransferId::new();
        let active: HashSet<_> = [live].into();
        for created in [0, 9_999] {
            assert_eq!(
                reconcile_reservation(&res(Some(live), 1, created, true), &active, 10_000, 1_000),
                ReservationAction::Restore
            );
        }
    }

    #[test]
    fn plan_totals_split_released_from_still_reserved() {
        let live = TransferId::new();
        let active: HashSet<_> = [live].into();
        let rs = [
            res(Some(live), 100, 0, false), // restore
            res(None, 40, 0, false),        // release
            res(None, 7, 0, true),          // deferred
        ];
        let p = reconcile_reservations(&rs, &active, 10_000, 1_000);
        assert_eq!(p.bytes_still_reserved, 107);
        assert_eq!(p.bytes_released, 40);
        assert_eq!(p.actions.len(), 3);
    }

    #[test]
    fn reconcile_is_stable_across_repeated_runs() {
        let active = HashSet::new();
        let rs = [res(None, 5, 0, false)];
        let a = reconcile_reservations(&rs, &active, 5_000, 1_000);
        let b = reconcile_reservations(&rs, &active, 5_000, 1_000);
        assert_eq!(a, b);
    }
}
