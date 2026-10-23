//! Siar's desktop UI: Slint views plus the bridge to `siar-ui-state`.
//!
//! Architecture (ui-ux-02, plan.md §52-55):
//!
//! ```text
//!  Slint views ──callbacks──▶ bridge ──AppCommand──▶ core task (apps/desktop)
//!       ▲                                                │
//!       └── projection ◀── UiHub (Slices) ◀── update ────┘
//! ```
//!
//! This crate deliberately depends on NO networking, crypto-transport or
//! storage crate. The "UI never calls siar-messaging" rule is therefore
//! enforced by the compiler instead of by convention.

// Hand-written code may not use `unsafe`. Slint's *generated* module does
// internally (vtables), so only that module is exempt.
#![deny(unsafe_code)]

mod bridge;
mod hub;
mod projection;
mod settings;
mod slices;
mod text;

#[allow(unsafe_code, clippy::all, dead_code)]
mod ui {
    slint::include_modules!();
}

pub use hub::{SettingsSink, TicketValidator, UiHub};
pub use settings::{DensityChoice, ThemeChoice, UiSettings};
pub use slices::{EventView, LocalIdentity, Notice, NoticeLevel, Slices};
pub use ui::{
    AppState, Density, MainWindow, NoticeKind, Page, SecurityTab, Theme, ThemeMode, WindowClass,
};

use std::sync::Arc;

use siar_ui_state::AppCommand;
use slint::ComponentHandle;
use tokio::sync::mpsc;

pub struct UiOptions {
    pub settings: UiSettings,
    /// Is this pasted text a usable peer ticket?
    pub validate_ticket: TicketValidator,
    /// Persist appearance settings (called on the UI thread).
    pub on_settings_changed: SettingsSink,
}

impl UiOptions {
    /// Accepts any non-empty ticket and discards settings. For tests.
    pub fn permissive() -> Self {
        Self {
            settings: UiSettings::default(),
            validate_ticket: Arc::new(|t| {
                if t.trim().is_empty() {
                    Err("empty".into())
                } else {
                    Ok(())
                }
            }),
            on_settings_changed: Arc::new(|_| {}),
        }
    }
}

pub struct SiarUi {
    window: MainWindow,
    hub: UiHub,
}

impl SiarUi {
    /// Creates the window on the current (UI) thread.
    pub fn new(
        options: UiOptions,
        commands: mpsc::Sender<AppCommand>,
    ) -> Result<Self, slint::PlatformError> {
        Ok(Self::attach(MainWindow::new()?, options, commands))
    }

    /// Binds the bridge to an existing window — lets headless tests supply
    /// a window from a software-renderer platform.
    pub fn attach(
        window: MainWindow,
        options: UiOptions,
        commands: mpsc::Sender<AppCommand>,
    ) -> Self {
        projection::install(&window);
        bridge::apply_settings(&window, options.settings);
        let hub = UiHub::new(
            window.as_weak(),
            commands,
            options.validate_ticket,
            options.on_settings_changed,
        );
        bridge::install(&window, &hub);
        Self { window, hub }
    }

    /// The thread-safe handle the core uses to push state into the UI.
    pub fn hub(&self) -> UiHub {
        self.hub.clone()
    }

    pub fn window(&self) -> &MainWindow {
        &self.window
    }

    /// Shows the window and runs the event loop until it closes.
    pub fn run(self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}
