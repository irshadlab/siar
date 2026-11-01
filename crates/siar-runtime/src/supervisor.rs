//! §29-§31, §97-§98, §121, §142: worker supervision.
//!
//! Each worker runs in its **own task**, so a panic is caught as a join error
//! and treated like any other failure (§121: a crashing component must not
//! take the runtime with it). Restarts back off exponentially and are bounded
//! by a circuit breaker: more than `max_restarts` failures inside `window`
//! and the worker is parked as `Unavailable` instead of restart-looping
//! forever (§98).
//!
//! The restart/backoff/circuit-breaker policy and the health bookkeeping are
//! ours (no off-the-shelf crate models "restart with a window-bounded breaker
//! and report it as `Degraded`"). What is *not* ours any more is the
//! bookkeeping of "which tasks are still running, and wait for all of them":
//! that is [`tokio_util::task::TaskTracker`].

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::AbortHandle;
use tokio_util::task::TaskTracker;

use crate::health::{HealthRegistry, HealthState};
use crate::lifecycle::{Lifecycle, RuntimeState};
use crate::metrics::Metrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    pub max_restarts: u32,
    pub window: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            window: Duration::from_secs(60),
            backoff_base: Duration::from_millis(250),
            backoff_max: Duration::from_secs(30),
        }
    }
}

impl RestartPolicy {
    /// `base * 2^(attempt-1)`, capped. `attempt` is 1-based; saturating.
    #[must_use]
    pub fn backoff(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(32);
        self.backoff_base
            .saturating_mul(1u32.checked_shl(shift).unwrap_or(u32::MAX))
            .min(self.backoff_max)
    }
}

/// Re-derive `Ready`/`Degraded` from health. No effect in other states, so a
/// worker failing during shutdown or maintenance never "revives" the runtime.
pub fn sync_lifecycle(lifecycle: &Lifecycle, health: &HealthRegistry) {
    let want = if health.report().overall == HealthState::Healthy {
        RuntimeState::Ready
    } else {
        RuntimeState::Degraded
    };
    if matches!(
        lifecycle.state(),
        RuntimeState::Ready | RuntimeState::Degraded
    ) {
        let _ = lifecycle.transition(want);
    }
}

#[derive(Clone)]
pub struct Supervisor {
    lifecycle: Lifecycle,
    health: HealthRegistry,
    /// Tracks every supervisor loop so shutdown can wait for all of them.
    tracker: TaskTracker,
    /// Only for the §36 forced path: the tracker waits, it does not abort.
    aborts: Arc<Mutex<Vec<AbortHandle>>>,
    metrics: Option<Arc<Metrics>>,
}

impl Supervisor {
    #[must_use]
    pub fn new(lifecycle: Lifecycle, health: HealthRegistry) -> Self {
        Self {
            lifecycle,
            health,
            tracker: TaskTracker::new(),
            aborts: Arc::default(),
            metrics: None,
        }
    }

    #[must_use]
    pub fn with_metrics(mut self, m: Arc<Metrics>) -> Self {
        self.metrics = Some(m);
        self
    }

