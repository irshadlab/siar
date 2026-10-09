//! §136 "Recovery Ownership", §137 "Recovery Context", §138
//! "Recovery Mode", §139 "Fast Startup vs Thorough Check".
//!
//! Each subsystem owns its reconciler ([`RecoverableSubsystem`]); the
//! [`RecoveryCoordinator`] only orders them. It runs subsystems phase
//! by phase in §109's order, enforced with `CleanupSequencer`, folds
//! their reports into one `RecoveryReport`, and stops — "do not
//! continue blindly" — the moment a subsystem reports `ReadOnly` or
//! worse. In `ReadOnly` mode the destructive cache/temp phase is
//! skipped outright.
//!
//! The coordinator uses `dyn` dispatch and boxes each subsystem's
//! future. That is a deliberate trade: recovery runs once per start,
//! subsystems are heterogeneous, and none of this is on a hot path.

use crate::cleanup_quarantine::{CleanupPhase, CleanupSequencer};
use crate::error_model::RecoveryFailure;
use crate::recovery_mode::{RecoverySeverity, SeverityAccumulator};
use crate::reporting::{RecoveryAction, RecoveryReport, RecoveryWarning, WarningCode};
use crate::timing_recovery::WallMillis;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

// ---------------------------------------------------------------------
// §137 / §138
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryMode {
    Normal,
    Thorough,
    ReadOnly,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryContext {
    pub generation: u64,
    pub unclean_shutdown: bool,
    pub mode: RecoveryMode,
}

// ---------------------------------------------------------------------
// §139
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartupFacts {
    pub previous_shutdown_clean: bool,
    /// Operator or policy forces a non-mutating start.
    pub force_read_only: bool,
    pub admin_repair_requested: bool,
}

/// §138: normal startup is lightweight; unclean shutdown may run
/// thorough. Precedence: read-only (safety) > explicit repair >
/// unclean → thorough > normal.
pub const fn startup_mode(f: StartupFacts) -> RecoveryMode {
    if f.force_read_only {
        RecoveryMode::ReadOnly
    } else if f.admin_repair_requested {
        RecoveryMode::Repair
    } else if !f.previous_shutdown_clean {
        RecoveryMode::Thorough
    } else {
        RecoveryMode::Normal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckDepth {
    /// Clean previous shutdown: cheap checks only.
    Fast,
    /// Unclean: targeted checks of what a crash could have touched.
    TargetedThorough,
    /// Periodic maintenance / explicit repair.
    Full,
}

/// Whether a full integrity pass is due (periodic maintenance, not
/// every launch).
pub const fn full_check_due(
    last_full: Option<WallMillis>,
    now: WallMillis,
    interval_ms: u64,
) -> bool {
    match last_full {
        None => true,
        Some(t) => now.0.saturating_sub(t.0) >= interval_ms,
    }
}

pub const fn check_depth(mode: RecoveryMode, full_check_due: bool) -> CheckDepth {
    match mode {
        RecoveryMode::Repair => CheckDepth::Full,
        RecoveryMode::Thorough => CheckDepth::TargetedThorough,
        // Read-only never mutates, so it stays cheap even when a full
        // check is due; that check waits for a writable start.
        RecoveryMode::ReadOnly => CheckDepth::Fast,
        RecoveryMode::Normal => {
            if full_check_due {
                CheckDepth::Full
            } else {
                CheckDepth::Fast
            }
        }
    }
}

// ---------------------------------------------------------------------
// §136
// ---------------------------------------------------------------------

/// Code-only error (no free text → no secret can leak into it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("subsystem recovery failed ({failure}, severity {severity:?})")]
pub struct RecoveryError {
    pub failure: RecoveryFailure,
    pub severity: RecoverySeverity,
}

impl RecoveryError {
    /// A failure at its default severity (§181).
    pub const fn of(failure: RecoveryFailure) -> Self {
        Self {
            failure,
            severity: failure.default_severity(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsystemRecoveryReport {
    pub severity: RecoverySeverity,
    pub actions: Vec<RecoveryAction>,
    pub warnings: Vec<RecoveryWarning>,
}

impl SubsystemRecoveryReport {
    pub fn clean() -> Self {
        Self {
            severity: RecoverySeverity::Clean,
            actions: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// The §136 trait. Native `async fn` in a trait: no macro, no
/// allocation of its own. The lint is allowed because recovery runs on
/// a single task; a `Send` flavour can be added without changing
/// implementors if a multi-threaded caller ever needs it.
#[allow(async_fn_in_trait)]
pub trait RecoverableSubsystem {
    fn name(&self) -> &'static str;
    /// Which §109 cleanup phase this subsystem belongs to.
    fn phase(&self) -> CleanupPhase;
    async fn recover(
        &self,
        ctx: &RecoveryContext,
    ) -> Result<SubsystemRecoveryReport, RecoveryError>;
}

type RecoverFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SubsystemRecoveryReport, RecoveryError>> + 'a>>;

trait ErasedSubsystem {
    fn name(&self) -> &'static str;
    fn phase(&self) -> CleanupPhase;
    fn recover<'a>(&'a self, ctx: &'a RecoveryContext) -> RecoverFuture<'a>;
}

impl<T: RecoverableSubsystem> ErasedSubsystem for T {
    fn name(&self) -> &'static str {
        RecoverableSubsystem::name(self)
    }
    fn phase(&self) -> CleanupPhase {
        RecoverableSubsystem::phase(self)
    }
    fn recover<'a>(&'a self, ctx: &'a RecoveryContext) -> RecoverFuture<'a> {
        Box::pin(RecoverableSubsystem::recover(self, ctx))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorOutcome {
    pub report: RecoveryReport,
    /// Phases fully completed, in order.
    pub phases_completed: usize,
    /// Set when recovery stopped early.
    pub halted_at: Option<(CleanupPhase, &'static str)>,
    /// Subsystem names in the order they actually ran.
    pub ran: Vec<&'static str>,
    /// Worst severity seen per phase, indexed like `CleanupPhase::ORDER`.
    /// `None` = no subsystem ran in that phase (none registered, the
    /// phase was skipped, or recovery halted before reaching it).
    pub phase_severity: [Option<RecoverySeverity>; 6],
}

#[derive(Default)]
pub struct RecoveryCoordinator {
    subsystems: Vec<Box<dyn ErasedSubsystem>>,
}

impl RecoveryCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<S: RecoverableSubsystem + 'static>(&mut self, s: S) {
        self.subsystems.push(Box::new(s));
    }

    pub fn len(&self) -> usize {
        self.subsystems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.subsystems.is_empty()
    }

    /// Runs every phase in §109 order. Within a phase, subsystems run
    /// in registration order.
    pub async fn run(&self, ctx: &RecoveryContext) -> CoordinatorOutcome {
        let mut seq = CleanupSequencer::new();
        let mut acc = SeverityAccumulator::new();
        let mut report = RecoveryReport::clean();
        let mut ran = Vec::new();
        let mut halted_at = None;
        let mut phases_completed = 0;
        let mut phase_severity: [Option<RecoverySeverity>; 6] = [None; 6];

        'phases: for phase in CleanupPhase::ORDER {
            // Destructive GC/temp cleanup never runs in read-only mode.
            let skip =
                phase == CleanupPhase::CacheAndTempCleanup && ctx.mode == RecoveryMode::ReadOnly;
            if !skip {
                let slot = phase as usize;
                for s in self.subsystems.iter().filter(|s| s.phase() == phase) {
                    ran.push(s.name());
                    match s.recover(ctx).await {
                        Ok(r) => {
                            acc.record(r.severity);
                            phase_severity[slot] = phase_severity[slot].max(Some(r.severity));
                            report.repaired_items.extend(r.actions);
                            report.warnings.extend(r.warnings);
                        }
                        Err(e) => {
                            acc.record(e.severity);
                            phase_severity[slot] = phase_severity[slot].max(Some(e.severity));
                            report.warnings.push(RecoveryWarning {
                                code: WarningCode::SubsystemRecoveryFailed,
                                count: 1,
                            });
                            if e.severity >= RecoverySeverity::ReadOnly {
                                halted_at = Some((phase, s.name()));
                                break 'phases;
                            }
                        }
                    }
                    if acc.severity() >= RecoverySeverity::ReadOnly {
                        halted_at = Some((phase, s.name()));
                        break 'phases;
                    }
                }
            }
            // An empty or skipped phase still counts as passed, so the
            // sequencer's order check stays meaningful.
            seq.complete(phase)
                .expect("phases are iterated in ORDER, so completion is always in order");
            phases_completed += 1;
        }
        report.severity = acc.severity();
        CoordinatorOutcome {
            report,
            phases_completed,
            halted_at,
            ran,
            phase_severity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reporting::RecoveryActionKind;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    type Log = Rc<RefCell<Vec<&'static str>>>;

    struct Stub {
        name: &'static str,
        phase: CleanupPhase,
        result: Result<SubsystemRecoveryReport, RecoveryError>,
        log: Log,
    }

    impl RecoverableSubsystem for Stub {
        fn name(&self) -> &'static str {
            self.name
        }
        fn phase(&self) -> CleanupPhase {
            self.phase
        }
        async fn recover(
            &self,
            _ctx: &RecoveryContext,
        ) -> Result<SubsystemRecoveryReport, RecoveryError> {
            self.log.borrow_mut().push(self.name);
            self.result.clone()
        }
    }

    fn ok(sev: RecoverySeverity) -> Result<SubsystemRecoveryReport, RecoveryError> {
        Ok(SubsystemRecoveryReport {
            severity: sev,
            actions: vec![RecoveryAction {
                kind: RecoveryActionKind::ProjectionRebuilt,
                count: 1,
            }],
            warnings: vec![],
        })
    }

    fn err(sev: RecoverySeverity) -> Result<SubsystemRecoveryReport, RecoveryError> {
        Err(RecoveryError {
            failure: RecoveryFailure::IntegrityFailure,
            severity: sev,
        })
    }

    fn stub(
        name: &'static str,
        phase: CleanupPhase,
        result: Result<SubsystemRecoveryReport, RecoveryError>,
        log: &Log,
    ) -> Stub {
        Stub {
            name,
            phase,
            result,
            log: log.clone(),
        }
    }

    fn ctx(mode: RecoveryMode) -> RecoveryContext {
        RecoveryContext {
            generation: 3,
            unclean_shutdown: true,
            mode,
        }
    }

    #[test]
    fn subsystems_run_in_phase_order_regardless_of_registration_order() {
        use CleanupPhase::*;
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        // deliberately registered backwards
        c.register(stub(
            "cache",
            CacheAndTempCleanup,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        c.register(stub("dtn", Dtn, ok(RecoverySeverity::Clean), &log));
        c.register(stub(
            "events",
            EventAndOutbox,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        c.register(stub(
            "identity",
            Identity,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        c.register(stub("db", DbIntegrity, ok(RecoverySeverity::Clean), &log));
        let out = block_on(c.run(&ctx(RecoveryMode::Thorough)));
        assert_eq!(
            *log.borrow(),
            vec!["db", "identity", "events", "dtn", "cache"]
        );
        assert_eq!(out.ran, *log.borrow());
        assert_eq!(out.phases_completed, 6);
        assert_eq!(out.halted_at, None);
    }

    #[test]
    fn reports_are_merged_and_the_worst_severity_wins() {
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        c.register(stub(
            "a",
            CleanupPhase::EventAndOutbox,
            ok(RecoverySeverity::Repaired),
            &log,
        ));
        c.register(stub(
            "b",
            CleanupPhase::Dtn,
            ok(RecoverySeverity::Degraded),
            &log,
        ));
        let out = block_on(c.run(&ctx(RecoveryMode::Normal)));
        assert_eq!(out.report.severity, RecoverySeverity::Degraded);
        assert_eq!(
            out.report.count_of(RecoveryActionKind::ProjectionRebuilt),
            2
        );
    }

    #[test]
    fn a_readonly_or_worse_failure_halts_later_phases() {
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        c.register(stub(
            "db",
            CleanupPhase::DbIntegrity,
            err(RecoverySeverity::Fatal),
            &log,
        ));
        c.register(stub(
            "events",
            CleanupPhase::EventAndOutbox,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        let out = block_on(c.run(&ctx(RecoveryMode::Thorough)));
        assert_eq!(*log.borrow(), vec!["db"], "no blind continuation");
        assert_eq!(out.halted_at, Some((CleanupPhase::DbIntegrity, "db")));
        assert_eq!(out.report.severity, RecoverySeverity::Fatal);
        assert_eq!(out.phases_completed, 0);
        assert!(out
            .report
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::SubsystemRecoveryFailed));
    }

    #[test]
    fn a_degraded_failure_does_not_stop_unrelated_subsystems() {
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        c.register(stub(
            "dtn",
            CleanupPhase::Dtn,
            err(RecoverySeverity::Degraded),
            &log,
        ));
        c.register(stub(
            "cache",
            CleanupPhase::CacheAndTempCleanup,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        let out = block_on(c.run(&ctx(RecoveryMode::Normal)));
        assert_eq!(*log.borrow(), vec!["dtn", "cache"]);
        assert_eq!(out.report.severity, RecoverySeverity::Degraded);
        assert_eq!(out.halted_at, None);
    }

    #[test]
    fn a_subsystem_reporting_readonly_via_ok_also_halts() {
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        c.register(stub(
            "id",
            CleanupPhase::Identity,
            ok(RecoverySeverity::ReadOnly),
            &log,
        ));
        c.register(stub(
            "events",
            CleanupPhase::EventAndOutbox,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        let out = block_on(c.run(&ctx(RecoveryMode::Normal)));
        assert_eq!(*log.borrow(), vec!["id"]);
        assert_eq!(out.halted_at, Some((CleanupPhase::Identity, "id")));
    }

    #[test]
    fn read_only_mode_skips_destructive_cleanup() {
        let log: Log = Rc::default();
        let mut c = RecoveryCoordinator::new();
        c.register(stub(
            "events",
            CleanupPhase::EventAndOutbox,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        c.register(stub(
            "cache",
            CleanupPhase::CacheAndTempCleanup,
            ok(RecoverySeverity::Clean),
            &log,
        ));
        let out = block_on(c.run(&ctx(RecoveryMode::ReadOnly)));
        assert_eq!(*log.borrow(), vec!["events"]);
        assert_eq!(
            out.phases_completed, 6,
            "skipped phase still counts as passed"
        );
    }

    #[test]
    fn empty_coordinator_is_a_clean_complete_run() {
        let c = RecoveryCoordinator::new();
        assert!(c.is_empty());
        let out = block_on(c.run(&ctx(RecoveryMode::Normal)));
        assert_eq!(out.report.severity, RecoverySeverity::Clean);
        assert_eq!(out.phases_completed, 6);
    }

    #[test]
    fn startup_mode_precedence() {
        let f = |clean, ro, repair| {
            startup_mode(StartupFacts {
                previous_shutdown_clean: clean,
                force_read_only: ro,
                admin_repair_requested: repair,
            })
        };
        assert_eq!(f(true, false, false), RecoveryMode::Normal);
        assert_eq!(f(false, false, false), RecoveryMode::Thorough);
        assert_eq!(f(true, false, true), RecoveryMode::Repair);
        assert_eq!(f(false, false, true), RecoveryMode::Repair);
        assert_eq!(f(false, true, true), RecoveryMode::ReadOnly);
    }

    #[test]
    fn check_depth_matches_the_three_part_strategy() {
        use RecoveryMode::*;
        assert_eq!(check_depth(Normal, false), CheckDepth::Fast);
        assert_eq!(check_depth(Thorough, false), CheckDepth::TargetedThorough);
        assert_eq!(
            check_depth(Normal, true),
            CheckDepth::Full,
            "periodic maintenance"
        );
        assert_eq!(check_depth(Repair, false), CheckDepth::Full);
        assert_eq!(check_depth(ReadOnly, true), CheckDepth::Fast);
    }

    #[test]
    fn full_check_is_periodic_not_every_launch() {
        let i = 7 * 24 * 3600 * 1000;
        assert!(full_check_due(None, WallMillis(1), i));
        assert!(!full_check_due(
            Some(WallMillis(1_000)),
            WallMillis(2_000),
            i
        ));
        assert!(full_check_due(
            Some(WallMillis(1_000)),
            WallMillis(1_000 + i),
            i
        ));
        assert!(
            !full_check_due(Some(WallMillis(9_000)), WallMillis(1_000), i),
            "clock skew never triggers"
        );
    }
}
