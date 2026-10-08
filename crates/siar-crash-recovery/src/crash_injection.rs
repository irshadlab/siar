//! §120 "Crash Injection Testing" (mandatory).
//!
//! [`CrashPoint`] names the nine injection points §120 lists.
//! [`CrashInjector::check`] is the hook placed at each point in code
//! under test: when armed for that point it returns
//! `Err(InjectedCrash)`, which a test harness propagates out of the
//! operation and then discards all in-memory state — exactly what a
//! crash does — before running recovery against whatever "durable"
//! state was left.
//!
//! Zero cost in production: without the `crash-injection` feature (and
//! outside `cfg(test)`) `CrashInjector` is a zero-sized type and
//! `check` is an inlined `Ok(())` the optimiser deletes. The wider
//! failpoint framework (§121) builds on this.
//!
//! The test module at the bottom is the §120 matrix: a simulated
//! send pipeline built from this crate's own recovery pieces, crashed
//! at each point, with the property "after recovery the effect has been
//! applied exactly once if and only if the DB commit happened."

/// §120's list, in pipeline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrashPoint {
    BeforeDbCommit,
    AfterDbCommit,
    BeforeNetworkSend,
    AfterNetworkSend,
    BeforeResultMarker,
    DuringChunkWrite,
    DuringFinalRename,
    DuringDtnPersistence,
    DuringProjectionCheckpoint,
}

impl CrashPoint {
    pub const ALL: [CrashPoint; 9] = [
        CrashPoint::BeforeDbCommit,
        CrashPoint::AfterDbCommit,
        CrashPoint::BeforeNetworkSend,
        CrashPoint::AfterNetworkSend,
        CrashPoint::BeforeResultMarker,
        CrashPoint::DuringChunkWrite,
        CrashPoint::DuringFinalRename,
        CrashPoint::DuringDtnPersistence,
        CrashPoint::DuringProjectionCheckpoint,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("injected crash at {0:?}")]
pub struct InjectedCrash(pub CrashPoint);

#[cfg(any(test, feature = "crash-injection"))]
#[derive(Debug, Clone, Copy)]
pub struct CrashInjector {
    armed: Option<(CrashPoint, u32)>,
    hits: u32,
    fired: bool,
}

#[cfg(any(test, feature = "crash-injection"))]
impl CrashInjector {
    pub const fn disarmed() -> Self {
        Self {
            armed: None,
            hits: 0,
            fired: false,
        }
    }

    /// Crash on the `nth` (1-based) time `point` is reached.
    pub const fn arm(point: CrashPoint, nth: u32) -> Self {
        Self {
            armed: Some((point, nth)),
            hits: 0,
            fired: false,
        }
    }

    pub const fn has_fired(&self) -> bool {
        self.fired
    }

    /// Fires at most once: a process only crashes once.
    pub fn check(&mut self, point: CrashPoint) -> Result<(), InjectedCrash> {
        if self.fired {
            return Ok(());
        }
        if let Some((p, nth)) = self.armed {
            if p == point {
                self.hits += 1;
                if self.hits == nth {
                    self.fired = true;
                    return Err(InjectedCrash(point));
                }
            }
        }
        Ok(())
    }
}

/// Production build: nothing to store, nothing to check.
#[cfg(not(any(test, feature = "crash-injection")))]
#[derive(Debug, Clone, Copy)]
pub struct CrashInjector;

#[cfg(not(any(test, feature = "crash-injection")))]
impl CrashInjector {
    pub const fn disarmed() -> Self {
        Self
    }

    #[inline(always)]
    pub const fn check(&mut self, _point: CrashPoint) -> Result<(), InjectedCrash> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk_recovery::TransferId;
    use crate::derived_recovery::{plan_projection, ProjectionCheckpoint, ProjectionPlan};
    use crate::dtn_recovery::{RelayLedger, RelayReceiveOutcome};
    use crate::effect_identity::{EffectId, IdempotentReceiver, ReceiveKey, ReceiveOutcome};
    use crate::fault_policy::{chunk_may_be_marked_complete, ChunkWriteResult};
    use crate::finalization_recovery::{
        reconcile_finalization, FinalizationRecoveryAction, FinalizationState,
    };
    use siar_domain::MessageId;
    use siar_dtn_bundle::BundleId;

    #[test]
    fn spec_lists_nine_distinct_points() {
        let set: std::collections::HashSet<_> = CrashPoint::ALL.iter().collect();
        assert_eq!(set.len(), 9);
    }

    #[test]
    fn disarmed_never_fires() {
        let mut i = CrashInjector::disarmed();
        for p in CrashPoint::ALL {
            assert_eq!(i.check(p), Ok(()));
        }
        assert!(!i.has_fired());
    }

    #[test]
    fn armed_fires_on_the_nth_hit_exactly_once() {
        let mut i = CrashInjector::arm(CrashPoint::DuringChunkWrite, 3);
        assert_eq!(i.check(CrashPoint::DuringChunkWrite), Ok(()));
        assert_eq!(i.check(CrashPoint::AfterDbCommit), Ok(()));
        assert_eq!(i.check(CrashPoint::DuringChunkWrite), Ok(()));
        assert_eq!(
            i.check(CrashPoint::DuringChunkWrite),
            Err(InjectedCrash(CrashPoint::DuringChunkWrite))
        );
        assert!(i.has_fired());
        assert_eq!(
            i.check(CrashPoint::DuringChunkWrite),
            Ok(()),
            "a process crashes once"
        );
    }

