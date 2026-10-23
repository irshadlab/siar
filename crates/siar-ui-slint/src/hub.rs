//! `UiHub`: the one handle the rest of the app uses to talk to the UI.
//!
//! Replaces Dioxus `Signal`s. Any thread may call [`UiHub::update`] to
//! mutate the shared [`Slices`]; the hub then schedules at most ONE
//! projection onto the Slint event loop, no matter how many updates
//! arrive before it runs (a burst of incoming messages costs one redraw).
//!
//! Commands go the other way over a bounded channel with `try_send`, so
//! the UI thread can never block on the core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use siar_ui_state::AppCommand;
use slint::Weak;
use tokio::sync::mpsc;

use crate::settings::UiSettings;
use crate::slices::{NoticeLevel, Slices};
use crate::ui::MainWindow;

/// Host-supplied ticket check. The UI crate depends on neither
/// `siar-messaging` nor `siar-transport`, so the host injects the one
/// piece of knowledge it needs (what a valid ticket looks like) and the
/// UI can reject a bad paste synchronously, next to the field.
pub type TicketValidator = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
/// Called (on the UI thread) when the user changes appearance settings.
pub type SettingsSink = Arc<dyn Fn(UiSettings) + Send + Sync>;

pub(crate) struct HubInner {
    slices: Mutex<Slices>,
    dirty: AtomicBool,
    pub(crate) window: Weak<MainWindow>,
    commands: mpsc::Sender<AppCommand>,
    pub(crate) validate_ticket: TicketValidator,
    pub(crate) on_settings_changed: SettingsSink,
    /// No window behind this hub (tests, headless tools): state and the
    /// command channel work, redraw scheduling is skipped.
    detached: bool,
}

#[derive(Clone)]
pub struct UiHub(pub(crate) Arc<HubInner>);

impl UiHub {
    pub(crate) fn new(
        window: Weak<MainWindow>,
        commands: mpsc::Sender<AppCommand>,
        validate_ticket: TicketValidator,
        on_settings_changed: SettingsSink,
    ) -> Self {
        Self(Arc::new(HubInner {
            slices: Mutex::new(Slices::new()),
            dirty: AtomicBool::new(false),
            window,
            commands,
            validate_ticket,
            on_settings_changed,
            detached: false,
        }))
    }

    /// A hub with no window, for driving the core headlessly (end-to-end
    /// tests). Everything except drawing works.
    #[doc(hidden)]
    pub fn detached(commands: mpsc::Sender<AppCommand>) -> Self {
        Self(Arc::new(HubInner {
            slices: Mutex::new(Slices::new()),
            dirty: AtomicBool::new(false),
            window: Weak::default(),
            commands,
            validate_ticket: Arc::new(|_| Ok(())),
            on_settings_changed: Arc::new(|_| {}),
            detached: true,
        }))
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Slices> {
        // A panic while holding the lock must not brick the UI: the data
        // is plain display state, safe to keep using.
        self.0.slices.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Mutate state from any thread and schedule a redraw.
    pub fn update<R>(&self, f: impl FnOnce(&mut Slices) -> R) -> R {
        let out = f(&mut self.lock());
        self.request_flush();
        out
    }

    /// Mutate state WITHOUT redrawing (e.g. every keystroke in the
    /// composer — nothing visible depends on it).
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

    /// Queue a command for the core. Never blocks; a full queue is
    /// reported to the user instead of silently dropping the action.
    pub fn dispatch(&self, command: AppCommand) -> bool {
        match self.0.commands.try_send(command) {
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

    pub(crate) fn request_flush(&self) {
        if self.0.detached {
            return;
        }
        if !self.0.dirty.swap(true, Ordering::AcqRel) {
            let hub = self.clone();
            // Err only when the event loop is gone (app shutting down).
            let _ = slint::invoke_from_event_loop(move || hub.flush_now());
        }
    }

    /// Project the current state into the window. UI thread only. Public
    /// (hidden) so headless tests can drive it without an event loop.
    #[doc(hidden)]
    pub fn flush_now(&self) {
        self.0.dirty.store(false, Ordering::Release);
        crate::projection::flush(self);
    }
}
