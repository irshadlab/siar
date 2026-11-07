//! §32-§36, §111-§112: runtime state machine and startup sequence.
//!
//! The legal transitions are a table, not scattered `if`s, so "can a
//! stopped runtime become ready again?" has exactly one answer.

use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

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
    /// §112: operator maintenance; mutating commands refused and
    /// [`network_open`](Self::network_open) is false. (The runtime publishes
    /// that flag; the hosting app must act on it — the daemon's core does
    /// not yet, see `backup.rs`.)
    Maintenance,
    /// §35: graceful drain in progress.
    ShuttingDown,
    Stopped,
}

impl RuntimeState {
    /// Whether network listeners *should* accept peers (§34). A published
    /// predicate: nothing here closes a socket, and the app core does not
    /// currently consult it.
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
///
/// The *state machine* (table, barrier, observable `watch`) is ours; the
/// one-way "shutdown has been requested" signal that every worker, listener
/// and connection waits on is a [`CancellationToken`] — a maintained
/// primitive with child tokens and cancel-safe waiting, instead of a
/// re-implemented wait loop over the state channel.
#[derive(Debug, Clone)]
pub struct Lifecycle {
    tx: std::sync::Arc<watch::Sender<RuntimeState>>,
    shutdown: CancellationToken,
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
            shutdown: CancellationToken::new(),
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
        // `ShuttingDown` is the only way into `Stopped` and is never left, so
        // cancelling on first entry is exact; cancelling twice is a no-op.
        if result.is_ok() && matches!(to, RuntimeState::ShuttingDown | RuntimeState::Stopped) {
            self.shutdown.cancel();
        }
        result
    }

    /// Request shutdown from any non-terminal state. Idempotent.
    pub fn begin_shutdown(&self) {
        let _ = self.transition(RuntimeState::ShuttingDown);
    }

    /// Resolves when the runtime is `ShuttingDown` or `Stopped`. Cancel-safe,
    /// and ready immediately if shutdown was requested before the call.
    pub async fn shutdown_requested(&self) {
        self.shutdown.cancelled().await;
    }

    /// A token that is cancelled when shutdown begins. Hand a
    /// [`CancellationToken::child_token`] of it to any component that needs
    /// to be cancelled *separately* as well (e.g. one connection) without
    /// being able to cancel the runtime.
    #[must_use]
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
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

    #[tokio::test]
    async fn shutdown_requested_is_ready_if_shutdown_already_began() {
        let l = Lifecycle::new();
        l.begin_shutdown();
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            l.shutdown_requested(),
        )
        .await
        .expect("late waiters must not hang");
    }

    #[tokio::test]
    async fn child_tokens_cancel_with_the_runtime_but_not_the_other_way() {
        let l = Lifecycle::new();
        let (a, b) = (
            l.shutdown_token().child_token(),
            l.shutdown_token().child_token(),
        );
        a.cancel(); // one connection going away…
        assert!(!b.is_cancelled() && !l.shutdown_token().is_cancelled());
        l.begin_shutdown(); // …versus the whole runtime going away
        assert!(b.is_cancelled());
    }

    #[test]
    fn an_illegal_transition_does_not_signal_shutdown() {
        let l = Lifecycle::new();
        assert!(l.transition(S::Ready).is_err());
        assert!(!l.shutdown_token().is_cancelled());
    }
}
