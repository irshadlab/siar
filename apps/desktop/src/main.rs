//! Siar desktop shell.
//!
//! Slint UI (`siar-ui-slint`) on top of the unchanged messaging core.
//!
//! Threading: Slint owns the main thread and its event loop; everything
//! that touches `siar-messaging` / `siar-transport` runs on a separate
//! multi-thread Tokio runtime. The two meet only at `UiHub` (state into
//! the UI) and one bounded `AppCommand` channel (actions out of it).

mod bootstrap;
mod core;
mod history;
mod peers;
mod read_marks;
mod verification;

#[cfg(test)]
mod e2e_test;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use siar_messaging::PeerTicket;
use siar_ui_slint::{SettingsSink, SiarUi, TicketValidator, UiOptions, UiSettings};
use siar_ui_state::AppCommand;
use tokio::sync::mpsc;

const SETTINGS_FILE: &str = "ui-settings.txt";

/// Write-then-rename, so a crash mid-write can't leave a torn settings file.
fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("siar-core")
        .build()?;

    // Appearance settings are optional: no data dir or a damaged file just
    // means defaults, never a failed start.
    let settings_path = bootstrap::resolve_data_paths()
        .ok()
        .map(|p| p.data_dir.join(SETTINGS_FILE));
    let settings = settings_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| UiSettings::from_text(&t))
        .unwrap_or_default();
    let on_settings_changed: SettingsSink = Arc::new(move |s| {
        if let Some(path) = &settings_path {
            if let Err(e) = write_atomic(path, &s.to_text()) {
                tracing::warn!(error = %e, "couldn't save appearance settings");
            }
        }
    });
    // The UI crate can't see siar-messaging, so it asks us what a valid ticket is.
    let validate_ticket: TicketValidator =
        Arc::new(|t| PeerTicket::decode(t).map(|_| ()).map_err(|e| e.to_string()));

    // Created before the core is up: commands issued during start-up queue
    // here instead of being lost.
    let (command_tx, command_rx) = mpsc::channel::<AppCommand>(64);
    let ui = SiarUi::new(
        UiOptions {
            settings,
            validate_ticket,
            on_settings_changed,
        },
        command_tx,
    )
    .map_err(|e| anyhow!("couldn't create the window: {e}"))?;

    let hub = ui.hub();
    runtime.spawn(async move {
        match bootstrap::bootstrap_messaging().await {
            Ok(boot) => core::run(boot, hub, command_rx).await,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "failed to bootstrap messaging");
                hub.update(|s| {
                    s.startup_error = Some(format!(
                        "Siar couldn’t open its data or start networking.\n\n{e:#}"
                    ));
                });
            }
        }
    });

    let result = ui.run().map_err(|e| anyhow!("UI event loop failed: {e}"));
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}
