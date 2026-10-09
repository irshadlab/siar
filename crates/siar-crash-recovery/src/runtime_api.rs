//! §160 "Recovery API Boundary", §161 "Recovery Report API", §162
//! "Health State After Recovery", §163 "Admission After Recovery",
//! §164 "Network Start Barrier", §165 "Listener Startup Order".
//!
//! The application's whole interface to recovery is
//! `runtime.recover().await?` — it never orchestrates subsystem
//! repair itself. [`Runtime`] wraps the coordinator, keeps the
//! resulting report and health, and publishes an [`AdmissionGate`]
//! (lock-free, shareable across tasks) that decides what may run
//! before, during and after recovery. [`ListenerBarrier`] keeps
//! inbound transport listeners down until identity, the critical
//! store and dedup state are ready.

use crate::coordinator::{CoordinatorOutcome, RecoveryContext, RecoveryCoordinator};
use crate::recovery_mode::{gate, Operation, OperationDenied, RecoverySeverity};
use crate::reporting::RecoveryReport;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------
// §162
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthState {
    Healthy,
    Degraded,
    Unavailable,
}

/// §162, verbatim fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHealth {
    pub recovery: RecoverySeverity,
    pub storage: HealthState,
    pub identity: HealthState,
    pub messaging: HealthState,
    pub files: HealthState,
    pub dtn: HealthState,
}

impl RuntimeHealth {
    /// Maps per-phase recovery results to components:
    /// DbIntegrity→storage, Identity→identity, EventAndOutbox→
    /// messaging, TransferStaging→files, Dtn→dtn. A phase that never
    /// ran *because recovery halted earlier* is `Unavailable` (it was
    /// not verified); a phase with no registered subsystem and no
    /// halt is `Healthy`.
    pub fn from_outcome(o: &CoordinatorOutcome) -> Self {
        let halted = o.halted_at.is_some();
        let h = |idx: usize| match o.phase_severity[idx] {
            None => {
                if halted && idx >= o.phases_completed {
                    HealthState::Unavailable
                } else {
                    HealthState::Healthy
                }
            }
            Some(RecoverySeverity::Clean | RecoverySeverity::Repaired) => HealthState::Healthy,
            Some(RecoverySeverity::Degraded) => HealthState::Degraded,
            Some(RecoverySeverity::ReadOnly | RecoverySeverity::Fatal) => HealthState::Unavailable,
        };
        Self {
            recovery: o.report.severity,
            storage: h(0),
            identity: h(1),
            messaging: h(2),
            files: h(3),
            dtn: h(4),
        }
    }
}

// ---------------------------------------------------------------------
// §163
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    NotStarted,
    Recovering,
    Ready(RecoverySeverity),
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Allow,
    /// Not yet — queue or retry after recovery completes.
    Defer,
    Reject(OperationDenied),
}

const S_NOT_STARTED: u8 = 0;
const S_RECOVERING: u8 = 1;
const S_FAILED: u8 = 2;
const S_READY_BASE: u8 = 16;

const fn sev_to_u8(s: RecoverySeverity) -> u8 {
    match s {
        RecoverySeverity::Clean => 0,
        RecoverySeverity::Repaired => 1,
        RecoverySeverity::Degraded => 2,
        RecoverySeverity::ReadOnly => 3,
        RecoverySeverity::Fatal => 4,
    }
}

const fn sev_from_u8(v: u8) -> RecoverySeverity {
    match v {
        0 => RecoverySeverity::Clean,
        1 => RecoverySeverity::Repaired,
        2 => RecoverySeverity::Degraded,
        3 => RecoverySeverity::ReadOnly,
        _ => RecoverySeverity::Fatal,
    }
}

/// Lock-free recovery state shared between the recovering task and
/// every caller that wants to know whether an operation may run.
#[derive(Debug)]
pub struct AdmissionGate {
    state: AtomicU8,
}

