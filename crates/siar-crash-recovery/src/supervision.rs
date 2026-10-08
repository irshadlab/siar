//! §73 "Supervision", §74 "Worker Restart Policy", §75 "Panic
//! Boundaries".
//!
//! The supervisor owns one [`RestartTracker`] per long-lived worker.
//! A failing worker gets bounded exponential backoff; if it keeps
//! failing inside a window the supervisor stops restarting it and
//! marks the subsystem degraded (no restart storm). Trackers are
//! independent, so one worker's failures never change another's
//! decisions. Time is passed in; nothing here sleeps or spawns.

use crate::recovery_mode::DegradedSubsystem;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// §73's supervision tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Worker {
    OutboxWorker,
    TransferScheduler,
    DtnScheduler,
    ProjectionRunner,
    DiscoveryService,
}

impl Worker {
    pub const ALL: [Worker; 5] = [
        Worker::OutboxWorker,
        Worker::TransferScheduler,
        Worker::DtnScheduler,
        Worker::ProjectionRunner,
        Worker::DiscoveryService,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Worker::OutboxWorker => "outbox-worker",
            Worker::TransferScheduler => "transfer-scheduler",
            Worker::DtnScheduler => "dtn-scheduler",
            Worker::ProjectionRunner => "projection-runner",
            Worker::DiscoveryService => "discovery-service",
        }
    }
}

/// §74: "bounded exponential backoff", plus the give-up rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    pub base_delay_millis: u64,
    pub max_delay_millis: u64,
    /// Failures within `window_millis` that trigger degraded-stop.
    pub max_failures_in_window: u32,
    pub window_millis: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            base_delay_millis: 200,
            max_delay_millis: 30_000,
            max_failures_in_window: 5,
            window_millis: 60_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    RestartAfter {
        delay_millis: u64,
    },
    /// Stop restarting and mark the subsystem degraded.
    MarkDegradedAndStop,
}

#[derive(Debug, Clone)]
pub struct RestartTracker {
    policy: RestartPolicy,
    /// Failure timestamps inside the window; bounded by
    /// `max_failures_in_window`.
    recent: VecDeque<u64>,
    stopped: bool,
}

impl RestartTracker {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            recent: VecDeque::new(),
            stopped: false,
        }
    }

    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Records a failure at `now_millis` and decides what to do.
    /// Delay = `base * 2^(failures_in_window - 1)`, capped.
    pub fn on_failure(&mut self, now_millis: u64) -> RestartDecision {
        if self.stopped {
            return RestartDecision::MarkDegradedAndStop;
        }
        while self
            .recent
            .front()
            .is_some_and(|&t| now_millis.saturating_sub(t) >= self.policy.window_millis)
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now_millis);
        let n = self.recent.len() as u32;
        if n >= self.policy.max_failures_in_window {
            self.stopped = true;
            return RestartDecision::MarkDegradedAndStop;
        }
        let shift = (n - 1).min(40);
        let delay = self
            .policy
            .base_delay_millis
            .saturating_mul(1u64 << shift)
            .min(self.policy.max_delay_millis);
        RestartDecision::RestartAfter {
            delay_millis: delay,
        }
    }

    /// An operator/explicit action re-enables a stopped worker.
    pub fn reset(&mut self) {
        self.recent.clear();
        self.stopped = false;
    }
}

#[derive(Debug, Clone)]
pub struct Supervisor {
    trackers: HashMap<Worker, RestartTracker>,
}

impl Supervisor {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            trackers: Worker::ALL
                .iter()
                .map(|w| (*w, RestartTracker::new(policy)))
                .collect(),
        }
    }

    pub fn report_failure(&mut self, w: Worker, now_millis: u64) -> RestartDecision {
        self.trackers
            .get_mut(&w)
            .map(|t| t.on_failure(now_millis))
            .unwrap_or(RestartDecision::MarkDegradedAndStop)
    }

    pub fn is_stopped(&self, w: Worker) -> bool {
        self.trackers
            .get(&w)
            .is_some_and(RestartTracker::is_stopped)
    }

    /// Subsystems the supervisor has given up on, for §48's
    /// diagnostics surface.
    pub fn degraded_subsystems(&self) -> Vec<DegradedSubsystem> {
        Worker::ALL
            .iter()
            .filter(|w| self.is_stopped(**w))
            .map(|w| DegradedSubsystem::Other(w.name()))
            .collect()
    }
}

// ---------------------------------------------------------------------
// §75
// ---------------------------------------------------------------------

