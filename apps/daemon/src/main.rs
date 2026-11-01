//! `siar-daemon` — the application core with no window.
//!
//! * `siar-daemon run` — host the core under `siar-runtime` (lifecycle,
//!   supervised admin socket, signals, `sd_notify`). Messages keep arriving
//!   and are stored while no UI is open.
//! * `siar-daemon ctl <cmd>` — talk to a running daemon over its local socket.
//! * `siar-daemon unit` — print a hardened `systemd --user` unit;
//!   `install` / `uninstall` manage it through the `service-manager` crate.
//!
//! One process per data folder: the same lock the desktop app takes, so a
//! daemon and a window can never write the same database. (Making the window
//! a *client* of this daemon needs domain-level state sync — see ROADMAP.)

mod app;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use siar_runtime::ipc::{default_socket_path, IpcClient};
use siar_runtime::protocol::{Command, Query, Reply, Request, Role};
use siar_runtime::{CommandId, RestartPolicy, Runtime};

const CTL_COMMANDS: &str = "\
ctl commands:
  status | health | power | metrics | summary | ticket | conversations
  audit                   recent admin actions (kinds only), Admin
  ui-state                what a window would see (via the sync protocol)
  messages <conversation-id>
  pair <ticket>
  send <conversation-id> <text...>
  shutdown";

/// The command line, parsed by `clap` (derive). `--replace` asks a running
/// daemon to stop gracefully first (upgrade handoff); SIGHUP re-reads the
/// config file and reports changes that need a restart.
#[derive(Debug, clap::Parser)]
#[command(name = "siar-daemon", version, about, after_help = CTL_COMMANDS)]
enum Args {
    /// Host the core under siar-runtime (lifecycle, supervised admin socket, signals).
    Run {
        /// Control socket path (default: per-user runtime dir).
        #[arg(long, conflicts_with = "no_socket")]
        socket: Option<PathBuf>,
        /// Do not open a control socket.
        #[arg(long)]
        no_socket: bool,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Stop a running daemon gracefully, then take over.
        #[arg(long)]
        replace: bool,
    },
    /// Talk to a running daemon over its local socket.
    Ctl {
        #[arg(long)]
        socket: Option<PathBuf>,
        /// The ctl command (see below); options go before it.
        #[arg(required = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Print a hardened systemd --user unit.
    Unit,
    /// Install and enable the per-user service.
    Install,
    /// Remove the per-user service.
    Uninstall,
}

#[derive(Debug, PartialEq, Eq)]
enum Cli {
    Run {
        socket: Option<PathBuf>,
        config: Option<PathBuf>,
        replace: bool,
    },
    Ctl {
        socket: PathBuf,
        args: Vec<String>,
    },
    Unit,
    Install,
    Uninstall,
}

impl From<Args> for Cli {
    fn from(a: Args) -> Self {
        match a {
            Args::Run {
                socket,
                no_socket,
                config,
                replace,
            } => Cli::Run {
                socket: (!no_socket)
                    .then(|| socket.unwrap_or_else(|| default_socket_path("daemon"))),
                config,
                replace,
            },
            Args::Ctl { socket, args } => Cli::Ctl {
                socket: socket.unwrap_or_else(|| default_socket_path("daemon")),
                args,
            },
            Args::Unit => Cli::Unit,
            Args::Install => Cli::Install,
            Args::Uninstall => Cli::Uninstall,
        }
    }
}

/// Test seam: parse an argv (without argv[0]) the way `main` does.
#[cfg(test)]
fn parse(args: &[String]) -> Result<Cli, String> {
    use clap::Parser as _;
    Args::try_parse_from(std::iter::once("siar-daemon".to_owned()).chain(args.iter().cloned()))
        .map(Cli::from)
        .map_err(|e| e.to_string())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::from(<Args as clap::Parser>::parse());
    match cli {
        Cli::Install => {
            let exe = std::env::current_exe()?;
            siar_runtime::service::install_user_service("Siar daemon", &exe, &["run"])
                .context("installing the service")?;
            println!("installed {}", siar_runtime::service::SERVICE_LABEL);
            Ok(())
        }
        Cli::Uninstall => {
            siar_runtime::service::uninstall_user_service().context("removing the service")?;
            Ok(())
        }
        Cli::Unit => {
            let exe = std::env::current_exe()?;
            print!(
                "{}",
                siar_runtime::service::systemd_user_unit("Siar daemon", &exe, &["run"])
            );
            Ok(())
        }
        Cli::Ctl { socket, args } => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(ctl(&socket, &args)),
        Cli::Run {
            socket,
            config,
            replace,
        } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("siar-daemon")
                .build()?;
            rt.block_on(run(socket, config, replace))
        }
    }
}

