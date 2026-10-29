//! `siar-daemon` — the application core with no window.
//!
//! * `siar-daemon run` — host the core under `siar-runtime` (lifecycle,
//!   supervised admin socket, signals, `sd_notify`). Messages keep arriving
//!   and are stored while no UI is open.
//! * `siar-daemon ctl <cmd>` — talk to a running daemon over its local socket.
//! * `siar-daemon unit` — print a hardened `systemd --user` unit.
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

const USAGE: &str = "\
usage:
  siar-daemon run [--socket PATH] [--no-socket] [--config PATH] [--replace]
  siar-daemon ctl [--socket PATH] <command>
      status | health | power | metrics | summary | ticket | conversations
      audit                   recent admin actions (kinds only), Admin
      messages <conversation-id>
      pair <ticket>
      send <conversation-id> <text...>
      shutdown
  siar-daemon unit            print a systemd --user unit

--replace asks a running daemon to stop gracefully first (upgrade handoff).
SIGHUP re-reads the config file; changes that need a restart are reported.
";

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
}

fn parse(args: &[String]) -> Result<Cli, String> {
    let mut it = args.iter().cloned();
    let sub = it.next().ok_or("missing subcommand")?;
    let mut socket = Some(default_socket_path("daemon"));
    let mut config = None;
    let mut replace = false;
    let mut rest = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = Some(PathBuf::from(it.next().ok_or("--socket needs a path")?)),
            "--no-socket" => socket = None,
            "--config" => config = Some(PathBuf::from(it.next().ok_or("--config needs a path")?)),
            "--replace" => replace = true,
            _ => rest.push(a),
        }
    }
    match sub.as_str() {
        "run" => Ok(Cli::Run {
            socket,
            config,
            replace,
        }),
        "ctl" => {
            if rest.is_empty() {
                return Err("ctl needs a command".into());
            }
            Ok(Cli::Ctl {
                socket: socket.ok_or("--no-socket makes no sense for ctl")?,
                args: rest,
            })
        }
        "unit" => Ok(Cli::Unit),
        other => Err(format!("unknown subcommand {other:?}")),
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse(&args).map_err(|e| anyhow!("{e}\n\n{USAGE}"))?;
    match cli {
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
                let _ = live.set(app::Live {
                    core: core.clone(),
                    commands: hub_tx,
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
                    siar_runtime::ipc::serve_unix(rt2, &path, cfg)
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
    siar_runtime::service::sd_notify("READY=1");
    tracing::info!("siar-daemon ready");

    runtime.lifecycle.shutdown_requested().await;
    siar_runtime::service::sd_notify("STOPPING=1");
    drop(cmd_tx); // lets the core's command loop finish
    let aborted = runtime.shutdown(runtime.shutdown_grace()).await;
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
    let mut c = IpcClient::connect_unix(socket, Role::Admin, "siar-daemon ctl")
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
        other => bail!("unknown command {other:?}\n\n{USAGE}"),
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
}