/// A panic caught at a service boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("panic at service boundary: {message}")]
pub struct PanicReport {
    pub message: String,
}

/// Runs `f` and converts a panic into a [`PanicReport`] instead of
/// letting it unwind into the caller — the shape every `extern "C"` /
/// JNI entry point must use, since unwinding across FFI is undefined
/// behaviour. Requires `panic = "unwind"` (the default); with
/// `panic = "abort"` the process aborts instead, which §76 accepts
/// for unrecoverable invariants. Ordinary errors must still be
/// `Result`s: this is a boundary net, not error control.
pub fn catch_boundary<R>(f: impl FnOnce() -> R) -> Result<R, PanicReport> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let message = if let Some(s) = p.downcast_ref::<&'static str>() {
            (*s).to_owned()
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_owned()
        };
        PanicReport { message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RestartPolicy {
        RestartPolicy {
            base_delay_millis: 100,
            max_delay_millis: 1_000,
            max_failures_in_window: 5,
            window_millis: 10_000,
        }
    }

    fn delay(d: RestartDecision) -> u64 {
        match d {
            RestartDecision::RestartAfter { delay_millis } => delay_millis,
            other => panic!("expected restart, got {other:?}"),
        }
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let mut t = RestartTracker::new(RestartPolicy {
            max_failures_in_window: 100,
            ..policy()
        });
        let ds: Vec<_> = (0..6).map(|i| delay(t.on_failure(i))).collect();
        assert_eq!(ds, vec![100, 200, 400, 800, 1_000, 1_000]);
    }

    #[test]
    fn repeated_failure_stops_the_restart_storm() {
        let mut t = RestartTracker::new(policy());
        for i in 0..4 {
            assert!(matches!(
                t.on_failure(i),
                RestartDecision::RestartAfter { .. }
            ));
        }
        assert_eq!(t.on_failure(4), RestartDecision::MarkDegradedAndStop);
        assert!(t.is_stopped());
        // stays stopped, even much later
        assert_eq!(
            t.on_failure(1_000_000),
            RestartDecision::MarkDegradedAndStop
        );
    }

    #[test]
    fn old_failures_age_out_of_the_window() {
        let mut t = RestartTracker::new(policy());
        for i in 0..4 {
            t.on_failure(i);
        }
        // far later: window cleared, back to the base delay
        assert_eq!(delay(t.on_failure(100_000)), 100);
        assert!(!t.is_stopped());
    }

    #[test]
    fn explicit_reset_reenables_a_stopped_worker() {
        let mut t = RestartTracker::new(policy());
        for i in 0..5 {
            t.on_failure(i);
        }
        assert!(t.is_stopped());
        t.reset();
        assert_eq!(delay(t.on_failure(10)), 100);
    }

    #[test]
    fn one_workers_failures_do_not_affect_another() {
        let mut s = Supervisor::new(policy());
        for i in 0..5 {
            s.report_failure(Worker::DiscoveryService, i);
        }
        assert!(s.is_stopped(Worker::DiscoveryService));
        assert!(!s.is_stopped(Worker::OutboxWorker));
        assert_eq!(delay(s.report_failure(Worker::OutboxWorker, 0)), 100);
    }

    #[test]
    fn stopped_workers_surface_as_degraded_subsystems() {
        let mut s = Supervisor::new(policy());
        assert!(s.degraded_subsystems().is_empty());
        for i in 0..5 {
            s.report_failure(Worker::ProjectionRunner, i);
        }
        assert_eq!(
            s.degraded_subsystems(),
            vec![DegradedSubsystem::Other("projection-runner")]
        );
    }

    #[test]
    fn all_five_spec_workers_are_supervised() {
        assert_eq!(Worker::ALL.len(), 5);
        let s = Supervisor::new(policy());
        for w in Worker::ALL {
            assert!(!s.is_stopped(w));
        }
    }

    #[test]
    fn panics_are_caught_and_reported_not_propagated() {
        let r = catch_boundary(|| -> u32 { panic!("boom {}", 7) });
        assert_eq!(
            r,
            Err(PanicReport {
                message: "boom 7".into()
            })
        );
        let r = catch_boundary(|| -> u32 { panic!("static") });
        assert_eq!(r.unwrap_err().message, "static");
    }

    #[test]
    fn non_panicking_work_passes_through_untouched() {
        assert_eq!(catch_boundary(|| 41 + 1), Ok(42));
    }
}
