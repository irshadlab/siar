//! §46 "Storage Watermarks", §47 "Storage Pressure Actions" (partial —
//! see below), §48 "Critical Storage Reserve", §49 "Storage
//! Reservation", §50 "Storage Reservation Record".
//!
//! §47's four pressure-level *actions* (reduce prefetch, pause bulk,
//! reject large relay bundles, ...) are policy decisions for whatever
//! subsystem owns prefetching/caching/relay — this module classifies
//! pressure (§46) and enforces the one mechanical rule §47-48 both
//! depend on (the critical reserve bulk storage can't touch), but
//! doesn't reach into cache-eviction or relay-rejection logic that
//! lives in other crates entirely.

use crate::admission::{AdmissionResult, DeferredReason, DropReason, ResourceOwner};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// §81's own enum (referenced again here since §46's storage-specific
/// prose uses a fourth label, "Full", for the same concept §81 already
/// names `Exhausted` — [`StorageWatermarks::classify`] returns this
/// type rather than a parallel storage-only enum, so "Full" and
/// `Exhausted` are structurally the same value, not two names a caller
/// could accidentally treat as different states).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PressureState {
    #[default]
    Normal,
    Elevated,
    Critical,
    Exhausted,
}

/// §46's own worked thresholds (Normal < 70%, Elevated 70-85%,
/// Critical 85-95%, Full/Exhausted > 95%) — used verbatim as the
/// default here, unlike most numeric defaults elsewhere in this
/// crate, because the spec gives these as its own concrete example
/// rather than leaving the number unstated. §46's closing line ("Exact
/// thresholds configurable") is why the fields are still adjustable
/// rather than hardcoded constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageWatermarks {
    pub elevated_at_percent: u8,
    pub critical_at_percent: u8,
    pub exhausted_at_percent: u8,
}

impl StorageWatermarks {
    pub const fn spec_example() -> Self {
        Self {
            elevated_at_percent: 70,
            critical_at_percent: 85,
            exhausted_at_percent: 95,
        }
    }

    /// §46's classification, applied to a live `(used, capacity)` pair.
    pub fn classify(&self, used_bytes: u64, capacity_bytes: u64) -> PressureState {
        if capacity_bytes == 0 {
            return PressureState::Exhausted;
        }
        // Integer percent via a wider intermediate to avoid overflow
        // on large byte counts before the division.
        let percent = ((used_bytes as u128 * 100) / capacity_bytes as u128) as u64;
        if percent > self.exhausted_at_percent as u64 {
            PressureState::Exhausted
        } else if percent > self.critical_at_percent as u64 {
            PressureState::Critical
        } else if percent > self.elevated_at_percent as u64 {
            PressureState::Elevated
        } else {
            PressureState::Normal
        }
    }
}

/// §48: "Reserve bytes for: identity changes / small messages / SOS /
/// delivery ACKs / security events. Bulk files must not consume this
/// reserve." §48 names `Files` specifically as the excluded category
/// — [`CriticalStorageReserve::applies_to`] denies only
/// [`ResourceOwner::Files`] and permits every other owner, since that
/// is the one exclusion the spec actually states. A finer per-request
/// classification (e.g. "this specific `Messaging` request is
/// secretly a bulk attachment") isn't expressible with `ResourceOwner`
/// alone and isn't guessed at here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CriticalStorageReserve {
    pub reserved_bytes: u64,
}

impl CriticalStorageReserve {
    pub fn applies_to(owner: &ResourceOwner) -> bool {
        !matches!(owner, ResourceOwner::Files)
    }
}

/// A process-local monotonic id generator — the spec's own `50`
/// sketch names the type `ReservationId` but gives it no internal
/// shape, so a simple atomic counter (no `uuid` dependency needed for
/// this crate) is this module's own reasoned choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReservationId(u64);

static NEXT_RESERVATION_ID: AtomicU64 = AtomicU64::new(1);

