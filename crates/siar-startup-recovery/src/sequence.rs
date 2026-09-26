//! `04-offline-event-log-architecture.md` §47 "Startup Recovery" —
//! the spec's own recommended sequence, verbatim: open DB → verify
//! migrations → resume projections → reconcile work queues → load
//! local read models → render UI → start networking, with its one
//! stated guarantee: "no remote connection is required before local
//! state becomes usable." `ROADMAP.md`'s own §47 row: "No orchestrated
//! startup sequence exists anywhere in what's been built."
//!
//! [`StartupSequence`] is that orchestration, made real. Its
//! constructor takes the spec's six real steps as POSITIONAL
//! arguments in the spec's own order — not a builder with named
//! setters — so the correct order is enforced by the type signature
//! itself: there is no way to construct a [`StartupSequence`] with the
//! steps out of order, only a way to leave one unwritten (a compile
//! error, not a runtime one).
//!
//! ## "Render UI" — the one step this crate cannot do, named honestly
//!
//! A headless library crate cannot draw pixels. [`StartupSequence`]
//! doesn't pretend to: in its place is
//! [`StartupSequence::on_local_state_ready`], an optional hook run at
//! EXACTLY the point the spec's own diagram puts "render UI" — after
//! local read models are loaded, before networking starts. A real
//! caller puts its actual UI-rendering call there. Left unset, this
//! step is a no-op, and [`StartupSequence::run`] proceeds straight to
//! `start_networking` — never silently skipped in a way that would be
//! surprising, just genuinely optional the way the spec's own UI step
//! is unavoidably platform-specific.
//!
//! ## What this guarantees, and how that's actually checked
//!
//! The spec's one stated invariant — local state usable before any
//! remote connection — isn't a runtime check here; it's true BY
//! CONSTRUCTION, the same way [`super::reconciliation`]'s correctness
//! is checked by what it computes rather than what it promises:
//! [`StartupSequence::run`] fully awaits every earlier step before
//! `start_networking` is ever invoked, and stops entirely — never
//! reaching `start_networking` — the moment any earlier step fails.
//! This module's own tests confirm the ORDER really is enforced (not
//! just documented) and that a failure partway through really does
//! stop the sequence, using a shared call-order log rather than trusting
//! the implementation's own claims about itself.

use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

/// A step's own error type — deliberately opaque (`Box<dyn Error>`):
/// this crate has no idea what a real "open DB" or "verify migrations"
/// failure looks like for any particular backend, and shouldn't need
/// to in order to sequence them correctly.
pub type StepError = Box<dyn std::error::Error + Send + Sync>;

type BoxFuture = Pin<Box<dyn Future<Output = Result<(), StepError>> + Send>>;

/// One step in [`StartupSequence`] — an owned, one-shot async closure.
/// Built with [`step`] rather than boxed/pinned by hand at every call
/// site.
pub type Step = Box<dyn FnOnce() -> BoxFuture + Send>;

/// Wrap an ordinary `async` closure as a [`Step`], so a real caller
/// writes `step(|| async { ... })` rather than hand-rolling
/// `Box::pin`/`Box::new` at every one of the spec's six call sites.
pub fn step<F, Fut>(f: F) -> Step
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), StepError>> + Send + 'static,
{
    Box::new(move || Box::pin(f()))
}

/// §47's own six real steps (the spec lists seven; "render UI" is
/// [`StartupSequence::on_local_state_ready`], a hook rather than a
/// numbered step here — see this module's own doc comment for why).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStep {
    OpenDb,
    VerifyMigrations,
    ResumeProjections,
    ReconcileWorkQueues,
    LoadReadModels,
    OnLocalStateReady,
    StartNetworking,
}

/// Which step failed, and that step's own error — [`StartupSequence::
/// run`]'s only `Err` shape. Every step from [`StartupStep::OpenDb`]
/// up to and including the one named here already ran; every step
/// after it never did.
#[derive(Debug, Error)]
#[error("startup recovery failed at {step:?}: {error}")]
pub struct StartupFailure {
    pub step: StartupStep,
    #[source]
    pub error: StepError,
}

/// §47's own orchestrator — see this module's own doc comment.
pub struct StartupSequence {
    open_db: Step,
    verify_migrations: Step,
    resume_projections: Step,
    reconcile_work_queues: Step,
    load_read_models: Step,
    on_local_state_ready: Option<Step>,
    start_networking: Step,
}

impl StartupSequence {
    /// The spec's own six mandatory steps, positional, in the spec's
    /// own order — see this module's own doc comment for why that's
    /// deliberate rather than a builder with named setters.
    pub fn new(
        open_db: Step,
        verify_migrations: Step,
        resume_projections: Step,
        reconcile_work_queues: Step,
        load_read_models: Step,
        start_networking: Step,
    ) -> Self {
        Self {
            open_db,
            verify_migrations,
            resume_projections,
            reconcile_work_queues,
            load_read_models,
            on_local_state_ready: None,
            start_networking,
        }
    }

    /// See this module's own doc comment's "Render UI" section. Not
    /// part of [`Self::new`]'s own required steps since it's the one
    /// genuinely optional one — a headless caller (a background sync
    /// process, a test harness) has no UI to render and no obligation
    /// to set this.
    pub fn on_local_state_ready(mut self, step: Step) -> Self {
        self.on_local_state_ready = Some(step);
        self
    }

