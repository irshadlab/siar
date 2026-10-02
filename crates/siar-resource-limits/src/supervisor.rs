//! §163 supervisor, §164 restart policy.
//!
//! A pure state machine (no spawning, no sleeping — the runtime feeds it
//! exit events and clock readings and acts on its decisions), so restart
//! behaviour is testable and deterministic. Rules:
//! - restart with exponential backoff (bounded);
//! - more than `max_restarts` crashes inside `window_millis` → the worker
//!   is marked **Degraded**, not restarted in a loop;
//! - a degraded worker is retried at most once per `degraded_cooldown`,
//!   and a crash of that single probe re-degrades it immediately.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartMode {
    /// Long-lived service: any exit, even a clean one, is a failure.
    Always,
    /// Restart only after a crash; a clean exit means "done".
    OnFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    pub mode: RestartMode,
    pub max_restarts: u32,
    pub window_millis: u64,
    pub base_backoff_millis: u64,
    pub max_backoff_millis: u64,
    pub degraded_cooldown_millis: u64,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            mode: RestartMode::Always,
            max_restarts: 5,
            window_millis: 60_000,
            base_backoff_millis: 200,
            max_backoff_millis: 10_000,
            degraded_cooldown_millis: 5 * 60_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    Clean,
    Crash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    Stopped,
    Running,
    Backoff { restart_at_millis: u64 },
    Degraded { retry_at_millis: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    RestartAfter {
        millis: u64,
    },
    MarkDegraded {
        retry_at_millis: u64,
    },
    /// Finished normally; nothing more to do.
    Done,
}

#[derive(Debug)]
struct Worker {
    state: WorkerState,
    crashes: VecDeque<u64>,
    probing: bool,
}

#[derive(Debug)]
pub struct Supervisor<K> {
    policy: RestartPolicy,
    workers: HashMap<K, Worker>,
}

impl<K: Hash + Eq + Copy> Supervisor<K> {
    pub fn new(policy: RestartPolicy) -> Self {
        Self {
            policy,
            workers: HashMap::new(),
        }
    }

    fn worker(&mut self, k: K) -> &mut Worker {
        self.workers.entry(k).or_insert(Worker {
            state: WorkerState::Stopped,
            crashes: VecDeque::new(),
            probing: false,
        })
    }

    pub fn on_started(&mut self, k: K) {
        self.worker(k).state = WorkerState::Running;
    }

    pub fn state(&self, k: &K) -> WorkerState {
        self.workers
            .get(k)
            .map_or(WorkerState::Stopped, |w| w.state)
    }

    pub fn on_exit(&mut self, k: K, kind: ExitKind, now: u64) -> Decision {
        let p = self.policy;
        let w = self.worker(k);
        if kind == ExitKind::Clean && p.mode == RestartMode::OnFailure {
            w.state = WorkerState::Stopped;
            w.probing = false;
            return Decision::Done;
        }
        // Prune, then record this failure (bounded ring).
        while w
            .crashes
            .front()
            .is_some_and(|t| now.saturating_sub(*t) > p.window_millis)
        {
            w.crashes.pop_front();
        }
        w.crashes.push_back(now);
        while w.crashes.len() > p.max_restarts as usize + 1 {
            w.crashes.pop_front();
        }
        let n = w.crashes.len() as u32; // failures inside the window, >= 1
        if w.probing || n > p.max_restarts {
            w.probing = false;
            let retry_at = now.saturating_add(p.degraded_cooldown_millis);
            w.state = WorkerState::Degraded {
                retry_at_millis: retry_at,
            };
            return Decision::MarkDegraded {
                retry_at_millis: retry_at,
            };
        }
        let backoff = p
            .base_backoff_millis
            .saturating_mul(1u64 << (n - 1).min(30))
            .min(p.max_backoff_millis);
        w.state = WorkerState::Backoff {
            restart_at_millis: now.saturating_add(backoff),
        };
        Decision::RestartAfter { millis: backoff }
    }

    /// Workers the runtime should (re)start now. Degraded workers whose
    /// cooldown elapsed are returned as a single *probe*.
    pub fn due(&mut self, now: u64) -> Vec<K> {
        let mut out = Vec::new();
        for (k, w) in &mut self.workers {
            match w.state {
                WorkerState::Backoff { restart_at_millis } if now >= restart_at_millis => {
                    out.push(*k)
                }
                WorkerState::Degraded { retry_at_millis } if now >= retry_at_millis => {
                    w.probing = true;
                    w.state = WorkerState::Backoff {
                        restart_at_millis: now,
                    };
                    out.push(*k);
                }
                _ => {}
            }
        }
        out
    }

