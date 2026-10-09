//! `04-offline-event-log-architecture.md` §30 "Effect Processing",
//! §32 "Retry Event Granularity", §45 "Replay Must Not Re-run Side
//! Effects", §46 "Replay Modes" — §92 Phase 5's "effects" and "retry".
//!
//! §30: events may schedule external work (`MessageQueued → send`,
//! `TransferCreated → transfer`, `EmergencyReportCreated →
//! routing/DTN`), and "effects are asynchronous, retryable, and
//! idempotent." [`EffectProcessor`] is the loop that drains an
//! [`crate::outbox::Outbox`] into an [`EffectHandler`] with exactly
//! those properties:
//!
//! - **asynchronous** — it runs after the commit that created the work
//!   (§13's "network effect later"), never inside it.
//! - **retryable** — a [`EffectOutcome::Retry`] reschedules the entry
//!   per a [`BackoffPolicy`]; a crash mid-effect is covered by the
//!   outbox lease (see [`crate::outbox`]).
//! - **idempotent** — the processor delivers *at least once*. Every
//!   [`EffectHandler`] receives a stable [`crate::outbox::OperationId`]
//!   and must make it the receiver-side deduplication key. That
//!   contract, not any property of this loop, is what yields §31's
//!   "effectively-once logical outcome."
//!
//! ## §32: attempts are metrics, transitions are history
//!
//! "Do not append one durable event per tiny transport retry." The
//! processor therefore never appends events. It does two separate
//! things instead:
//!
//! - every attempt, retry, completion and failure bumps a counter in
//!   [`AttemptMetrics`] (§32's "tracing/metrics");
//! - only *meaningful transitions* are surfaced, through
//!   [`TransitionSink`]: [`TransitionKind::Deferred`] (first time an
//!   operation fails and goes back to waiting — §32's `MessageDeferred`
//!   analogue), [`TransitionKind::Completed`], and
//!   [`TransitionKind::Failed`]. The tenth retry of an operation that
//!   is already deferred is *not* a transition and emits nothing.
//!
//! What a sink does with a transition — typically append the domain's
//! own `MessageDeferred`/`TransferFailed` event — is the owning
//! domain's decision; this crate has no domain event types (see
//! `lib.rs`). The sink is synchronous so the hot loop never allocates a
//! future for it; a sink that must do async work hands the transition to
//! its own channel.
//!
//! ## §45/§46: replay never re-runs side effects
//!
//! There are two structural reasons projection replay cannot resend a
//! message, restart a transfer, or rebroadcast an SOS, and one runtime
//! guard:
//!
//! 1. [`crate::projection::Projection::apply`] has no access to an
//!    outbox or handler — it can only mutate its own view.
//! 2. Effects are created at *command-commit* time
//!    ([`crate::command_flow`]) or by explicit reconciliation
//!    ([`crate::outbox_reconcile`]), never as a by-product of reading
//!    the log.
//! 3. [`EffectProcessor::run_once`] takes a [`ReplayMode`] and does
//!    nothing at all under [`ReplayMode::ProjectionOnly`] — it does not
//!    even claim work, so a maintenance rebuild cannot start a transfer
//!    by accident.
//!
//! Under [`ReplayMode::Recovery`] and [`ReplayMode::Live`] the processor
//! runs "according to durable pending state" (§46): it executes what
//! the outbox says is owed, not what the log's history would suggest.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ids::{EventId, StreamId, Timestamp};
use crate::outbox::{EffectKind, OperationId, Outbox, OutboxEntry, OutboxError};
use crate::projection::ReplayMode;

/// What a handler reports for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectOutcome {
    /// The logical outcome is achieved (or the receiver already had
    /// it — deduplicated by [`OperationId`]).
    Done,
    /// Transient failure; try again later per the [`BackoffPolicy`].
    Retry { reason: String },
    /// Retrying cannot help (recipient revoked, payload invalid, ...).
    Permanent { reason: String },
}

/// Executes one kind of external effect.
///
/// **Must be idempotent on `entry.operation_id`.** The processor
/// guarantees at-least-once, not exactly-once (§31) — after a crash
/// between "effect succeeded" and "outbox marked done", the same
/// operation is handed to the handler again.
pub trait EffectHandler: Send + Sync {
    fn handle(&self, entry: &OutboxEntry) -> impl Future<Output = EffectOutcome> + Send;
}

