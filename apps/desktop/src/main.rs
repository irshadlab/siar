//! Siar desktop shell.
//!
//! Slint UI (`siar-ui-slint`) as a thin client of `siar-daemon`. The window
//! never opens the database, keys or network identity: the daemon owns them,
//! so there is nothing for two processes to fight over. On start the window
//! connects to the daemon's control socket and, if none is running, starts
//! one (found beside this program, never via `PATH`).
//!
//! Threading: Slint owns the main thread and its event loop; everything else
//! runs on a separate multi-thread Tokio runtime. The two meet only at
//! `UiHub` (state into the UI) and one bounded `AppCommand` channel (actions
//! out of it).

mod cli;

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

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let socket = take_socket(&mut args);
    match cli::parse(&args) {
        Ok(cli::Command::Run) => run_app(socket),
        Ok(command) => cli::execute(command),
        Err(why) => {
            eprintln!("{why}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    }
}

/// `--socket PATH`: use a daemon at a non-default control socket. Removes the
/// flag so the normal parser never sees it. (`--connect [PATH]` is accepted as
/// an alias; it no longer changes behaviour because connecting is the default.)
fn take_socket(args: &mut Vec<String>) -> std::path::PathBuf {
    let default = || siar_runtime::ipc::default_socket_path("daemon");
    let Some(i) = args
        .iter()
        .position(|a| a == "--socket" || a == "--connect")
    else {
        return default();
    };
    args.remove(i);
    let explicit = args.get(i).filter(|a| !a.starts_with('-')).cloned();
    if explicit.is_some() {
        args.remove(i);
    }
    explicit.map_or_else(default, std::path::PathBuf::from)
}

/// Reach the daemon, starting it if needed. The message on failure is what the
/// user sees in the window, so it says what to do next.
async fn reach_daemon(socket: &std::path::Path) -> Result<(), String> {
    let s = socket.to_path_buf();
    siar_runtime::ensure::ensure_daemon(
        socket,
        siar_runtime::Role::Operator,
        "siar-desktop",
        move || async move { siar_runtime::ensure::spawn_daemon_beside_exe(&s) },
        Duration::from_secs(20),
    )
    .await
    .map(drop)
    .map_err(|e| {
        format!(
            "Siar couldn’t reach its background service.\n\n{e}\n\n\
             Start it yourself with `siar-daemon run`, then reopen this window."
        )
    })
}

fn run_app(socket: std::path::PathBuf) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("siar-ui")
        .build()?;

    // Appearance settings are optional: no data dir or a damaged file just
    // means defaults, never a failed start. (Paths only; nothing is opened.)
    let settings_path = siar_app_core::bootstrap::resolve_data_paths()
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

    // Created before the daemon is reached: commands issued during start-up
    // queue here instead of being lost.
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
    let stop = tokio_util::sync::CancellationToken::new();
    {
        let (state, stop) = (hub.state(), stop.clone());
        runtime.spawn(async move {
            tracing::info!(socket = %socket.display(), "connecting to the Siar daemon");
            match reach_daemon(&socket).await {
                Ok(()) => siar_ui_sync::run_remote(socket, state, command_rx, stop).await,
                Err(message) => state.update(|s| s.startup_error = Some(message)),
            }
        });
    }

    let result = ui.run().map_err(|e| anyhow!("UI event loop failed: {e}"));
    stop.cancel();
    runtime.shutdown_timeout(Duration::from_secs(2));
    result
}