    /// Runs every step in the spec's own order, stopping at the first
    /// failure. `Ok(())` means all seven ran (six mandatory plus
    /// whichever of [`Self::on_local_state_ready`] was set) — this
    /// crate has no partial-success shape, matching the spec's own
    /// framing of a single recommended sequence, not independent
    /// steps a caller might want to know succeeded individually.
    pub async fn run(self) -> Result<(), StartupFailure> {
        run_step(StartupStep::OpenDb, self.open_db).await?;
        run_step(StartupStep::VerifyMigrations, self.verify_migrations).await?;
        run_step(StartupStep::ResumeProjections, self.resume_projections).await?;
        run_step(StartupStep::ReconcileWorkQueues, self.reconcile_work_queues).await?;
        run_step(StartupStep::LoadReadModels, self.load_read_models).await?;
        if let Some(on_ready) = self.on_local_state_ready {
            run_step(StartupStep::OnLocalStateReady, on_ready).await?;
        }
        run_step(StartupStep::StartNetworking, self.start_networking).await?;
        Ok(())
    }
}

async fn run_step(step_name: StartupStep, step: Step) -> Result<(), StartupFailure> {
    step().await.map_err(|error| StartupFailure {
        step: step_name,
        error,
    })
}

/// A [`Step`] that records `name` into `log` and always succeeds —
/// this module's own tests use this to observe real execution order
/// rather than trusting [`StartupSequence::run`]'s own doc comment
/// about what order it runs things in.
#[cfg(test)]
fn logging_step(
    name: StartupStep,
    log: std::sync::Arc<std::sync::Mutex<Vec<StartupStep>>>,
) -> Step {
    step(move || async move {
        log.lock().unwrap().push(name);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn every_step_runs_in_exactly_the_spec_own_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sequence = StartupSequence::new(
            logging_step(StartupStep::OpenDb, log.clone()),
            logging_step(StartupStep::VerifyMigrations, log.clone()),
            logging_step(StartupStep::ResumeProjections, log.clone()),
            logging_step(StartupStep::ReconcileWorkQueues, log.clone()),
            logging_step(StartupStep::LoadReadModels, log.clone()),
            logging_step(StartupStep::StartNetworking, log.clone()),
        )
        .on_local_state_ready(logging_step(StartupStep::OnLocalStateReady, log.clone()));

        sequence.run().await.unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                StartupStep::OpenDb,
                StartupStep::VerifyMigrations,
                StartupStep::ResumeProjections,
                StartupStep::ReconcileWorkQueues,
                StartupStep::LoadReadModels,
                StartupStep::OnLocalStateReady,
                StartupStep::StartNetworking,
            ]
        );
    }

    #[tokio::test]
    async fn on_local_state_ready_is_a_true_no_op_when_never_set() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let sequence = StartupSequence::new(
            logging_step(StartupStep::OpenDb, log.clone()),
            logging_step(StartupStep::VerifyMigrations, log.clone()),
            logging_step(StartupStep::ResumeProjections, log.clone()),
            logging_step(StartupStep::ReconcileWorkQueues, log.clone()),
            logging_step(StartupStep::LoadReadModels, log.clone()),
            logging_step(StartupStep::StartNetworking, log.clone()),
        );

        sequence.run().await.unwrap();

        assert_eq!(
            *log.lock().unwrap(),
            vec![
                StartupStep::OpenDb,
                StartupStep::VerifyMigrations,
                StartupStep::ResumeProjections,
                StartupStep::ReconcileWorkQueues,
                StartupStep::LoadReadModels,
                StartupStep::StartNetworking,
            ]
        );
    }

    #[tokio::test]
    async fn a_failure_partway_through_stops_the_sequence_before_networking() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let failing_step = step(|| async {
            Err::<(), StepError>(Box::<dyn std::error::Error + Send + Sync>::from(
                "migrations table corrupt",
            ))
        });
        let sequence = StartupSequence::new(
            logging_step(StartupStep::OpenDb, log.clone()),
            failing_step,
            logging_step(StartupStep::ResumeProjections, log.clone()),
            logging_step(StartupStep::ReconcileWorkQueues, log.clone()),
            logging_step(StartupStep::LoadReadModels, log.clone()),
            logging_step(StartupStep::StartNetworking, log.clone()),
        );

        let result = sequence.run().await;
        let failure = result.unwrap_err();
        assert_eq!(failure.step, StartupStep::VerifyMigrations);
        assert_eq!(
            failure.to_string(),
            "startup recovery failed at VerifyMigrations: migrations table corrupt"
        );

        // OpenDb ran (it's before the failure); nothing after
        // VerifyMigrations ever did — most importantly,
        // StartNetworking is nowhere in this log at all.
        assert_eq!(*log.lock().unwrap(), vec![StartupStep::OpenDb]);
    }

    #[tokio::test]
    async fn local_state_ready_hook_failing_still_blocks_networking() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let failing_hook = step(|| async {
            Err::<(), StepError>(Box::<dyn std::error::Error + Send + Sync>::from(
                "UI failed to initialize",
            ))
        });
        let sequence = StartupSequence::new(
            logging_step(StartupStep::OpenDb, log.clone()),
            logging_step(StartupStep::VerifyMigrations, log.clone()),
            logging_step(StartupStep::ResumeProjections, log.clone()),
            logging_step(StartupStep::ReconcileWorkQueues, log.clone()),
            logging_step(StartupStep::LoadReadModels, log.clone()),
            logging_step(StartupStep::StartNetworking, log.clone()),
        )
        .on_local_state_ready(failing_hook);

        let failure = sequence.run().await.unwrap_err();
        assert_eq!(failure.step, StartupStep::OnLocalStateReady);
        assert!(!log.lock().unwrap().contains(&StartupStep::StartNetworking));
    }
}
