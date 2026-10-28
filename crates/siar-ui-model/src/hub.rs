//! `StateHub`: shared UI state + the command channel, with no GUI toolkit.
//!
//! This is the half of the old `UiHub` that the application core needs: any
//! thread may [`StateHub::update`] the shared [`Slices`], and the UI may send
//! commands back over a bounded channel with `try_send` so it can never block
//! on the core. What `UiHub` (in `siar-ui-slint`) adds is only *redraw*
//! scheduling, which it plugs in through [`StateHub::set_wake`]. A daemon or
//! a test simply never sets a wake callback.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use siar_ui_state::AppCommand;
use tokio::sync::mpsc;

use crate::slices::{NoticeLevel, Slices};

/// Called (from whichever thread mutated state) after a visible change.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct StateHub {
    slices: Mutex<Slices>,
    commands: mpsc::Sender<AppCommand>,
    wake: Mutex<Option<Wake>>,
}

impl StateHub {
    #[must_use]
    pub fn new(commands: mpsc::Sender<AppCommand>) -> Self {
        Self {
            slices: Mutex::new(Slices::new()),
            commands,
            wake: Mutex::new(None),
        }
    }

    /// Install the "something visible changed" callback (a renderer's redraw
    /// scheduler). Replaces any previous one.
    pub fn set_wake(&self, wake: Wake) {
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(wake);
    }

    pub fn lock(&self) -> MutexGuard<'_, Slices> {
        // A panic while holding the lock must not brick the UI: the data is
        // plain display state, safe to keep using.
        self.slices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(&self) {
        // Clone out of the lock: the callback must never run under it.
        let w = self
            .wake
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(w) = w {
            w();
        }
    }

    /// Mutate state from any thread and schedule a redraw.
    pub fn update<R>(&self, f: impl FnOnce(&mut Slices) -> R) -> R {
        let out = f(&mut self.lock());
        self.wake();
        out
    }

    /// Mutate state WITHOUT redrawing (e.g. every keystroke in the composer).
    pub fn update_quiet<R>(&self, f: impl FnOnce(&mut Slices) -> R) -> R {
        f(&mut self.lock())
    }

    pub fn read<R>(&self, f: impl FnOnce(&Slices) -> R) -> R {
        f(&self.lock())
    }

    /// Show a transient notice (validation problems, failures).
    pub fn notify(&self, level: NoticeLevel, text: impl Into<String>) {
        let text = text.into();
        self.update(|s| s.notify(level, text));
    }

    /// Queue a command for the core. Never blocks; a full queue is reported
    /// to the user instead of silently dropping the action.
    pub fn dispatch(&self, command: AppCommand) -> bool {
        match self.commands.try_send(command) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("command queue full");
                self.notify(
                    NoticeLevel::Warning,
                    "Siar is busy — try that again in a moment.",
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!("command queue closed — the core task has stopped");
                self.notify(
                    NoticeLevel::Error,
                    "Siar’s background service stopped. Restart the app.",
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn update_wakes_but_quiet_update_and_read_do_not() {
        let (tx, _rx) = mpsc::channel(4);
        let hub = StateHub::new(tx);
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        hub.set_wake(Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        }));
        hub.update(|s| s.ready = true);
        hub.update_quiet(|s| s.open_content = true);
        assert!(hub.read(|s| s.ready && s.open_content));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn works_with_no_wake_callback_as_a_daemon_would() {
        let (tx, _rx) = mpsc::channel(4);
        let hub = StateHub::new(tx);
        hub.update(|s| s.ready = true);
        assert!(hub.read(|s| s.ready));
    }

    #[test]
    fn full_queue_is_reported_not_swallowed() {
        let (tx, _rx) = mpsc::channel(1);
        let hub = StateHub::new(tx);
        let open = || AppCommand::OpenConversation(siar_domain::ConversationId::new());
        assert!(hub.dispatch(open()));
        assert!(
            !hub.dispatch(open()),
            "second send overflows the 1-slot queue"
        );
        assert!(
            hub.read(|s| s.notice.is_some()),
            "the user is told, not left guessing"
        );
    }

    #[test]
    fn closed_channel_is_reported() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let hub = StateHub::new(tx);
        assert!(!hub.dispatch(AppCommand::OpenConversation(
            siar_domain::ConversationId::new()
        )));
        assert!(hub.read(|s| s.notice.is_some()));
    }
}