async fn run(socket: Option<PathBuf>, config_path: Option<PathBuf>, replace: bool) -> Result<()> {
    // §85-§88: config first — a bad file stops us before anything is touched,
    // and says everything that is wrong at once.
    let config_path = config_path.or_else(default_config_path);
    let config = load_config(config_path.as_deref())?;

    // §100-§101, §107: take over from a running daemon gracefully. The upgrade
    // lock stops two upgraders deciding to replace the same daemon at once.
    let socket_dir = socket
        .as_deref()
        .and_then(|p| p.parent())
        .map(PathBuf::from);
    let _upgrade = if replace {
        let (Some(sock), Some(dir)) = (socket.as_deref(), socket_dir.as_deref()) else {
            bail!("--replace needs a control socket (don’t combine it with --no-socket)");
        };
        let lock = siar_runtime::instance::UpgradeLock::acquire(dir)
            .map_err(|e| anyhow!("another upgrade is in progress ({e:?})"))?;
        match siar_runtime::instance::request_handoff(sock, Duration::from_secs(30)).await {
            Ok(old) => {
                tracing::info!(old_build = %old.old_build, "previous daemon stopped; taking over")
            }
            Err(siar_runtime::instance::HandoffError::NoDaemon) => {
                tracing::info!("no daemon running; starting fresh")
            }
            Err(e) => bail!("couldn’t take over from the running daemon: {e}"),
        }
        Some(lock)
    } else {
        None
    };

    // One process per data folder (shared with the desktop app).
    let data_dir = siar_app_core::bootstrap::resolve_data_paths()
        .context("resolving the data folder")?
        .data_dir;
    // After a handoff, give the old process a moment to release the lock
    // (it has already stopped serving; this covers platforms where we can't
    // check its pid).
    let _lock = match acquire_lock(&data_dir, replace).await {
        Ok(l) => Some(l),
        Err(siar_backup::LockError::Held) => {
            bail!("Siar is already running with this data folder (the window or another daemon). Use --replace to take over from a daemon.")
        }
        Err(e) => {
            tracing::warn!(error = %e, "couldn’t take the data folder lock; continuing");
            None
        }
    };

    let app = app::DaemonApp::default();
    let runtime = Runtime::builder()
        .app(app.clone())
        .build_id(concat!("siar-daemon ", env!("CARGO_PKG_VERSION")))
        .config(config.clone())
        .power_policy(siar_power::PowerPolicyConfig {
            deployment: siar_power::Deployment::AlwaysOn,
            ..Default::default()
        })
        .build();

    // Recovery = opening storage, identity and the transport endpoint. Only
    // after it succeeds do we start the core and open the control socket
    // (§33-§34: no listeners before recovery completes).
    let booted = Arc::new(Mutex::new(None));
    let (b1, b2) = (booted.clone(), booted.clone());
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(64);
    let hub_tx = cmd_tx.clone();
    let live = app.live.clone();
    let (bus, lifecycle, supervisor) = (
        runtime.bus.clone(),
        runtime.lifecycle.clone(),
        runtime.supervisor.clone(),
    );
    runtime
        .start(
            async move {
                let boot = siar_app_core::bootstrap::bootstrap_messaging()
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                *b1.lock().unwrap() = Some(boot);
                Ok::<(), String>(())
            },
            async move {
                let boot = b2.lock().unwrap().take().ok_or("no bootstrap")?;
                let hub = Arc::new(siar_ui_model::StateHub::new(hub_tx.clone()));
                let core = siar_app_core::core::start(boot, hub).await;
                // Part 16 §25: a window attaches to this — domain-level
                // snapshot + deltas, never UI structs.
                let sync = siar_ui_sync::UiSyncService::new(core.state(), bus.clone());
                {
                    let (svc, lc) = (sync.clone(), lifecycle.clone());
                    supervisor.spawn("ui-sync", false, RestartPolicy::default(), move || {
                        let (svc, lc) = (svc.clone(), lc.clone());
                        async move {
                            svc.run(lc).await;
                            Ok(())
                        }
                    });
                }
                let _ = live.set(app::Live {
                    core: core.clone(),
                    commands: hub_tx,
                    sync,
                });
                tokio::spawn(siar_app_core::core::command_loop(core, cmd_rx));
                Ok::<(), String>(())
            },
        )
        .await
        .map_err(|e| anyhow!("daemon failed to start: {e}"))?;

    let server_cfg = config.server_config();
    if let Some(path) = socket.clone() {
        let rt2 = runtime.clone();
        runtime.supervisor.spawn(
            "control-socket",
            false,
            RestartPolicy::default(),
            move || {
                let (rt2, path, cfg) = (rt2.clone(), path.clone(), server_cfg.clone());
                async move {
                    siar_runtime::ipc::serve_local(rt2, &path, cfg)
                        .await
                        .map_err(|e| e.to_string())
                }
            },
        );
    }
    {
        let lc = runtime.lifecycle.clone();
        tokio::spawn(async move {
            siar_runtime::service::terminate_signal().await;
            tracing::info!("termination signal: shutting down gracefully");
            lc.begin_shutdown();
        });
    }
    // §167-§168: tell upgraders and `ctl` who we are.
    if let Some(dir) = socket_dir.as_deref() {
        let meta = siar_runtime::instance::InstanceMeta::current(
            runtime.bus.instance_id(),
            concat!("siar-daemon ", env!("CARGO_PKG_VERSION")),
        );
        if let Err(e) = siar_runtime::instance::write_meta(dir, &meta) {
            tracing::warn!(error = %e, "couldn’t write the instance file");
        }
    }
    // §97: only pings while genuinely healthy (no-op when no watchdog is configured).
    tokio::spawn(siar_runtime::service::watchdog_loop(
        runtime.lifecycle.clone(),
        runtime.health.clone(),
    ));
    // §87: SIGHUP re-reads the config; cold changes are reported, not faked.
    {
        let (rt, path) = (runtime.clone(), config_path.clone());
        tokio::spawn(siar_runtime::service::on_hangup(
            runtime.lifecycle.clone(),
            move || match load_config(path.as_deref())
                .and_then(|c| rt.reload(c).map_err(Into::into))
            {
                Ok(plan) => {
                    tracing::info!(applied = ?plan.applied, restart_required = ?plan.restart_required, "configuration reloaded")
                }
                Err(e) => {
                    tracing::warn!(error = %e, "configuration reload rejected; keeping the current one")
                }
            },
        ));
    }
    siar_runtime::service::notify_ready();
    tracing::info!("siar-daemon ready");

    runtime.lifecycle.shutdown_requested().await;
    siar_runtime::service::notify_stopping();
    drop(cmd_tx); // lets the core's command loop finish
    let aborted = runtime.shutdown(runtime.shutdown_grace()).await;
    if let Some(live) = app.live.get() {
        live.core.shutdown_transport().await;
    }
    if let Some(dir) = socket_dir.as_deref() {
        siar_runtime::instance::remove_meta(dir);
    }
    if aborted > 0 {
        tracing::warn!(aborted, "forced shutdown aborted stragglers");
    }
    Ok(())
}