impl AdmissionGate {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(S_NOT_STARTED),
        }
    }

    pub fn state(&self) -> RuntimeState {
        match self.state.load(Ordering::Acquire) {
            S_NOT_STARTED => RuntimeState::NotStarted,
            S_RECOVERING => RuntimeState::Recovering,
            S_FAILED => RuntimeState::Failed,
            v => RuntimeState::Ready(sev_from_u8(v - S_READY_BASE)),
        }
    }

    fn set(&self, v: u8) {
        self.state.store(v, Ordering::Release);
    }

    /// §163: before recovery completes, new durable operations and
    /// network starts are *deferred*; read-only queries may be allowed
    /// earlier when the caller says that is safe
    /// (`reads_allowed_early`). Diagnostics are always allowed.
    pub fn admit(&self, op: Operation, reads_allowed_early: bool) -> Admission {
        match self.state() {
            RuntimeState::NotStarted | RuntimeState::Recovering => match op {
                Operation::Diagnostics => Admission::Allow,
                Operation::ViewMessages | Operation::InspectDevices | Operation::ExportData => {
                    if reads_allowed_early {
                        Admission::Allow
                    } else {
                        Admission::Defer
                    }
                }
                Operation::DurableWrite | Operation::StartNetwork => Admission::Defer,
            },
            RuntimeState::Ready(sev) => match gate(sev, op) {
                Ok(()) => Admission::Allow,
                Err(e) => Admission::Reject(e),
            },
            RuntimeState::Failed => match op {
                Operation::Diagnostics => Admission::Allow,
                _ => Admission::Reject(OperationDenied {
                    op,
                    severity: RecoverySeverity::Fatal,
                }),
            },
        }
    }
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// §160 / §161
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RecoverError {
    #[error("recovery already started")]
    AlreadyStarted,
    /// Recovery ended in `Fatal`. The report and health are still
    /// available for display and admin tooling.
    #[error("recovery failed fatally")]
    Fatal,
}

pub struct Runtime {
    coordinator: RecoveryCoordinator,
    gate: Arc<AdmissionGate>,
    outcome: Option<CoordinatorOutcome>,
    health: Option<RuntimeHealth>,
}

impl Runtime {
    pub fn new(coordinator: RecoveryCoordinator) -> Self {
        Self {
            coordinator,
            gate: Arc::new(AdmissionGate::new()),
            outcome: None,
            health: None,
        }
    }

    /// A handle other tasks use to ask "may I do this now?".
    pub fn admission_gate(&self) -> Arc<AdmissionGate> {
        Arc::clone(&self.gate)
    }

    /// §160: the one call the application makes. Returns the final
    /// severity; `Err(Fatal)` only when recovery cannot continue.
    pub async fn recover(
        &mut self,
        ctx: &RecoveryContext,
    ) -> Result<RecoverySeverity, RecoverError> {
        if self.gate.state() != RuntimeState::NotStarted {
            return Err(RecoverError::AlreadyStarted);
        }
        self.gate.set(S_RECOVERING);
        let outcome = self.coordinator.run(ctx).await;
        let sev = outcome.report.severity;
        self.health = Some(RuntimeHealth::from_outcome(&outcome));
        self.outcome = Some(outcome);
        if sev == RecoverySeverity::Fatal {
            self.gate.set(S_FAILED);
            Err(RecoverError::Fatal)
        } else {
            self.gate.set(S_READY_BASE + sev_to_u8(sev));
            Ok(sev)
        }
    }

    /// §161: for UI and admin tools.
    pub fn recovery_report(&self) -> Option<&RecoveryReport> {
        self.outcome.as_ref().map(|o| &o.report)
    }

    pub fn outcome(&self) -> Option<&CoordinatorOutcome> {
        self.outcome.as_ref()
    }

    pub fn health(&self) -> Option<&RuntimeHealth> {
        self.health.as_ref()
    }

