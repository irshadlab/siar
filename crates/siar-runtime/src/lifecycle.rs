//! §32-§36, §111-§112: runtime state machine and startup sequence.
//!
//! The legal transitions are a table, not scattered `if`s, so "can a
//! stopped runtime become ready again?" has exactly one answer.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RuntimeState {
    /// Process up, nothing initialised.
    Starting,
    /// §33: crash recovery / migrations running; **no network listeners yet** (§34).
    Recovering,
    /// Recovery done, services starting; network barrier still closed.
    StartingServices,
    Ready,
    /// §140: running with a failed non-critical subsystem.
    Degraded,
    /// §111: reads work, mutating commands are refused.
    ReadOnly,
    /// §112: operator maintenance; mutating commands refused, network quiesced.
    Maintenance,
    /// §35: graceful drain in progress.
    ShuttingDown,
    Stopped,
}

impl RuntimeState {
    /// Network listeners may accept peers (§34).
    #[must_use]
    pub const fn network_open(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }

    /// Mutating commands are accepted.
    #[must_use]
    pub const fn accepts_mutations(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }

    /// Queries and subscriptions are served.
    #[must_use]
    pub const fn accepts_reads(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::Degraded
                | Self::ReadOnly
                | Self::Maintenance
                | Self::Recovering
                | Self::StartingServices
        )
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped)
    }

    /// The transition table.
    #[must_use]
    pub const fn can_become(self, to: Self) -> bool {
        use RuntimeState as S;
        matches!(
            (self, to),
            (S::Starting, S::Recovering | S::ShuttingDown)
                | (
                    S::Recovering,
                    S::StartingServices | S::ReadOnly | S::ShuttingDown
                )
                | (
                    S::StartingServices,
                    S::Ready | S::Degraded | S::ShuttingDown
                )
                | (
                    S::Ready,
                    S::Degraded | S::ReadOnly | S::Maintenance | S::ShuttingDown
                )
                | (
                    S::Degraded,
                    S::Ready | S::ReadOnly | S::Maintenance | S::ShuttingDown
                )
                | (
                    S::ReadOnly,
                    S::Ready | S::Degraded | S::Maintenance | S::ShuttingDown
                )
                | (
                    S::Maintenance,
                    S::Ready | S::Degraded | S::ReadOnly | S::ShuttingDown
                )
                | (S::ShuttingDown, S::Stopped)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: RuntimeState,
    pub to: RuntimeState,
}

/// Shared, observable lifecycle. Cloning shares the same state.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    tx: std::sync::Arc<watch::Sender<RuntimeState>>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl Lifecycle {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tx: std::sync::Arc::new(watch::channel(RuntimeState::Starting).0),
        }
    }

    #[must_use]
    pub fn state(&self) -> RuntimeState {
        *self.tx.borrow()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<RuntimeState> {
        self.tx.subscribe()
    }

    /// Atomically move to `to` if the table allows it from the *current* state.
    pub fn transition(&self, to: RuntimeState) -> Result<(), IllegalTransition> {
        let mut result = Ok(());
        self.tx.send_if_modified(|cur| {
            if *cur == to {
                false
            } else if cur.can_become(to) {
                *cur = to;
                true
            } else {
                result = Err(IllegalTransition { from: *cur, to });
                false
            }
        });
        result
    }

    /// Request shutdown from any non-terminal state. Idempotent.
    pub fn begin_shutdown(&self) {
        let _ = self.transition(RuntimeState::ShuttingDown);
    }

    /// Resolves when the runtime is `ShuttingDown` or `Stopped`.
    pub async fn shutdown_requested(&self) {
        let mut rx = self.subscribe();
        loop {
            if matches!(
                *rx.borrow_and_update(),
                RuntimeState::ShuttingDown | RuntimeState::Stopped
            ) {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RuntimeState as S;

    #[test]
    fn happy_path_and_barrier() {
        let l = Lifecycle::new();
        assert!(!l.state().network_open());
        for s in [S::Recovering, S::StartingServices] {
            l.transition(s).unwrap();
            assert!(
                !l.state().network_open(),
                "§34: no listeners before recovery completes"
            );
        }
        l.transition(S::Ready).unwrap();
        assert!(l.state().network_open() && l.state().accepts_mutations());
        l.begin_shutdown();
        l.transition(S::Stopped).unwrap();
        assert!(l.state().is_terminal());
    }

    #[test]
    fn cannot_skip_recovery_or_leave_stopped() {
        let l = Lifecycle::new();
        assert_eq!(
            l.transition(S::Ready),
            Err(IllegalTransition {
                from: S::Starting,
                to: S::Ready
            })
        );
        l.begin_shutdown();
        l.transition(S::Stopped).unwrap();
        for s in [S::Ready, S::Starting, S::Recovering] {
            assert!(l.transition(s).is_err());
        }
    }

    #[test]
    fn read_only_serves_reads_not_mutations() {
        assert!(S::ReadOnly.accepts_reads() && !S::ReadOnly.accepts_mutations());
        assert!(S::Maintenance.accepts_reads() && !S::Maintenance.network_open());
    }

    #[test]
    fn every_non_terminal_state_can_shut_down() {
        for s in [
            S::Starting,
            S::Recovering,
            S::StartingServices,
            S::Ready,
            S::Degraded,
            S::ReadOnly,
            S::Maintenance,
        ] {
            assert!(s.can_become(S::ShuttingDown), "{s:?}");
        }
        assert!(!S::Stopped.can_become(S::ShuttingDown));
    }

    #[tokio::test]
    async fn shutdown_requested_wakes_waiters() {
        let l = Lifecycle::new();
        let l2 = l.clone();
        let t = tokio::spawn(async move { l2.shutdown_requested().await });
        tokio::task::yield_now().await;
        l.begin_shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(2), t)
            .await
            .unwrap()
            .unwrap();
    }
}