async fn acquire_lock(
    dir: &std::path::Path,
    patient: bool,
) -> Result<siar_backup::InstanceLock, siar_backup::LockError> {
    let deadline = std::time::Instant::now()
        + if patient {
            Duration::from_secs(10)
        } else {
            Duration::ZERO
        };
    loop {
        match siar_backup::InstanceLock::acquire(dir) {
            Err(siar_backup::LockError::Held) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => return other,
        }
    }
}

/// `$XDG_CONFIG_HOME/siar/daemon.toml`, else `~/.config/siar/daemon.toml`.
fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("siar").join("daemon.toml"))
}

/// A missing file is fine (defaults); an unreadable or invalid one is not.
fn load_config(path: Option<&std::path::Path>) -> Result<siar_runtime::config::RuntimeConfig> {
    let Some(path) = path else {
        return Ok(Default::default());
    };
    match std::fs::read_to_string(path) {
        Ok(text) => siar_runtime::config::RuntimeConfig::parse(&text)
            .with_context(|| format!("in {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// `println!` panics on a closed pipe (`siar-daemon ctl … | head`); a CLI
/// should just stop quietly.
macro_rules! outln {
    ($($a:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($a)*);
    }};
}

async fn ctl(socket: &std::path::Path, args: &[String]) -> Result<()> {
    let mut c = IpcClient::connect_local(socket, Role::Admin, "siar-daemon ctl")
        .await
        .with_context(|| {
            format!(
                "couldn’t reach a daemon at {} — is `siar-daemon run` going?",
                socket.display()
            )
        })?;
    let rest = args[1..].join(" ");
    let app_query = |name: &str, data: &str| {
        Request::Query(Query::App {
            name: name.into(),
            data: data.as_bytes().to_vec(),
        })
    };
    let req = match args[0].as_str() {
        "status" => Request::Query(Query::Status),
        "health" => Request::Query(Query::Health),
        "power" => Request::Query(Query::Power),
        "metrics" => Request::Query(Query::Metrics),
        "audit" => Request::Query(Query::Audit),
        "summary" | "ticket" | "conversations" => app_query(&args[0], ""),
        "messages" => app_query("messages", &rest),
        "ui-state" => {
            outln!(
                "{}",
                siar_ui_sync::client::snapshot_summary(socket)
                    .await
                    .map_err(|e| anyhow!(e))?
            );
            return Ok(());
        }
        "pair" => Request::Command {
            id: CommandId(uuid::Uuid::new_v4().as_u128()),
            cmd: Command::App {
                name: "pair".into(),
                data: rest.into_bytes(),
            },
        },
        "send" => Request::Command {
            id: CommandId(uuid::Uuid::new_v4().as_u128()),
            cmd: Command::App {
                name: "send".into(),
                data: rest.into_bytes(),
            },
        },
        "shutdown" => Request::Command {
            id: CommandId(uuid::Uuid::new_v4().as_u128()),
            cmd: Command::Shutdown,
        },
        other => bail!("unknown command {other:?}\n\n{CTL_COMMANDS}"),
    };
    match c.request(req).await? {
        Reply::Ok => outln!("ok"),
        Reply::Bytes(b) => outln!("{}", String::from_utf8_lossy(&b)),
        Reply::Status(s) => outln!(
            "state: {:?}\nuptime: {} s\nsubscribers: {}",
            s.state,
            s.uptime_ms / 1000,
            s.subscribers
        ),
        Reply::Health(h) => {
            outln!("overall: {:?}", h.overall);
            for s in h.subsystems {
                outln!("  {}: {:?} {}", s.name, s.state, s.detail);
            }
        }
        Reply::Power(p) => outln!("profile: {:?}\nwaiting: {}", p.profile, p.deferred.total()),
        Reply::Subscribed { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_subcommands_and_socket_flags() {
        assert!(matches!(
            parse(&v(&["run"])),
            Ok(Cli::Run {
                socket: Some(_),
                config: None,
                replace: false
            })
        ));
        assert_eq!(
            parse(&v(&["run", "--no-socket"])),
            Ok(Cli::Run {
                socket: None,
                config: None,
                replace: false
            })
        );
        match parse(&v(&[
            "ctl", "--socket", "/x/s", "send", "abc", "hi", "there",
        ]))
        .unwrap()
        {
            Cli::Ctl { socket, args } => {
                assert_eq!(socket, PathBuf::from("/x/s"));
                assert_eq!(args, v(&["send", "abc", "hi", "there"]));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(parse(&v(&["unit"])), Ok(Cli::Unit));
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse(&[]).is_err());
        assert!(parse(&v(&["bogus"])).is_err());
        assert!(parse(&v(&["ctl"])).is_err());
        assert!(parse(&v(&["ctl", "--no-socket", "status"])).is_err());
        assert!(parse(&v(&["run", "--socket"])).is_err());
    }

    #[test]
    fn hyphens_after_the_command_are_message_text_not_flags() {
        match parse(&v(&["ctl", "send", "c1", "-x", "--not-a-flag"])).unwrap() {
            Cli::Ctl { args, .. } => assert_eq!(args, v(&["send", "c1", "-x", "--not-a-flag"])),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse(&v(&["install"])), Ok(Cli::Install));
        assert!(parse(&v(&["run", "--socket", "/a", "--no-socket"])).is_err());
    }
}
