//! §87 "Recovery Metadata", §88 "Recovery Generation", §89 "Orphan
//! Resource Ownership".
//!
//! [`RecoveryMetadata`] is §87's struct. The generation (§88) is
//! bumped once per successful start and is what lets §89 tell a stale
//! orphan from a current operation's resources.

use crate::chunk_recovery::TransferId;
use crate::recovery_mode::RecoverySeverity;
use crate::shutdown_marker::RuntimeGeneration;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// §87. Timestamps are milliseconds since the Unix epoch, matching
/// this crate's other persisted times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryMetadata {
    pub last_clean_shutdown: Option<u64>,
    pub schema_version: u32,
    pub recovery_generation: u64,
    pub last_recovery_status: RecoverySeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("recovery generation counter exhausted")]
pub struct GenerationExhausted;

impl RecoveryMetadata {
    /// First-ever start.
    pub const fn fresh(schema_version: u32) -> Self {
        Self {
            last_clean_shutdown: None,
            schema_version,
            recovery_generation: 0,
            last_recovery_status: RecoverySeverity::Clean,
        }
    }

    pub const fn generation(&self) -> RuntimeGeneration {
        RuntimeGeneration(self.recovery_generation)
    }

    /// Did the previous run end without a clean-shutdown record?
    /// (Combine with the §6 marker; this is the metadata-side view.)
    pub const fn last_start_was_unclean(&self, last_start_millis: u64) -> bool {
        match self.last_clean_shutdown {
            None => true,
            Some(t) => t < last_start_millis,
        }
    }

    /// §88: increment on every successful recovery/start and record
    /// its outcome. Fails (rather than wrapping) at `u64::MAX`, since a
    /// wrapped generation would make stale resources look current.
    pub fn record_successful_start(
        &mut self,
        status: RecoverySeverity,
    ) -> Result<RuntimeGeneration, GenerationExhausted> {
        let next = self
            .recovery_generation
            .checked_add(1)
            .ok_or(GenerationExhausted)?;
        self.recovery_generation = next;
        self.last_recovery_status = status;
        Ok(RuntimeGeneration(next))
    }

    pub fn record_clean_shutdown(&mut self, now_millis: u64) {
        self.last_clean_shutdown = Some(now_millis);
    }
}

// ---------------------------------------------------------------------
// §89
// ---------------------------------------------------------------------

/// §89's two stamp fields on every temp/staging record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipStamp {
    pub created_generation: RuntimeGeneration,
    pub operation_id: Option<TransferId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerStatus {
    /// Its operation is live (possibly resumed from an earlier
    /// generation).
    CurrentOperation,
    /// Created by an earlier generation and its operation is not
    /// active: a stale orphan, safe for §24 cleanup.
    StaleOrphan,
    /// Created in *this* generation but no active operation claims it
    /// (yet, or any more). Not stale — leave it.
    CurrentGenerationUnclaimed,
    /// Stamped with a generation newer than ours (database restored,
    /// or a newer binary ran): can't judge, never delete.
    FromFuture,
}

pub fn classify_ownership(
    stamp: &OwnershipStamp,
    current: RuntimeGeneration,
    active_ops: &HashSet<TransferId>,
) -> OwnerStatus {
    let active = stamp.operation_id.is_some_and(|o| active_ops.contains(&o));
    if stamp.created_generation > current {
        OwnerStatus::FromFuture
    } else if active {
        OwnerStatus::CurrentOperation
    } else if stamp.created_generation == current {
        OwnerStatus::CurrentGenerationUnclaimed
    } else {
        OwnerStatus::StaleOrphan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_metadata_starts_at_generation_zero_and_clean() {
        let m = RecoveryMetadata::fresh(4);
        assert_eq!(m.recovery_generation, 0);
        assert_eq!(m.last_recovery_status, RecoverySeverity::Clean);
        assert_eq!(m.last_clean_shutdown, None);
    }

    #[test]
    fn each_successful_start_increments_and_records_status() {
        let mut m = RecoveryMetadata::fresh(4);
        assert_eq!(
            m.record_successful_start(RecoverySeverity::Repaired),
            Ok(RuntimeGeneration(1))
        );
        assert_eq!(
            m.record_successful_start(RecoverySeverity::Clean),
            Ok(RuntimeGeneration(2))
        );
        assert_eq!(m.generation(), RuntimeGeneration(2));
        assert_eq!(m.last_recovery_status, RecoverySeverity::Clean);
    }

    #[test]
    fn generation_never_wraps() {
        let mut m = RecoveryMetadata::fresh(1);
        m.recovery_generation = u64::MAX;
        assert_eq!(
            m.record_successful_start(RecoverySeverity::Clean),
            Err(GenerationExhausted)
        );
        assert_eq!(m.recovery_generation, u64::MAX);
    }

    #[test]
    fn unclean_start_detection() {
        let mut m = RecoveryMetadata::fresh(1);
        assert!(m.last_start_was_unclean(100));
        m.record_clean_shutdown(50);
        assert!(m.last_start_was_unclean(100)); // shutdown predates that start
        m.record_clean_shutdown(150);
        assert!(!m.last_start_was_unclean(100));
    }

    #[test]
    fn metadata_and_stamp_are_persistable() {
        fn persistable<T: Serialize + serde::de::DeserializeOwned>() {}
        persistable::<RecoveryMetadata>();
        persistable::<OwnershipStamp>();
    }

    fn stamp(g: u64, op: Option<TransferId>) -> OwnershipStamp {
        OwnershipStamp {
            created_generation: RuntimeGeneration(g),
            operation_id: op,
        }
    }

    #[test]
    fn ownership_distinguishes_stale_from_current() {
        let live = TransferId::new();
        let active: HashSet<_> = [live].into();
        let cur = RuntimeGeneration(5);
        assert_eq!(
            classify_ownership(&stamp(3, Some(live)), cur, &active),
            OwnerStatus::CurrentOperation
        );
        assert_eq!(
            classify_ownership(&stamp(5, Some(live)), cur, &active),
            OwnerStatus::CurrentOperation
        );
        assert_eq!(
            classify_ownership(&stamp(3, Some(TransferId::new())), cur, &active),
            OwnerStatus::StaleOrphan
        );
        assert_eq!(
            classify_ownership(&stamp(3, None), cur, &active),
            OwnerStatus::StaleOrphan
        );
        assert_eq!(
            classify_ownership(&stamp(5, None), cur, &active),
            OwnerStatus::CurrentGenerationUnclaimed
        );
    }

    #[test]
    fn future_generation_is_never_judged_stale() {
        let active = HashSet::new();
        assert_eq!(
            classify_ownership(&stamp(9, None), RuntimeGeneration(5), &active),
            OwnerStatus::FromFuture
        );
    }
}