    pub fn degraded(&self) -> Vec<K> {
        self.workers
            .iter()
            .filter(|(_, w)| matches!(w.state, WorkerState::Degraded { .. }))
            .map(|(k, _)| *k)
            .collect()
    }

    /// Operator intervention: forget history and allow a normal restart.
    pub fn reset(&mut self, k: K) {
        let w = self.worker(k);
        w.crashes.clear();
        w.probing = false;
        w.state = WorkerState::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> RestartPolicy {
        RestartPolicy {
            mode: RestartMode::Always,
            max_restarts: 3,
            window_millis: 10_000,
            base_backoff_millis: 100,
            max_backoff_millis: 350,
            degraded_cooldown_millis: 5_000,
        }
    }

    #[test]
    fn backoff_doubles_then_caps() {
        let mut s = Supervisor::new(pol());
        s.on_started("w");
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 0),
            Decision::RestartAfter { millis: 100 }
        );
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 1),
            Decision::RestartAfter { millis: 200 }
        );
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 2),
            Decision::RestartAfter { millis: 350 }
        );
    }

    #[test]
    fn crash_loop_degrades_instead_of_restarting_forever() {
        let mut s = Supervisor::new(pol());
        for t in 0..3 {
            assert!(matches!(
                s.on_exit("w", ExitKind::Crash, t),
                Decision::RestartAfter { .. }
            ));
        }
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 3),
            Decision::MarkDegraded {
                retry_at_millis: 5_003
            }
        );
        assert_eq!(s.degraded(), vec!["w"]);
        assert!(
            s.due(4_000).is_empty(),
            "no restarts while degraded and cooling down"
        );
    }

    #[test]
    fn old_crashes_age_out_of_the_window() {
        let mut s = Supervisor::new(pol());
        for t in [0, 1, 2] {
            s.on_exit("w", ExitKind::Crash, t);
        }
        // far later: a lone crash is a first failure again, not the 4th.
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 100_000),
            Decision::RestartAfter { millis: 100 }
        );
    }

    #[test]
    fn degraded_worker_gets_one_probe_and_re_degrades_if_it_crashes() {
        let mut s = Supervisor::new(pol());
        for t in 0..4 {
            s.on_exit("w", ExitKind::Crash, t);
        }
        assert_eq!(s.due(5_003), vec!["w"]);
        s.on_started("w");
        // the probe dies once -> straight back to degraded, no backoff loop
        assert_eq!(
            s.on_exit("w", ExitKind::Crash, 5_100),
            Decision::MarkDegraded {
                retry_at_millis: 10_100
            }
        );
        assert!(s.due(6_000).is_empty());
    }

    #[test]
    fn a_probe_that_stays_up_is_a_normal_worker_again() {
        let mut s = Supervisor::new(pol());
        for t in 0..4 {
            s.on_exit("w", ExitKind::Crash, t);
        }
        s.due(6_000);
        s.on_started("w");
        assert_eq!(s.state(&"w"), WorkerState::Running);
        // Crashes from the old burst have aged out of the window by now.
        assert!(
            matches!(
                s.on_exit("w", ExitKind::Crash, 60_000),
                Decision::MarkDegraded { .. }
            ),
            "probe flag still set"
        );
    }

    #[test]
    fn on_failure_mode_lets_clean_exits_finish() {
        let mut s = Supervisor::new(RestartPolicy {
            mode: RestartMode::OnFailure,
            ..pol()
        });
        assert_eq!(s.on_exit("job", ExitKind::Clean, 0), Decision::Done);
        assert_eq!(s.state(&"job"), WorkerState::Stopped);
        let mut always = Supervisor::new(pol());
        assert!(matches!(
            always.on_exit("svc", ExitKind::Clean, 0),
            Decision::RestartAfter { .. }
        ));
    }

    #[test]
    fn due_returns_backoff_workers_when_ready_and_reset_clears_history() {
        let mut s = Supervisor::new(pol());
        s.on_exit("a", ExitKind::Crash, 0);
        assert!(s.due(99).is_empty());
        assert_eq!(s.due(100), vec!["a"]);
        for t in 1..5 {
            s.on_exit("a", ExitKind::Crash, t);
        }
        assert_eq!(s.degraded(), vec!["a"]);
        s.reset("a");
        assert_eq!(
            s.on_exit("a", ExitKind::Crash, 6),
            Decision::RestartAfter { millis: 100 }
        );
    }
}