    pub fn state(&self) -> RuntimeState {
        self.gate.state()
    }
}

// ---------------------------------------------------------------------
// §164 / §165
// ---------------------------------------------------------------------

/// §165's recommended order, as stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ListenerStage {
    IdentityReady = 0,
    EventStoreReady = 1,
    DedupOutboxReady = 2,
    DtnStoreReady = 3,
}

impl ListenerStage {
    pub const ORDER: [ListenerStage; 4] = [
        ListenerStage::IdentityReady,
        ListenerStage::EventStoreReady,
        ListenerStage::DedupOutboxReady,
        ListenerStage::DtnStoreReady,
    ];

    const fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// §164: inbound listeners wait for identity, the critical store and
/// dedup state — otherwise duplicate/security handling can be wrong.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListenerBarrier {
    ready: u8,
}

impl ListenerBarrier {
    pub const fn new() -> Self {
        Self { ready: 0 }
    }

    pub fn mark_ready(&mut self, s: ListenerStage) {
        self.ready |= s.bit();
    }

    pub const fn is_ready(&self, s: ListenerStage) -> bool {
        self.ready & s.bit() != 0
    }

    const REQUIRED: u8 = ListenerStage::IdentityReady.bit()
        | ListenerStage::EventStoreReady.bit()
        | ListenerStage::DedupOutboxReady.bit();

    /// May transport listeners start? Always needs the §164 three;
    /// a listener that can receive DTN bundles also needs the DTN
    /// store (§165's full order).
    pub const fn may_start_listeners(&self, accepts_dtn: bool) -> bool {
        let need = if accepts_dtn {
            Self::REQUIRED | ListenerStage::DtnStoreReady.bit()
        } else {
            Self::REQUIRED
        };
        self.ready & need == need
    }