impl ReservationId {
    fn next() -> Self {
        Self(NEXT_RESERVATION_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// §50, field-for-field, with one substitution: `expires_at` is
/// `u64` milliseconds here rather than a `Timestamp` type, matching
/// every other time-sensitive value already in this crate
/// (`token_bucket`'s `now_millis`) rather than pulling in a
/// `Timestamp` type from another crate for one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageReservation {
    pub id: ReservationId,
    pub bytes: u64,
    pub owner: ResourceOwner,
    pub expires_at_millis: u64,
}

/// §49's reservation lifecycle: reserve space before a large transfer
/// starts (so the transfer can't be admitted, then fail from actual
/// overcommit once real bytes arrive), and expire that reservation if
/// the transfer never actually starts (§49: "Reservation itself must
/// expire if transfer never starts").
///
/// This type tracks *reservations* and a capacity ceiling; it does not
/// track already-committed storage usage itself (`used_bytes` is
/// supplied by the caller via [`StorageReservations::set_used_bytes`])
/// — real on-disk accounting is a distinct, larger concern this pass
/// doesn't attempt.
#[derive(Debug, Clone)]
pub struct StorageReservations {
    capacity_bytes: u64,
    used_bytes: u64,
    reserve: CriticalStorageReserve,
    pending: HashMap<ReservationId, StorageReservation>,
}

impl StorageReservations {
    pub fn new(capacity_bytes: u64, reserve: CriticalStorageReserve) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            reserve,
            pending: HashMap::new(),
        }
    }

    pub fn set_used_bytes(&mut self, used_bytes: u64) {
        self.used_bytes = used_bytes;
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.pending.values().map(|r| r.bytes).sum()
    }

    /// §48: reservable space for a bulk (`Files`) owner excludes the
    /// critical reserve; every other owner may draw against the full
    /// capacity, reserve included.
    fn effective_capacity_for(&self, owner: &ResourceOwner) -> u64 {
        if CriticalStorageReserve::applies_to(owner) {
            self.capacity_bytes
        } else {
            self.capacity_bytes
                .saturating_sub(self.reserve.reserved_bytes)
        }
    }

    fn purge_expired(&mut self, now_millis: u64) {
        self.pending.retain(|_, r| r.expires_at_millis > now_millis);
    }

    /// §49: reserve `bytes` for `owner`, expiring at
    /// `now_millis + ttl_millis` if never committed or cancelled.
    /// Expired reservations are purged first, so their bytes are
    /// never counted against the new request.
    pub fn reserve(
        &mut self,
        bytes: u64,
        owner: ResourceOwner,
        ttl_millis: u64,
        now_millis: u64,
        durable: bool,
    ) -> Result<ReservationId, AdmissionResult> {
        self.purge_expired(now_millis);

        let effective_capacity = self.effective_capacity_for(&owner);
        let committed = self.used_bytes.saturating_add(self.reserved_bytes());
        let fits = committed.saturating_add(bytes) <= effective_capacity;

        if !fits {
            return Err(if durable {
                AdmissionResult::Deferred(DeferredReason::AwaitingBudget)
            } else {
                AdmissionResult::Dropped(DropReason::Stale)
            });
        }

        let id = ReservationId::next();
        self.pending.insert(
            id,
            StorageReservation {
                id,
                bytes,
                owner,
                expires_at_millis: now_millis.saturating_add(ttl_millis),
            },
        );
        Ok(id)
    }

    /// The reserved transfer actually started — releases the
    /// reservation bookkeeping (the caller is responsible for then
    /// reflecting the real bytes via [`StorageReservations::set_used_bytes`]
    /// as they land, since this module doesn't track live disk writes).
    pub fn commit(&mut self, id: ReservationId) -> Option<StorageReservation> {
        self.pending.remove(&id)
    }

    /// Explicit cancellation (not expiry) — e.g. the caller decided
    /// not to proceed.
    pub fn cancel(&mut self, id: ReservationId) -> Option<StorageReservation> {
        self.pending.remove(&id)
    }