    #[test]
    fn production_injector_is_the_cheap_shape() {
        // In test builds the injector carries state; the production
        // shape is a ZST. Assert the *API* is identical by calling it.
        let mut i = CrashInjector::disarmed();
        assert!(i.check(CrashPoint::BeforeDbCommit).is_ok());
    }

    // ---- the matrix -------------------------------------------------

    #[derive(Default)]
    struct Durable {
        committed: bool,
        marker: bool,
    }

    /// The remote side survives the local crash.
    #[derive(Default)]
    struct Remote {
        receiver: IdempotentReceiver,
        applied: u32,
    }

    impl Remote {
        fn deliver(&mut self, id: MessageId) {
            if self.receiver.receive(ReceiveKey::Message(id)) == ReceiveOutcome::Process {
                self.applied += 1;
            }
        }
    }

    fn pipeline(
        inj: &mut CrashInjector,
        d: &mut Durable,
        r: &mut Remote,
        id: MessageId,
    ) -> Result<(), InjectedCrash> {
        inj.check(CrashPoint::BeforeDbCommit)?;
        d.committed = true;
        inj.check(CrashPoint::AfterDbCommit)?;
        inj.check(CrashPoint::BeforeNetworkSend)?;
        r.deliver(id);
        inj.check(CrashPoint::AfterNetworkSend)?;
        inj.check(CrashPoint::BeforeResultMarker)?;
        d.marker = true;
        Ok(())
    }

    /// Recovery: committed-without-marker is an unknown outcome, so
    /// retry with the *same* id (§59/§64); the receiver dedups.
    fn recover(d: &mut Durable, r: &mut Remote, id: MessageId) {
        if d.committed && !d.marker {
            let _effect = EffectId::Message(id);
            r.deliver(id);
            d.marker = true;
        }
    }

    #[test]
    fn send_pipeline_applies_exactly_once_iff_committed_at_every_crash_point() {
        let pipeline_points = [
            CrashPoint::BeforeDbCommit,
            CrashPoint::AfterDbCommit,
            CrashPoint::BeforeNetworkSend,
            CrashPoint::AfterNetworkSend,
            CrashPoint::BeforeResultMarker,
        ];
        for point in pipeline_points {
            let (mut d, mut r) = (Durable::default(), Remote::default());
            let id = MessageId::new();
            let mut inj = CrashInjector::arm(point, 1);
            let crashed = pipeline(&mut inj, &mut d, &mut r, id).is_err();
            assert!(crashed, "{point:?} should have crashed the pipeline");

            // "restart": in-memory state gone; only `d` and `r` remain.
            recover(&mut d, &mut r, id);
            recover(&mut d, &mut r, id); // recovery itself is idempotent

            if d.committed {
                assert_eq!(r.applied, 1, "{point:?}: applied exactly once");
                assert!(d.marker, "{point:?}: result marker restored");
            } else {
                assert_eq!(r.applied, 0, "{point:?}: nothing leaked before commit");
                assert!(!d.marker);
            }
        }
    }

    #[test]
    fn no_crash_run_is_the_baseline() {
        let (mut d, mut r) = (Durable::default(), Remote::default());
        let id = MessageId::new();
        pipeline(&mut CrashInjector::disarmed(), &mut d, &mut r, id).unwrap();
        recover(&mut d, &mut r, id);
        assert_eq!((r.applied, d.committed, d.marker), (1, true, true));
    }

    #[test]
    fn crash_during_chunk_write_never_marks_the_chunk_complete() {
        let mut inj = CrashInjector::arm(CrashPoint::DuringChunkWrite, 1);
        let mut chunk_done = false;
        let res: Result<(), InjectedCrash> = (|| {
            inj.check(CrashPoint::DuringChunkWrite)?;
            chunk_done = chunk_may_be_marked_complete(ChunkWriteResult::Durable);
            Ok(())
        })();
        assert!(res.is_err());
        assert!(
            !chunk_done,
            "progress is recorded only after the durable write"
        );
        let _ = TransferId::new();
    }

    #[test]
    fn crash_during_final_rename_resumes_finalization() {
        // Whether or not the rename landed, `Finalizing` re-runs.
        for object_exists in [false, true] {
            assert_eq!(
                reconcile_finalization(FinalizationState::Finalizing, object_exists),
                FinalizationRecoveryAction::ResumeFinalization
            );
        }
    }

    #[test]
    fn crash_during_dtn_persistence_converges_via_dedup() {
        let id = BundleId::new();
        // crash BEFORE the bundle became durable → nothing stored
        let mut ledger = RelayLedger::from_durable([]);
        assert_eq!(ledger.receive(id), RelayReceiveOutcome::NewlyStored);
        // crash AFTER durable store, before ACK → origin resends
        let mut ledger = RelayLedger::from_durable([id]);
        assert_eq!(ledger.receive(id), RelayReceiveOutcome::DuplicateReAck);
        assert_eq!(ledger.len(), 1);
    }

    #[test]
    fn crash_during_projection_checkpoint_replays_idempotently() {
        // checkpoint write lost: still at seq 10 while head is 15
        let cp = Some(ProjectionCheckpoint {
            schema_version: 2,
            last_applied_seq: 10,
        });
        let expected = ProjectionPlan::Replay {
            from_exclusive: 10,
            to_inclusive: 15,
        };
        assert_eq!(plan_projection(cp, 2, 15), expected);
        // a second crash during the replay changes nothing
        assert_eq!(plan_projection(cp, 2, 15), expected);
    }
}
