//! A shutdown signal shared by every background task a `CallSession`
//! owns (`pipeline.rs`'s `spawn_blocking` loops, `android.rs`'s two
//! `tokio::spawn` tasks per direction). One `CallShutdown` per call;
//! every pipeline gets its own [`CancellationToken`] at construction time,
//! and `CallSession::apply_control_event` fires it the moment the signaling
//! state machine reaches `CallState::Ended`.
//!
//! This is a thin, call-shaped wrapper over [`tokio_util::sync::CancellationToken`]
//! (the maintained primitive for exactly this). The earlier hand-made
//! `watch::Sender<bool>` version had a documented hole — a receiver created
//! *after* `signal()` never resolved — that a token does not have:
//! `cancelled()` is ready immediately on an already-cancelled token, so a
//! pipeline spawned during hang-up still stops.

use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default)]
pub struct CallShutdown {
    token: CancellationToken,
}

impl CallShutdown {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A token for one background task to race against. Safe to take at any
    /// time, including after [`Self::signal`]: it is then already cancelled.
    #[must_use]
    pub fn subscribe(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Idempotent. Stops every task holding a token from [`Self::subscribe`].
    pub fn signal(&self) {
        self.token.cancel();
    }

    #[must_use]
    pub fn is_signaled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_token_taken_after_signal_is_already_cancelled() {
        let s = CallShutdown::new();
        s.signal();
        let late = s.subscribe();
        tokio::time::timeout(std::time::Duration::from_millis(100), late.cancelled())
            .await
            .expect("the old watch-based version hung here");
    }

    #[tokio::test]
    async fn signal_is_idempotent_and_reaches_every_subscriber() {
        let s = CallShutdown::new();
        let (a, b) = (s.subscribe(), s.subscribe());
        s.signal();
        s.signal();
        assert!(a.is_cancelled() && b.is_cancelled() && s.is_signaled());
    }

    #[test]
    fn a_task_cancelling_its_own_token_does_not_end_the_call() {
        let s = CallShutdown::new();
        let a = s.subscribe();
        a.cancel();
        assert!(!s.is_signaled());
    }
}
