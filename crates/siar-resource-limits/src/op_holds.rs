//! §135 accounting across retries, §136 across path switches.
//!
//! A logical operation owns two kinds of holds:
//! - **durable**: transfer slot, storage reservation — kept for the whole
//!   operation, across every retry and path switch;
//! - **per-attempt**: stream, connection, buffers — released *before* the
//!   next attempt acquires, so retries never accumulate permits and peak
//!   usage is that of one attempt, not several.

use crate::reservation::{PermitBundle, ReservationError, ResourcePools};
use crate::types::ResourceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AttemptError {
    #[error("retry budget exhausted after {0} attempts")]
    TooManyAttempts(u32),
    #[error(transparent)]
    Resource(#[from] ReservationError),
}

#[derive(Debug)]
pub struct OperationHolds {
    durable: PermitBundle,
    attempt: Option<PermitBundle>,
    attempts: u32,
    path_switches: u32,
    max_attempts: u32,
}

impl OperationHolds {
    pub fn new(durable: PermitBundle, max_attempts: u32) -> Self {
        Self {
            durable,
            attempt: None,
            attempts: 0,
            path_switches: 0,
            max_attempts,
        }
    }

    fn reacquire(
        &mut self,
        pools: &ResourcePools,
        per_attempt: &[(ResourceKind, u64)],
    ) -> Result<(), ReservationError> {
        // Release first: if acquisition fails we hold nothing per-attempt
        // (durable holds are untouched) rather than a stale half-set.
        self.attempt = None;
        self.attempt = Some(pools.acquire(per_attempt, None)?);
        Ok(())
    }

    /// Begin (or retry) an attempt.
    pub fn begin_attempt(
        &mut self,
        pools: &ResourcePools,
        per_attempt: &[(ResourceKind, u64)],
    ) -> Result<(), AttemptError> {
        if self.attempts >= self.max_attempts {
            self.attempt = None;
            return Err(AttemptError::TooManyAttempts(self.attempts));
        }
        self.attempts += 1;
        self.reacquire(pools, per_attempt).map_err(Into::into)
    }

    /// §136: change path. Keeps durable holds; swaps per-attempt holds.
    /// Not counted as a retry (the transfer didn't fail).
    pub fn switch_path(
        &mut self,
        pools: &ResourcePools,
        new_per_attempt: &[(ResourceKind, u64)],
    ) -> Result<(), ReservationError> {
        self.path_switches += 1;
        self.reacquire(pools, new_per_attempt)
    }

    pub fn finish(self) {}

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn path_switches(&self) -> u32 {
        self.path_switches
    }

    pub fn durable_amount(&self, kind: ResourceKind) -> u64 {
        self.durable.amount(kind)
    }

    pub fn holds_attempt_permits(&self) -> bool {
        self.attempt.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ResourceKind::*;

    fn pools() -> ResourcePools {
        // exactly one attempt's worth of streams: a leak would show at once
        ResourcePools::new(&[(Storage, 1_000), (Streams, 1), (Memory, 64)])
    }

    fn holds(p: &ResourcePools) -> OperationHolds {
        OperationHolds::new(p.acquire(&[(Storage, 500)], None).unwrap(), 3)
    }

    #[test]
    fn retries_do_not_accumulate_permits() {
        let p = pools();
        let mut op = holds(&p);
        for _ in 0..3 {
            op.begin_attempt(&p, &[(Streams, 1), (Memory, 64)]).unwrap();
            assert_eq!(p.in_use(Streams), 1);
            assert_eq!(p.in_use(Memory), 64);
        }
        assert_eq!(op.attempts(), 3);
        assert_eq!(p.in_use(Storage), 500, "durable hold survives every retry");
    }

    #[test]
    fn attempt_budget_is_enforced_and_releases_attempt_permits() {
        let p = pools();
        let mut op = holds(&p);
        for _ in 0..3 {
            op.begin_attempt(&p, &[(Streams, 1)]).unwrap();
        }
        assert_eq!(
            op.begin_attempt(&p, &[(Streams, 1)]),
            Err(AttemptError::TooManyAttempts(3))
        );
        assert_eq!(p.in_use(Streams), 0);
        assert_eq!(p.in_use(Storage), 500);
    }

    #[test]
    fn path_switch_keeps_durable_holds_and_swaps_the_rest() {
        let p = ResourcePools::new(&[(Storage, 1_000), (Streams, 1), (Connections, 1)]);
        let mut op = OperationHolds::new(p.acquire(&[(Storage, 500)], None).unwrap(), 3);
        op.begin_attempt(&p, &[(Streams, 1)]).unwrap();
        op.switch_path(&p, &[(Connections, 1)]).unwrap();
        assert_eq!((p.in_use(Streams), p.in_use(Connections)), (0, 1));
        assert_eq!(p.in_use(Storage), 500);
        assert_eq!(
            (op.attempts(), op.path_switches()),
            (1, 1),
            "a switch is not a retry"
        );
    }

    #[test]
    fn failed_reacquire_leaves_no_stale_attempt_permits() {
        let p = pools();
        let mut op = holds(&p);
        op.begin_attempt(&p, &[(Streams, 1)]).unwrap();
        let _squatter_cannot_exist = ();
        // ask for more than the pool has
        let err = op.begin_attempt(&p, &[(Streams, 2)]).unwrap_err();
        assert!(matches!(err, AttemptError::Resource(_)));
        assert!(!op.holds_attempt_permits());
        assert_eq!(p.in_use(Streams), 0);
        assert_eq!(op.durable_amount(Storage), 500);
    }

    #[test]
    fn dropping_the_operation_releases_everything() {
        let p = pools();
        let mut op = holds(&p);
        op.begin_attempt(&p, &[(Streams, 1)]).unwrap();
        op.finish();
        assert_eq!((p.in_use(Storage), p.in_use(Streams)), (0, 0));
    }
}