    /// Supervise `factory`: each call must produce a fresh run of the worker.
    /// `Ok(())` is a clean exit (worker done); `Err(reason)` or a panic is a failure.
    pub fn spawn<F, Fut>(
        &self,
        name: &'static str,
        critical: bool,
        policy: RestartPolicy,
        factory: F,
    ) where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let lifecycle = self.lifecycle.clone();
        let health = self.health.clone();
        let factory = Arc::new(factory);
        let metrics = self.metrics.clone();
        let handle = self.tracker.spawn(async move {
            let mut failures: VecDeque<Instant> = VecDeque::new();
            health.set(name, HealthState::Healthy, "", critical);
            loop {
                if matches!(
                    lifecycle.state(),
                    RuntimeState::ShuttingDown | RuntimeState::Stopped
                ) {
                    break;
                }
                let f = factory.clone();
                let run = tokio::spawn(async move { f().await });
                let abort = run.abort_handle();
                let outcome = tokio::select! {
                    r = run => r,
                    () = lifecycle.shutdown_requested() => { abort.abort(); break; }
                };
                let reason = match outcome {
                    Ok(Ok(())) => {
                        health.remove(name);
                        sync_lifecycle(&lifecycle, &health);
                        break;
                    }
                    Ok(Err(e)) => e,
                    Err(e) if e.is_panic() => "panicked".to_owned(),
                    Err(_) => break,
                };
                let now = Instant::now();
                failures.push_back(now);
                while failures
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > policy.window)
                {
                    failures.pop_front();
                }
                let n = u32::try_from(failures.len()).unwrap_or(u32::MAX);
                tracing::warn!(worker = name, failures = n, %reason, "worker failed");
                if n > policy.max_restarts {
                    if let Some(m) = &metrics {
                        Metrics::inc(&m.worker_gave_up);
                    }
                    health.set(
                        name,
                        HealthState::Unavailable,
                        format!("gave up after {n} failures: {reason}"),
                        critical,
                    );
                    sync_lifecycle(&lifecycle, &health);
                    break;
                }
                health.set(
                    name,
                    HealthState::Degraded,
                    format!("restarting {n}/{}: {reason}", policy.max_restarts),
                    critical,
                );
                sync_lifecycle(&lifecycle, &health);
                tokio::select! {
                    () = tokio::time::sleep(policy.backoff(n)) => {}
                    () = lifecycle.shutdown_requested() => break,
                }
                health.set(name, HealthState::Healthy, "", critical);
                sync_lifecycle(&lifecycle, &health);
            }
        });
        let mut aborts = self
            .aborts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        aborts.retain(|h| !h.is_finished());
        aborts.push(handle.abort_handle());
    }

    /// §35: wait for every worker to finish, up to `grace`; abort stragglers
    /// (§36 forced shutdown). Returns how many had to be aborted.
    pub async fn drain(&self, grace: Duration) -> usize {
        self.tracker.close();
        if tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_ok()
        {
            return 0;
        }
        let stragglers: Vec<AbortHandle> = std::mem::take(
            &mut *self
                .aborts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut aborted = 0;
        for h in stragglers.into_iter().filter(|h| !h.is_finished()) {
            h.abort();
            aborted += 1;
        }
        aborted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn fast() -> RestartPolicy {
        RestartPolicy {
            max_restarts: 3,
            window: Duration::from_secs(10),
            backoff_base: Duration::from_millis(5),
            backoff_max: Duration::from_millis(40),
        }
    }
    fn ready() -> (Lifecycle, HealthRegistry, Supervisor) {
        let l = Lifecycle::new();
        for s in [
            RuntimeState::Recovering,
            RuntimeState::StartingServices,
            RuntimeState::Ready,
        ] {
            l.transition(s).unwrap();
        }
        let h = HealthRegistry::new();
        let s = Supervisor::new(l.clone(), h.clone());
        (l, h, s)
    }

    #[test]
    fn backoff_doubles_then_caps_and_never_overflows() {
        let p = fast();
        assert_eq!(p.backoff(1), Duration::from_millis(5));
        assert_eq!(p.backoff(2), Duration::from_millis(10));
        assert_eq!(p.backoff(3), Duration::from_millis(20));
        assert_eq!(p.backoff(10), Duration::from_millis(40));
        assert_eq!(p.backoff(u32::MAX), Duration::from_millis(40));
    }

    #[tokio::test]
    async fn failing_worker_is_restarted_then_circuit_opens() {
        let (l, h, s) = ready();
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        s.spawn("flaky", false, fast(), move || {
            let r = r.clone();
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                Err("boom".to_owned())
            }
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while h
                .report()
                .subsystems
                .iter()
                .all(|x| x.state != HealthState::Unavailable)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("circuit must open");
        assert_eq!(
            runs.load(Ordering::SeqCst),
            4,
            "initial run + 3 restarts, then it stops (§98)"
        );
        assert_eq!(
            l.state(),
            RuntimeState::Degraded,
            "non-critical Unavailable still degrades overall"
        );
    }

    #[tokio::test]
    async fn panic_is_contained_and_restarted() {
        let (_l, h, s) = ready();
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        s.spawn("panicky", false, fast(), move || {
            let r = r.clone();
            async move {
                if r.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("kaboom");
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            }
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while runs.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("restarted after panic");
        assert_ne!(h.report().overall, HealthState::Unavailable);
    }

    #[tokio::test]
    async fn clean_exit_is_not_restarted_and_recovers_health() {
        let (l, h, s) = ready();
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        s.spawn("oneshot", true, fast(), move || {
            let r = r.clone();
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert!(h.report().subsystems.is_empty());
        assert_eq!(l.state(), RuntimeState::Ready);
    }

    /// The §36 forced path, which the test above never reaches: shutdown was
    /// *not* requested, so the supervisor loop is still running when the grace
    /// period ends. `TaskTracker` only waits; the abort count is ours.
    #[tokio::test]
    async fn drain_aborts_and_counts_loops_still_running_after_the_grace_period() {
        let (_l, _h, s) = ready();
        for name in ["a", "b"] {
            s.spawn(name, false, fast(), || async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Ok(())
            });
        }
        s.spawn("quick", false, fast(), || async { Ok(()) });
        tokio::time::sleep(Duration::from_millis(30)).await;
        let aborted = s.drain(Duration::from_millis(50)).await;
        assert_eq!(
            aborted, 2,
            "the finished worker is not counted, the two sleepers are"
        );
    }

    #[tokio::test]
    async fn shutdown_stops_workers_and_drain_aborts_stragglers() {
        let (l, _h, s) = ready();
        s.spawn("sleeper", false, fast(), || async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(())
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        l.begin_shutdown();
        let aborted = s.drain(Duration::from_secs(2)).await;
        assert_eq!(aborted, 0, "supervisor loop exits on shutdown by itself");
    }

    #[tokio::test]
    async fn degraded_runtime_recovers_to_ready_when_worker_heals() {
        let (l, _h, s) = ready();
        let runs = Arc::new(AtomicU32::new(0));
        let r = runs.clone();
        s.spawn("heals", true, fast(), move || {
            let r = r.clone();
            async move {
                if r.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("once".into())
                } else {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(())
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            while runs.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        })
        .await
        .unwrap();
        assert_eq!(l.state(), RuntimeState::Ready);
    }
}