    /// What is still holding listeners back, in §165 order.
    pub fn blocking(&self, accepts_dtn: bool) -> Vec<ListenerStage> {
        ListenerStage::ORDER
            .iter()
            .copied()
            .filter(|s| accepts_dtn || *s != ListenerStage::DtnStoreReady)
            .filter(|s| !self.is_ready(*s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup_quarantine::CleanupPhase;
    use crate::coordinator::{
        RecoverableSubsystem, RecoveryError, RecoveryMode, SubsystemRecoveryReport,
    };
    use crate::error_model::RecoveryFailure;
    use std::future::Future;
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

    struct Stub {
        phase: CleanupPhase,
        result: Result<RecoverySeverity, RecoverySeverity>,
    }

    impl RecoverableSubsystem for Stub {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn phase(&self) -> CleanupPhase {
            self.phase
        }
        async fn recover(
            &self,
            _ctx: &RecoveryContext,
        ) -> Result<SubsystemRecoveryReport, RecoveryError> {
            match self.result {
                Ok(severity) => Ok(SubsystemRecoveryReport {
                    severity,
                    actions: vec![],
                    warnings: vec![],
                }),
                Err(severity) => Err(RecoveryError {
                    failure: RecoveryFailure::IntegrityFailure,
                    severity,
                }),
            }
        }
    }

    fn runtime(stubs: Vec<Stub>) -> Runtime {
        let mut c = RecoveryCoordinator::new();
        for s in stubs {
            c.register(s);
        }
        Runtime::new(c)
    }

    fn ctx() -> RecoveryContext {
        RecoveryContext {
            generation: 1,
            unclean_shutdown: true,
            mode: RecoveryMode::Thorough,
        }
    }

    fn st(phase: CleanupPhase, sev: RecoverySeverity) -> Stub {
        Stub {
            phase,
            result: Ok(sev),
        }
    }

    #[test]
    fn one_call_recovers_and_unlocks_normal_operation() {
        let mut rt = runtime(vec![
            st(CleanupPhase::DbIntegrity, RecoverySeverity::Clean),
            st(CleanupPhase::EventAndOutbox, RecoverySeverity::Repaired),
        ]);
        assert_eq!(rt.state(), RuntimeState::NotStarted);
        let sev = block_on(rt.recover(&ctx())).unwrap();
        assert_eq!(sev, RecoverySeverity::Repaired);
        assert_eq!(rt.state(), RuntimeState::Ready(RecoverySeverity::Repaired));
        let gate = rt.admission_gate();
        assert_eq!(gate.admit(Operation::DurableWrite, false), Admission::Allow);
        assert_eq!(gate.admit(Operation::StartNetwork, false), Admission::Allow);
    }

    #[test]
    fn report_and_health_are_available_after_recovery() {
        let mut rt = runtime(vec![st(CleanupPhase::Dtn, RecoverySeverity::Degraded)]);
        assert!(rt.recovery_report().is_none() && rt.health().is_none());
        block_on(rt.recover(&ctx())).unwrap();
        assert_eq!(
            rt.recovery_report().unwrap().severity,
            RecoverySeverity::Degraded
        );
        let h = rt.health().unwrap();
        assert_eq!(h.dtn, HealthState::Degraded);
        assert_eq!(
            (h.storage, h.identity, h.messaging, h.files),
            (
                HealthState::Healthy,
                HealthState::Healthy,
                HealthState::Healthy,
                HealthState::Healthy
            )
        );
        assert_eq!(h.recovery, RecoverySeverity::Degraded);
    }

    #[test]
    fn new_durable_work_is_deferred_until_recovery_completes() {
        let rt = runtime(vec![]);
        let g = rt.admission_gate();
        for op in [Operation::DurableWrite, Operation::StartNetwork] {
            assert_eq!(g.admit(op, true), Admission::Defer, "{op:?}");
        }
        assert_eq!(g.admit(Operation::Diagnostics, false), Admission::Allow);
    }

    #[test]
    fn reads_are_allowed_early_only_when_the_caller_says_it_is_safe() {
        let g = AdmissionGate::new();
        assert_eq!(g.admit(Operation::ViewMessages, false), Admission::Defer);
        assert_eq!(g.admit(Operation::ViewMessages, true), Admission::Allow);
    }

    #[test]
    fn read_only_recovery_rejects_writes_but_serves_reads() {
        let mut rt = runtime(vec![st(CleanupPhase::Identity, RecoverySeverity::ReadOnly)]);
        assert_eq!(block_on(rt.recover(&ctx())), Ok(RecoverySeverity::ReadOnly));
        let g = rt.admission_gate();
        assert_eq!(g.admit(Operation::ViewMessages, false), Admission::Allow);
        assert!(matches!(
            g.admit(Operation::DurableWrite, false),
            Admission::Reject(_)
        ));
        assert!(matches!(
            g.admit(Operation::StartNetwork, false),
            Admission::Reject(_)
        ));
    }

    #[test]
    fn fatal_recovery_errors_but_report_and_diagnostics_survive() {
        let mut rt = runtime(vec![Stub {
            phase: CleanupPhase::DbIntegrity,
            result: Err(RecoverySeverity::Fatal),
        }]);
        assert_eq!(block_on(rt.recover(&ctx())), Err(RecoverError::Fatal));
        assert_eq!(rt.state(), RuntimeState::Failed);
        assert!(
            rt.recovery_report().is_some(),
            "admin tooling still gets the report"
        );
        let g = rt.admission_gate();
        assert_eq!(g.admit(Operation::Diagnostics, false), Admission::Allow);
        assert!(matches!(
            g.admit(Operation::ViewMessages, true),
            Admission::Reject(_)
        ));
        assert!(matches!(
            g.admit(Operation::DurableWrite, false),
            Admission::Reject(_)
        ));
    }

    #[test]
    fn recover_cannot_be_called_twice() {
        let mut rt = runtime(vec![]);
        block_on(rt.recover(&ctx())).unwrap();
        assert_eq!(
            block_on(rt.recover(&ctx())),
            Err(RecoverError::AlreadyStarted)
        );
    }

    #[test]
    fn components_after_a_halt_are_unavailable_not_assumed_healthy() {
        let mut rt = runtime(vec![
            st(CleanupPhase::DbIntegrity, RecoverySeverity::Clean),
            Stub {
                phase: CleanupPhase::Identity,
                result: Err(RecoverySeverity::ReadOnly),
            },
            st(CleanupPhase::EventAndOutbox, RecoverySeverity::Clean),
        ]);
        block_on(rt.recover(&ctx())).unwrap();
        let h = rt.health().unwrap();
        assert_eq!(h.storage, HealthState::Healthy);
        assert_eq!(h.identity, HealthState::Unavailable);
        assert_eq!(h.messaging, HealthState::Unavailable, "never verified");
        assert_eq!(h.files, HealthState::Unavailable);
        assert_eq!(h.dtn, HealthState::Unavailable);
    }

    #[test]
    fn the_gate_is_visible_to_other_threads_while_recovering() {
        let g = Arc::new(AdmissionGate::new());
        let g2 = Arc::clone(&g);
        assert_eq!(g.admit(Operation::DurableWrite, false), Admission::Defer);
        g.set(S_READY_BASE + sev_to_u8(RecoverySeverity::Clean));
        let seen = std::thread::spawn(move || g2.admit(Operation::DurableWrite, false))
            .join()
            .unwrap();
        assert_eq!(seen, Admission::Allow);
    }

    #[test]
    fn severity_encoding_round_trips() {
        for s in [
            RecoverySeverity::Clean,
            RecoverySeverity::Repaired,
            RecoverySeverity::Degraded,
            RecoverySeverity::ReadOnly,
            RecoverySeverity::Fatal,
        ] {
            assert_eq!(sev_from_u8(sev_to_u8(s)), s);
            let g = AdmissionGate::new();
            g.set(S_READY_BASE + sev_to_u8(s));
            assert_eq!(g.state(), RuntimeState::Ready(s));
        }
    }

    #[test]
    fn listeners_wait_for_identity_store_and_dedup() {
        use ListenerStage::*;
        let mut b = ListenerBarrier::new();
        assert!(!b.may_start_listeners(false));
        b.mark_ready(IdentityReady);
        b.mark_ready(EventStoreReady);
        assert!(!b.may_start_listeners(false), "dedup/outbox not ready");
        assert_eq!(b.blocking(false), vec![DedupOutboxReady]);
        b.mark_ready(DedupOutboxReady);
        assert!(b.may_start_listeners(false));
    }

    #[test]
    fn dtn_accepting_listeners_also_wait_for_the_dtn_store() {
        use ListenerStage::*;
        let mut b = ListenerBarrier::new();
        for s in [IdentityReady, EventStoreReady, DedupOutboxReady] {
            b.mark_ready(s);
        }
        assert!(b.may_start_listeners(false));
        assert!(!b.may_start_listeners(true));
        assert_eq!(b.blocking(true), vec![DtnStoreReady]);
        b.mark_ready(DtnStoreReady);
        assert!(b.may_start_listeners(true));
        assert!(b.blocking(true).is_empty());
    }

    #[test]
    fn readiness_order_of_marking_does_not_matter_only_the_set() {
        use ListenerStage::*;
        let mut b = ListenerBarrier::new();
        for s in [DedupOutboxReady, IdentityReady, EventStoreReady] {
            b.mark_ready(s);
        }
        assert!(b.may_start_listeners(false));
    }

    #[test]
    fn stage_order_matches_the_spec() {
        assert_eq!(
            ListenerStage::ORDER,
            [
                ListenerStage::IdentityReady,
                ListenerStage::EventStoreReady,
                ListenerStage::DedupOutboxReady,
                ListenerStage::DtnStoreReady
            ]
        );
    }
}