    /// Removes and returns every reservation that's expired as of
    /// `now_millis` (§49) — for a caller that wants to know what just
    /// lapsed, e.g. to log it, distinct from [`StorageReservations::reserve`]'s
    /// own internal silent purge.
    pub fn expire_stale(&mut self, now_millis: u64) -> Vec<StorageReservation> {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, r)| r.expires_at_millis <= now_millis)
            .map(|(_, r)| r.clone())
            .collect();
        for r in &expired {
            self.pending.remove(&r.id);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_matches_46s_own_worked_thresholds() {
        let watermarks = StorageWatermarks::spec_example();
        assert_eq!(watermarks.classify(50, 100), PressureState::Normal);
        assert_eq!(watermarks.classify(75, 100), PressureState::Elevated);
        assert_eq!(watermarks.classify(90, 100), PressureState::Critical);
        assert_eq!(watermarks.classify(96, 100), PressureState::Exhausted);
    }

    #[test]
    fn zero_capacity_is_always_exhausted_not_a_division_by_zero_panic() {
        let watermarks = StorageWatermarks::spec_example();
        assert_eq!(watermarks.classify(0, 0), PressureState::Exhausted);
    }

    #[test]
    fn bulk_files_cannot_reserve_into_the_critical_reserve() {
        // §48's exact rule.
        let reserve = CriticalStorageReserve {
            reserved_bytes: 100,
        };
        let mut reservations = StorageReservations::new(1000, reserve);
        reservations.set_used_bytes(920); // 80 bytes free total, all of it reserve

        let result = reservations.reserve(50, ResourceOwner::Files, 60_000, 0, true);
        assert_eq!(
            result,
            Err(AdmissionResult::Deferred(DeferredReason::AwaitingBudget))
        );
    }

    #[test]
    fn non_bulk_owner_may_draw_into_the_critical_reserve() {
        let reserve = CriticalStorageReserve {
            reserved_bytes: 100,
        };
        let mut reservations = StorageReservations::new(1000, reserve);
        reservations.set_used_bytes(920); // same 80 bytes free, but SOS is allowed to use reserve

        let result = reservations.reserve(50, ResourceOwner::Dtn, 60_000, 0, true);
        assert!(result.is_ok());
    }

    #[test]
    fn expired_reservation_is_purged_and_frees_its_bytes() {
        // §49: "Reservation itself must expire if transfer never starts."
        let reserve = CriticalStorageReserve { reserved_bytes: 0 };
        let mut reservations = StorageReservations::new(100, reserve);

        reservations
            .reserve(90, ResourceOwner::Files, 1_000, 0, true)
            .unwrap();
        // Still within its TTL: a second large reservation should be
        // blocked by the first one's still-live hold.
        assert!(reservations
            .reserve(90, ResourceOwner::Files, 1_000, 500, true)
            .is_err());

        // Past the first reservation's expiry: it's purged, freeing
        // its bytes for a new reservation.
        let result = reservations.reserve(90, ResourceOwner::Files, 1_000, 1_500, true);
        assert!(result.is_ok());
    }

    #[test]
    fn commit_releases_the_reservation_bookkeeping() {
        let reserve = CriticalStorageReserve { reserved_bytes: 0 };
        let mut reservations = StorageReservations::new(100, reserve);
        let id = reservations
            .reserve(50, ResourceOwner::Files, 60_000, 0, true)
            .unwrap();
        assert_eq!(reservations.reserved_bytes(), 50);

        reservations.commit(id);
        assert_eq!(reservations.reserved_bytes(), 0);
    }

    #[test]
    fn reservation_prevents_overcommit_beyond_capacity() {
        let reserve = CriticalStorageReserve { reserved_bytes: 0 };
        let mut reservations = StorageReservations::new(100, reserve);
        reservations
            .reserve(80, ResourceOwner::Files, 60_000, 0, true)
            .unwrap();

        let result = reservations.reserve(30, ResourceOwner::Files, 60_000, 0, false);
        assert_eq!(result, Err(AdmissionResult::Dropped(DropReason::Stale)));
    }

    #[test]
    fn expire_stale_reports_exactly_what_lapsed() {
        let reserve = CriticalStorageReserve { reserved_bytes: 0 };
        let mut reservations = StorageReservations::new(1000, reserve);
        let a = reservations
            .reserve(10, ResourceOwner::Files, 100, 0, true)
            .unwrap();
        let _b = reservations
            .reserve(10, ResourceOwner::Files, 10_000, 0, true)
            .unwrap();

        let lapsed = reservations.expire_stale(200);
        assert_eq!(lapsed.len(), 1);
        assert_eq!(lapsed[0].id, a);
        assert_eq!(reservations.reserved_bytes(), 10); // only b remains
    }
}