/// §32's retry schedule: exponential, capped, deterministically
/// jittered, bounded in attempts.
///
/// Jitter is derived from the [`OperationId`] and attempt number, not
/// from a random source: retries from many operations spread out
/// (no thundering herd after a transport comes back), while every run
/// of a given test or replay computes the same delays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    pub base_ms: u64,
    pub max_ms: u64,
    /// Total attempts allowed, counting the first. After this many
    /// claims an operation that still fails is dead-lettered.
    pub max_attempts: u32,
    /// Up to this many thousandths of each delay may be shaved off.
    /// Clamped to 1000.
    pub jitter_permille: u16,
}

impl BackoffPolicy {
    pub const DEFAULT: Self = Self {
        base_ms: 1_000,
        max_ms: 15 * 60 * 1_000,
        max_attempts: 12,
        jitter_permille: 250,
    };

    /// Delay before the next attempt, given how many attempts have
    /// already been made (`attempts_made >= 1`, as reported by the
    /// outbox's claim counter). `None` means "give up".
    pub fn delay_after(&self, attempts_made: u32, id: OperationId) -> Option<u64> {
        if attempts_made >= self.max_attempts {
            return None;
        }
        let exponent = attempts_made.saturating_sub(1).min(32);
        let raw = self
            .base_ms
            .saturating_mul(1u64 << exponent)
            .min(self.max_ms);

        let jitter = u128::from(self.jitter_permille.min(1000));
        let bytes = id.as_bytes();
        let mix =
            u16::from_le_bytes([bytes[0], bytes[1]]) ^ (attempts_made as u16).wrapping_mul(0x9E37);
        let shave = u128::from(raw) * jitter * u128::from(mix) / (1000 * u128::from(u16::MAX));
        let delay = raw.saturating_sub(shave as u64);
        Some(if raw > 0 { delay.max(1) } else { 0 })
    }
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// §32's "meaningful transition", with enough identity for a sink to
/// build the owning domain's own event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTransition {
    pub operation_id: OperationId,
    pub kind: EffectKind,
    pub source_event_id: EventId,
    pub stream_id: StreamId,
    pub attempts: u32,
    pub change: TransitionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionKind {
    /// First failure: the operation moved from "being attempted" to
    /// "waiting to be retried" (§32's `MessageDeferred`).
    Deferred {
        next_attempt_at: Timestamp,
    },
    Completed,
    /// Terminal (§32's `TransferFailed`).
    Failed {
        reason: String,
    },
}

/// Receives only the transitions §32 calls permanent history.
pub trait TransitionSink: Send + Sync {
    fn on_transition(&self, transition: EffectTransition);
}

/// For callers that only want the metrics.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTransitions;

impl TransitionSink for NoTransitions {
    fn on_transition(&self, _transition: EffectTransition) {}
}

/// §32's "tracing/metrics" half. Relaxed atomics: these are
/// monotonic diagnostics counters, never used for synchronization.
#[derive(Debug, Default)]
pub struct AttemptMetrics {
    attempts: AtomicU64,
    completed: AtomicU64,
    retried: AtomicU64,
    failed: AtomicU64,
    runs_skipped_by_mode: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AttemptMetricsSnapshot {
    pub attempts: u64,
    pub completed: u64,
    pub retried: u64,
    pub failed: u64,
    pub runs_skipped_by_mode: u64,
}

impl AttemptMetrics {
    pub fn snapshot(&self) -> AttemptMetricsSnapshot {
        AttemptMetricsSnapshot {
            attempts: self.attempts.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            retried: self.retried.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            runs_skipped_by_mode: self.runs_skipped_by_mode.load(Ordering::Relaxed),
        }
    }
}

/// What one [`EffectProcessor::run_once`] pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunReport {
    pub claimed: u32,
    pub completed: u32,
    pub retried: u32,
    pub failed: u32,
    /// `true` when [`ReplayMode::ProjectionOnly`] suppressed the whole
    /// pass (§45/§46); nothing was claimed.
    pub skipped_by_mode: bool,
}

/// Drains an [`Outbox`] into an [`EffectHandler`]. Generic over all
/// three collaborators (`?Sized`, so `&dyn` still works where a caller
/// wants it): the default path is fully monomorphized.
pub struct EffectProcessor<'a, O, H, S>
where
    O: Outbox + ?Sized,
    H: EffectHandler + ?Sized,
    S: TransitionSink + ?Sized,
{
    outbox: &'a O,
    handler: &'a H,
    sink: &'a S,
    policy: BackoffPolicy,
    lease_ms: u64,
    batch_size: usize,
    metrics: AttemptMetrics,
}

impl<'a, O, H, S> EffectProcessor<'a, O, H, S>
where
    O: Outbox + ?Sized,
    H: EffectHandler + ?Sized,
    S: TransitionSink + ?Sized,
{
    /// `lease_ms` must comfortably exceed the handler's own worst-case
    /// attempt time; a lease that expires mid-attempt lets a second
    /// worker run the same operation (still safe — handlers are
    /// idempotent — but wasteful).
    pub fn new(
        outbox: &'a O,
        handler: &'a H,
        sink: &'a S,
        policy: BackoffPolicy,
        lease_ms: u64,
        batch_size: usize,
    ) -> Self {
        Self {
            outbox,
            handler,
            sink,
            policy,
            lease_ms,
            batch_size: batch_size.max(1),
            metrics: AttemptMetrics::default(),
        }
    }

    pub fn metrics(&self) -> AttemptMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// One claim-execute-record pass. Entries in a batch run
    /// sequentially: a caller wanting concurrency runs several
    /// processors over the same outbox (claims are exclusive), which
    /// keeps per-batch ordering deterministic and avoids this crate
    /// choosing an executor.
    pub async fn run_once(
        &self,
        mode: ReplayMode,
        now: Timestamp,
    ) -> Result<RunReport, OutboxError> {
        let mut report = RunReport::default();
        if !mode.may_schedule_external_work() {
            self.metrics
                .runs_skipped_by_mode
                .fetch_add(1, Ordering::Relaxed);
            report.skipped_by_mode = true;
            return Ok(report);
        }

        let claimed = self
            .outbox
            .claim_due(now, self.lease_ms, self.batch_size)
            .await?;
        report.claimed = claimed.len() as u32;

        for entry in claimed {
            self.metrics.attempts.fetch_add(1, Ordering::Relaxed);
            let id = entry.operation_id;
            match self.handler.handle(&entry).await {
                EffectOutcome::Done => {
                    self.outbox.complete(id, now).await?;
                    self.metrics.completed.fetch_add(1, Ordering::Relaxed);
                    report.completed += 1;
                    self.emit(&entry, TransitionKind::Completed);
                }
                EffectOutcome::Retry { reason } => {
                    match self.policy.delay_after(entry.attempts, id) {
                        Some(delay) => {
                            let next = Timestamp(now.0.saturating_add(delay));
                            self.outbox.reschedule(id, next, Some(reason), now).await?;
                            self.metrics.retried.fetch_add(1, Ordering::Relaxed);
                            report.retried += 1;
                            // §32: only the *first* deferral is history.
                            if entry.attempts == 1 {
                                self.emit(
                                    &entry,
                                    TransitionKind::Deferred {
                                        next_attempt_at: next,
                                    },
                                );
                            }
                        }
                        None => {
                            let reason = format!("retries exhausted: {reason}");
                            self.fail(&entry, reason, now, &mut report).await?;
                        }
                    }
                }
                EffectOutcome::Permanent { reason } => {
                    self.fail(&entry, reason, now, &mut report).await?;
                }
            }
        }
        Ok(report)
    }

    async fn fail(
        &self,
        entry: &OutboxEntry,
        reason: String,
        now: Timestamp,
        report: &mut RunReport,
    ) -> Result<(), OutboxError> {
        self.outbox
            .dead_letter(entry.operation_id, reason.clone(), now)
            .await?;
        self.metrics.failed.fetch_add(1, Ordering::Relaxed);
        report.failed += 1;
        self.emit(entry, TransitionKind::Failed { reason });
        Ok(())
    }

    fn emit(&self, entry: &OutboxEntry, change: TransitionKind) {
        self.sink.on_transition(EffectTransition {
            operation_id: entry.operation_id,
            kind: entry.kind,
            source_event_id: entry.source_event_id,
            stream_id: entry.stream_id,
            attempts: entry.attempts,
            change,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::{InMemoryOutbox, OutboxState};
    use std::sync::Mutex;

    fn entry(n: u8) -> OutboxEntry {
        OutboxEntry::new(
            EventId::from_uuid(uuid::Uuid::from_bytes([n; 16])),
            StreamId::from_name("s"),
            EffectKind(1),
            0,
            vec![n],
            Timestamp(0),
        )
        .unwrap()
    }

    /// Replies from a script, one outcome per call, and records calls.
    struct Scripted {
        script: Mutex<Vec<EffectOutcome>>,
        seen: Mutex<Vec<OperationId>>,
    }

    impl Scripted {
        fn new(mut script: Vec<EffectOutcome>) -> Self {
            script.reverse();
            Self {
                script: Mutex::new(script),
                seen: Mutex::new(vec![]),
            }
        }
        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    impl EffectHandler for Scripted {
        async fn handle(&self, entry: &OutboxEntry) -> EffectOutcome {
            self.seen.lock().unwrap().push(entry.operation_id);
            self.script
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(EffectOutcome::Done)
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<EffectTransition>>);
    impl TransitionSink for Recorder {
        fn on_transition(&self, t: EffectTransition) {
            self.0.lock().unwrap().push(t);
        }
    }

    const NO_JITTER: BackoffPolicy = BackoffPolicy {
        base_ms: 100,
        max_ms: 1_000,
        max_attempts: 4,
        jitter_permille: 0,
    };

    #[test]
    fn backoff_is_exponential_capped_and_bounded() {
        let id = OperationId::from_bytes([1; 16]);
        assert_eq!(NO_JITTER.delay_after(1, id), Some(100));
        assert_eq!(NO_JITTER.delay_after(2, id), Some(200));
        assert_eq!(NO_JITTER.delay_after(3, id), Some(400));
        assert_eq!(NO_JITTER.delay_after(4, id), None, "max_attempts reached");
        let wide = BackoffPolicy {
            max_attempts: 100,
            ..NO_JITTER
        };
        assert_eq!(wide.delay_after(50, id), Some(1_000), "capped at max_ms");
        assert_eq!(wide.delay_after(99, id), Some(1_000), "no shift overflow");
    }

    #[test]
    fn jitter_only_shaves_is_deterministic_and_spreads() {
        let policy = BackoffPolicy {
            jitter_permille: 500,
            ..NO_JITTER
        };
        let mut distinct = std::collections::HashSet::new();
        for n in 0..=255u8 {
            let id = OperationId::from_bytes([n; 16]);
            let d = policy.delay_after(2, id).unwrap();
            assert!((100..=200).contains(&d), "within [raw/2, raw], got {d}");
            assert_eq!(Some(d), policy.delay_after(2, id), "deterministic");
            distinct.insert(d);
        }
        assert!(distinct.len() > 20, "different operations spread out");
    }

    #[tokio::test]
    async fn success_completes_and_emits_one_transition() {
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let handler = Scripted::new(vec![EffectOutcome::Done]);
        let sink = Recorder::default();
        let p = EffectProcessor::new(&outbox, &handler, &sink, NO_JITTER, 10_000, 8);

        let r = p.run_once(ReplayMode::Live, Timestamp(0)).await.unwrap();
        assert_eq!((r.claimed, r.completed), (1, 1));
        assert_eq!(
            outbox.get(id).await.unwrap().unwrap().state,
            OutboxState::Done
        );
        {
            let t = sink.0.lock().unwrap();
            assert_eq!(t.len(), 1);
            assert_eq!(t[0].change, TransitionKind::Completed);
        }
        // Done work is never handed out again.
        let r = p
            .run_once(ReplayMode::Live, Timestamp(10_000_000))
            .await
            .unwrap();
        assert_eq!(r.claimed, 0);
    }

    #[tokio::test]
    async fn retries_are_metrics_only_after_the_first_deferral() {
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let retry = || EffectOutcome::Retry {
            reason: "link down".into(),
        };
        let handler = Scripted::new(vec![retry(), retry(), retry(), EffectOutcome::Done]);
        let sink = Recorder::default();
        let p = EffectProcessor::new(&outbox, &handler, &sink, NO_JITTER, 10_000, 8);

        let mut now = 0u64;
        for _ in 0..4 {
            p.run_once(ReplayMode::Live, Timestamp(now)).await.unwrap();
            now += 10_000;
        }
        assert_eq!(handler.calls(), 4);
        let m = p.metrics();
        assert_eq!((m.attempts, m.retried, m.completed, m.failed), (4, 3, 1, 0));

        // §32: three failed attempts, but history gets exactly one
        // Deferred and one Completed — not one event per attempt.
        {
            let t = sink.0.lock().unwrap();
            let kinds: Vec<_> = t.iter().map(|x| &x.change).collect();
            assert_eq!(kinds.len(), 2, "got {kinds:?}");
            assert!(matches!(kinds[0], TransitionKind::Deferred { .. }));
            assert_eq!(kinds[1], &TransitionKind::Completed);
            assert_eq!(t[1].attempts, 4);
        }
        assert_eq!(
            outbox.get(id).await.unwrap().unwrap().last_error.as_deref(),
            Some("link down")
        );
    }

    #[tokio::test]
    async fn a_retry_is_not_run_before_its_backoff_elapses() {
        let outbox = InMemoryOutbox::new();
        outbox.enqueue(entry(1)).await.unwrap();
        let handler = Scripted::new(vec![EffectOutcome::Retry { reason: "x".into() }]);
        let p = EffectProcessor::new(&outbox, &handler, &NoTransitions, NO_JITTER, 10_000, 8);
        p.run_once(ReplayMode::Live, Timestamp(0)).await.unwrap();
        // Backoff after attempt 1 is 100ms.
        let r = p.run_once(ReplayMode::Live, Timestamp(99)).await.unwrap();
        assert_eq!(r.claimed, 0);
        let r = p.run_once(ReplayMode::Live, Timestamp(100)).await.unwrap();
        assert_eq!(r.claimed, 1);
    }

    #[tokio::test]
    async fn exhausting_attempts_dead_letters_once_with_a_failed_transition() {
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let retry = || EffectOutcome::Retry {
            reason: "nope".into(),
        };
        let handler = Scripted::new(vec![retry(), retry(), retry(), retry(), retry()]);
        let sink = Recorder::default();
        let p = EffectProcessor::new(&outbox, &handler, &sink, NO_JITTER, 10_000, 8);
        let mut now = 0u64;
        for _ in 0..8 {
            p.run_once(ReplayMode::Live, Timestamp(now)).await.unwrap();
            now += 100_000;
        }
        assert_eq!(handler.calls(), 4, "max_attempts = 4, then no more");
        let got = outbox.get(id).await.unwrap().unwrap();
        assert_eq!(got.state, OutboxState::DeadLettered);
        assert!(got.last_error.unwrap().starts_with("retries exhausted"));
        let t = sink.0.lock().unwrap();
        assert!(matches!(
            t.last().unwrap().change,
            TransitionKind::Failed { .. }
        ));
        assert_eq!(p.metrics().failed, 1);
    }

    #[tokio::test]
    async fn a_permanent_failure_skips_retries() {
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let handler = Scripted::new(vec![EffectOutcome::Permanent {
            reason: "recipient revoked".into(),
        }]);
        let p = EffectProcessor::new(&outbox, &handler, &NoTransitions, NO_JITTER, 10_000, 8);
        let r = p.run_once(ReplayMode::Live, Timestamp(0)).await.unwrap();
        assert_eq!(r.failed, 1);
        assert_eq!(
            outbox.get(id).await.unwrap().unwrap().state,
            OutboxState::DeadLettered
        );
    }

    #[tokio::test]
    async fn projection_only_mode_claims_and_executes_nothing() {
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let handler = Scripted::new(vec![]);
        let p = EffectProcessor::new(&outbox, &handler, &NoTransitions, NO_JITTER, 10_000, 8);

        let r = p
            .run_once(ReplayMode::ProjectionOnly, Timestamp(0))
            .await
            .unwrap();
        assert!(r.skipped_by_mode);
        assert_eq!(handler.calls(), 0);
        // Untouched — not even claimed, so attempts did not move.
        let got = outbox.get(id).await.unwrap().unwrap();
        assert_eq!((got.state, got.attempts), (OutboxState::Pending, 0));
        assert_eq!(p.metrics().runs_skipped_by_mode, 1);

        // Recovery and Live both run the same pending state.
        let r = p
            .run_once(ReplayMode::Recovery, Timestamp(0))
            .await
            .unwrap();
        assert_eq!(r.completed, 1);
    }

    #[tokio::test]
    async fn a_crash_after_the_effect_redelivers_the_same_operation_id() {
        // §31: the handler "succeeded" but the process died before
        // `complete`. Simulated by claiming without completing; once
        // the lease lapses the SAME id comes back, which is the
        // receiver's deduplication key.
        let outbox = InMemoryOutbox::new();
        let e = entry(1);
        let id = e.operation_id;
        outbox.enqueue(e).await.unwrap();
        let claimed = outbox.claim_due(Timestamp(0), 1_000, 8).await.unwrap();
        assert_eq!(claimed.len(), 1);

        let handler = Scripted::new(vec![EffectOutcome::Done]);
        let p = EffectProcessor::new(&outbox, &handler, &NoTransitions, NO_JITTER, 1_000, 8);
        assert_eq!(
            p.run_once(ReplayMode::Live, Timestamp(500))
                .await
                .unwrap()
                .claimed,
            0
        );
        let r = p
            .run_once(ReplayMode::Live, Timestamp(1_000))
            .await
            .unwrap();
        assert_eq!(r.completed, 1);
        assert_eq!(handler.seen.lock().unwrap().as_slice(), &[id]);
    }
}
