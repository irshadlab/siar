//! `UiHub`: the one handle the rest of the app uses to talk to the UI.
//!
//! The state and command channel live in [`siar_ui_model::StateHub`] (no GUI
//! toolkit). This wrapper adds the Slint half: any update schedules at most
//! ONE projection onto the Slint event loop, no matter how many updates
//! arrive before it runs (a burst of incoming messages costs one redraw).
//! The core is handed [`UiHub::state`] — it never sees Slint.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard};

use siar_ui_model::StateHub;
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
    state: Arc<StateHub>,
    dirty: AtomicBool,
    pub(crate) window: Weak<MainWindow>,
    pub(crate) validate_ticket: TicketValidator,
    pub(crate) on_settings_changed: SettingsSink,
    /// No window behind this hub (tests, headless tools): state and the
    /// command channel work, redraw scheduling is skipped.
    detached: bool,
}

#[derive(Clone)]
pub struct UiHub(pub(crate) Arc<HubInner>);

impl UiHub {
    fn build(
        window: Weak<MainWindow>,
        commands: mpsc::Sender<AppCommand>,
        validate_ticket: TicketValidator,
        on_settings_changed: SettingsSink,
        detached: bool,
    ) -> Self {
        let inner = Arc::new(HubInner {
            state: Arc::new(StateHub::new(commands)),
            dirty: AtomicBool::new(false),
            window,
            validate_ticket,
            on_settings_changed,
            detached,
        });
        if !detached {
            // Weak: the state must not keep the window's hub alive.
            let weak = Arc::downgrade(&inner);
            inner.state.set_wake(Arc::new(move || {
                if let Some(inner) = weak.upgrade() {
                    UiHub(inner).request_flush();
                }
            }));
        }
        Self(inner)
    }

    pub(crate) fn new(
        window: Weak<MainWindow>,
        commands: mpsc::Sender<AppCommand>,
        validate_ticket: TicketValidator,
        on_settings_changed: SettingsSink,
    ) -> Self {
        Self::build(
            window,
            commands,
            validate_ticket,
            on_settings_changed,
            false,
        )
    }

    /// A hub with no window, for driving the core headlessly (end-to-end
    /// tests). Everything except drawing works.
    #[doc(hidden)]
    pub fn detached(commands: mpsc::Sender<AppCommand>) -> Self {
        Self::build(
            Weak::default(),
            commands,
            Arc::new(|_| Ok(())),
            Arc::new(|_| {}),
            true,
        )
    }

    /// The toolkit-free state handle: what the application core is given.
    #[must_use]
    pub fn state(&self) -> Arc<StateHub> {
        self.0.state.clone()
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Slices> {
        self.0.state.lock()
    }

    /// Mutate state from any thread and schedule a redraw.
    pub fn update<R>(&self, f: impl FnOnce(&mut Slices) -> R) -> R {
        self.0.state.update(f)
    }

    /// Mutate state WITHOUT redrawing (e.g. every keystroke in the
    /// composer — nothing visible depends on it).
    pub fn update_quiet<R>(&self, f: impl FnOnce(&mut Slices) -> R) -> R {
        self.0.state.update_quiet(f)
    }

    pub fn read<R>(&self, f: impl FnOnce(&Slices) -> R) -> R {
        self.0.state.read(f)
    }

    /// Show a transient notice (validation problems, failures).
    pub fn notify(&self, level: NoticeLevel, text: impl Into<String>) {
        self.0.state.notify(level, text);
    }

    /// Queue a command for the core. Never blocks; a full queue is
    /// reported to the user instead of silently dropping the action.
    pub fn dispatch(&self, command: AppCommand) -> bool {
        self.0.state.dispatch(command)
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
