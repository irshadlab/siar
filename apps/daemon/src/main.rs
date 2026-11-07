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

mod admin;
mod app;
mod backup;
mod notify;
mod roles;
#[cfg(windows)]
mod winservice;

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
  maintenance <on|off>    quiesce / reopen the daemon (needed before `restore`), Admin
  read-only <on|off>      refuse / allow mutations, Admin
  provision-status        provisioning state; the challenge while unprovisioned, Admin
  provision <file> <sig>  enrol this node with a signed payload (see `siar-root payload`), Admin
  diagnostics             redacted diagnostic bundle (versions, health, config, metrics, errors), Admin
  backup <abs-dir>        consistent backup into <abs-dir> (quiesces the daemon meanwhile), Admin
  restore <abs-dir>       restore from <abs-dir>; daemon must be in maintenance first, Admin
  ui-state                what a window would see (via the sync protocol)
  messages <conversation-id>
  pair <ticket>
  pairwise-publish        this device's post-quantum key package: <device-id> <base64>
  pairwise-open <conversation-id> <peer-ticket> <peer-device-id> <key-package>
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
        /// Print the reply as one RON value (Rust Object Notation: lossless
        /// for every reply type, parseable with `ron::from_str::<Reply>`)
        /// instead of text.
        #[arg(long)]
        ron: bool,
        /// The ctl command (see below); options go before it.
        #[arg(required = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Create an operator key for remote administration; prints its public
    /// id (put that in the node's `[admin]` config).
    AdminKeygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Administer a node remotely over `comm-admin/1` (Iroh). One of: health,
    /// status, metrics, diagnostics, power, provision-status, audit,
    /// maintenance on|off, read-only on|off, backup <dir>, restore <dir>,
    /// shutdown. There is no remote shell.
    Admin {
        /// Your operator key file (see `admin-keygen`).
        #[arg(long)]
        key_file: PathBuf,
        /// The node's direct address (repeatable). Needed unless --discovery.
        #[arg(long = "addr")]
        addrs: Vec<std::net::SocketAddr>,
        /// Look the node up through Iroh discovery instead of --addr.
        #[arg(long)]
        discovery: bool,
        /// The node's admin endpoint id (64 hex chars; the node logs it and
        /// writes it to `<data>/admin/endpoint.id`).
        node: String,
        #[arg(required = true, trailing_var_arg = true)]
        op: Vec<String>,
    },
    /// Verify the durable audit log in a folder (the node's `<data>/audit`):
    /// every line's hash chain and the sequence numbers. Exits non-zero if it
    /// does not verify. Prints the chain head to record off-box.
    AuditVerify {
        /// The audit folder (contains `audit.log`, `audit.log.1`, …).
        dir: PathBuf,
    },
    /// Print a hardened systemd --user unit.
    Unit,
    /// Install and enable the per-user service.
    Install,
    /// Remove the per-user service.
    Uninstall,
    /// Entry point the Windows Service Control Manager launches (not for
    /// interactive use; `install` registers it).
    #[command(hide = true)]
    ServiceRun,
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
        ron: bool,
        args: Vec<String>,
    },
    AuditVerify {
        dir: PathBuf,
    },
    AdminKeygen {
        out: PathBuf,
    },
    Admin {
        key_file: PathBuf,
        addrs: Vec<std::net::SocketAddr>,
        discovery: bool,
        node: String,
        op: Vec<String>,
    },
    Unit,
    Install,
    Uninstall,
    ServiceRun,
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
            Args::Ctl { socket, ron, args } => Cli::Ctl {
                socket: socket.unwrap_or_else(|| default_socket_path("daemon")),
                ron,
                args,
            },
            Args::AuditVerify { dir } => Cli::AuditVerify { dir },
            Args::AdminKeygen { out } => Cli::AdminKeygen { out },
            Args::Admin {
                key_file,
                addrs,
                discovery,
                node,
                op,
            } => Cli::Admin {
                key_file,
                addrs,
                discovery,
                node,
                op,
            },
            Args::Unit => Cli::Unit,
            Args::Install => Cli::Install,
            Args::Uninstall => Cli::Uninstall,
            Args::ServiceRun => Cli::ServiceRun,
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
    let cli = Cli::from(<Args as clap::Parser>::parse());
    // One-shot operator commands (ctl, admin, admin-keygen, unit, …) print
    // their answer, not the transport library's connection chatter.
    let serving = matches!(cli, Cli::Run { .. } | Cli::ServiceRun);
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(if serving {
            tracing::Level::INFO
        } else {
            tracing::Level::WARN
        })
        .init();
    match cli {
        Cli::Install => {
            let exe = std::env::current_exe()?;
            // Windows: the SCM must launch the service-control entry point.
            let entry = if cfg!(windows) { "service-run" } else { "run" };
            siar_runtime::service::install_user_service("Siar daemon", &exe, &[entry])
                .context("installing the service")?;
            println!("installed {}", siar_runtime::service::SERVICE_LABEL);
            Ok(())
        }
        Cli::Uninstall => {
            siar_runtime::service::uninstall_user_service().context("removing the service")?;
            Ok(())
        }
        Cli::ServiceRun => service_run(),
        Cli::Unit => {
            let exe = std::env::current_exe()?;
            print!(
                "{}",
                siar_runtime::service::systemd_user_unit("Siar daemon", &exe, &["run"])
            );
            Ok(())
        }
        Cli::AuditVerify { dir } => {
            let rep = siar_runtime::audit_file::FileSink::verify_dir(
                &dir,
                siar_runtime::audit_file::FileLimits::default().keep,
            )
            .with_context(|| format!("reading {}", dir.display()))?;
            match &rep.problem {
                None => {
                    println!(
                        "OK: {} entries, seq {}..{}",
                        rep.entries,
                        rep.first_seq.map_or("-".into(), |v| v.to_string()),
                        rep.last_seq.map_or("-".into(), |v| v.to_string())
                    );
                    if let (Some(seq), Some(head)) = (rep.last_seq, &rep.head) {
                        println!(
                            "head seq={seq} chain={head}  (record this off-box to detect tail truncation)"
                        );
                    }
                    Ok(())
                }
                Some((seg, line, why)) => bail!(
                    "audit log does NOT verify: {seg} line {line}: {why} ({} entries were fine before that)",
                    rep.entries
                ),
            }
        }
        Cli::AdminKeygen { out } => {
            println!("{}", admin::keygen(&out)?);
            eprintln!(
                "key written to {} (mode 600). Put the id above in the node's daemon.toml:\n\
                 [[admin.operators]]\nkey = \"<id>\"\nrole = \"viewer|operator|security-admin|owner\"\nlabel = \"<name>\"",
                out.display()
            );
            Ok(())
        }
        Cli::Admin {
            key_file,
            addrs,
            discovery,
            node,
            op,
        } => {
            let op = admin::parse_op(&op)?;
            let out = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(admin::remote(&key_file, &node, &addrs, discovery, op))?;
            println!("{out}");
            Ok(())
        }
        Cli::Ctl { socket, ron, args } => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(ctl(&socket, ron, &args)),
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

#[cfg(windows)]
fn service_run() -> Result<()> {
    winservice::run().context("starting under the Windows Service Control Manager")
}

#[cfg(not(windows))]
fn service_run() -> Result<()> {
    bail!("service-run is the Windows Service Control Manager entry point; use `run` here")
}

/// §151-§158: announce new messages and security alerts on the desktop while
/// nobody is looking. `SIAR_NOTIFICATIONS=off|private|preview` (default
/// `private`: no sender, no text).
fn install_notifications(sync: &siar_ui_sync::UiSyncService, bus: &siar_runtime::EventBus) {
    use notify::{DesktopSink, NotificationEngine, NotifyPolicy, Sink as _};
    let policy = NotifyPolicy::from_env_value(std::env::var("SIAR_NOTIFICATIONS").ok().as_deref());
    if !policy.enabled {
        return;
    }
    let engine = std::sync::Mutex::new(NotificationEngine::new(policy));
    let sink = std::sync::Arc::new(DesktopSink::default());
    let bus = bus.clone();
    sync.set_observer(std::sync::Arc::new(move |hub| {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        // Decide under the state lock; show (D-Bus round trip) outside it.
        let attached = bus.subscriber_count();
        let due = hub.read(|s| {
            engine
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .observe(s, attached, now_ms)
        });
        for n in due {
            let sink = sink.clone();
            tokio::task::spawn_blocking(move || sink.show(&n));
        }
    }));
}

async fn run(socket: Option<PathBuf>, config_path: Option<PathBuf>, replace: bool) -> Result<()> {
    // §85-§88: config first — a bad file stops us before anything is touched,
    // and says everything that is wrong at once.
    let config_path = config_path.or_else(default_config_path);
    let local_config = load_config(config_path.as_deref())?;

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
    let lock = match acquire_lock(&data_dir, replace).await {
        Ok(l) => Some(Arc::new(l)),
        Err(siar_backup::LockError::Held) => {
            bail!("Siar is already running with this data folder (the window or another daemon). Use --replace to take over from a daemon.")
        }
        Err(e) => {
            tracing::warn!(error = %e, "couldn’t take the data folder lock; continuing");
            None
        }
    };

    // §115: a restore staged by `ctl restore` is applied now — after the lock,
    // before the database is opened. A failure refuses to start: carrying on
    // could create a fresh identity next to the data that is still there.
    if let Some(l) = &lock {
        match backup::apply_pending_restore(&data_dir, l) {
            Ok(Some(previous)) => tracing::warn!(
                previous = %previous.display(),
                "restore applied; the previous data was kept in that folder"
            ),
            Ok(None) => {}
            Err(e) => bail!(
                "a staged restore could not be applied: {e:#}. Nothing was lost. \
                 Fix the cause, or remove the *.restore-pending marker next to the data folder to start without it."
            ),
        }
    }

    // §72-§73: with `node.require_provisioning` the daemon starts enrolment-only
    // (IPC + the provisioning command; no core, no network) until an admin
    // key signs a payload for this node.
    let provisioner = if local_config.node.require_provisioning {
        let verifier =
            siar_runtime::node::Ed25519AdminVerifier::from_hex_keys(&local_config.node.admin_keys)
                .map_err(|e| anyhow!("node.admin_keys: {e}"))?;
        let (p, warning) = siar_runtime::node::Provisioner::open(&data_dir.join("node"), verifier)
            .context("opening the node provisioning state")?;
        if let Some(w) = warning {
            tracing::warn!("{w}");
        }
        Some(Arc::new(p))
    } else {
        None
    };
    // What this run is: enrolment-only (unprovisioned), a node whose role
    // runs no chat core, or a full node. The role decides — a relay or DTN
    // node must not create a chat identity or open a chat database.
    let started_unprovisioned = provisioner
        .as_ref()
        .is_some_and(|p| matches!(p.state(), siar_runtime::node::Provisioning::Unprovisioned));
    let boot_core = match &provisioner {
        None => true,
        Some(p) => p.allows(siar_runtime::node::Feature::Messaging),
    };
    // The admin's config (if the payload carried one) laid over the local
    // file, minus [node] and [secrets], which stay local.
    let config = siar_runtime::node::overlay_config(
        &local_config,
        provisioner.as_ref().and_then(|p| p.applied_config()),
    );
    if let Some(p) = &provisioner {
        if started_unprovisioned {
            let status = siar_runtime::node::ProvisioningService::status(p.as_ref());
            tracing::warn!(
                challenge = status.challenge_hex.as_deref().unwrap_or(""),
                "node is UNPROVISIONED: running the enrolment listener only. Show this challenge to the admin."
            );
        } else if let siar_runtime::node::Provisioning::Provisioned(done) = p.state() {
            let plan = roles::plan_for(done.role, &data_dir);
            match (boot_core, &plan) {
                (true, roles::RolePlan::CoreOnly) => {
                    tracing::info!(role = %done.role, "provisioned; starting the messaging core");
                }
                (true, roles::RolePlan::Child(_)) => tracing::info!(
                    role = %done.role,
                    "provisioned; starting the messaging core and the relay engine"
                ),
                (false, roles::RolePlan::Child(_)) => tracing::info!(
                    role = %done.role,
                    "provisioned; this role runs no messaging (no chat identity, no chat database) — starting the relay engine only"
                ),
                (_, roles::RolePlan::Unavailable(why)) => tracing::warn!(
                    role = %done.role,
                    "provisioned, but {why}: nothing is started for this role"
                ),
                (false, roles::RolePlan::CoreOnly) => tracing::warn!(
                    role = %done.role,
                    "provisioned, but this role runs nothing in this binary"
                ),
            }
        }
    }

    let backup_svc = backup::DaemonBackup::new(
        data_dir.clone(),
        lock.clone(),
        config.secrets.backup_passphrase.clone(),
    );
    let backup_db_slot = backup_svc.db_slot();

    let app = app::DaemonApp::default();
    let mut builder = Runtime::builder()
        .app(app.clone())
        .backup(backup_svc)
        .build_id(concat!("siar-daemon ", env!("CARGO_PKG_VERSION")))
        .config(config.clone())
        .power_policy(siar_power::PowerPolicyConfig {
            deployment: siar_power::Deployment::AlwaysOn,
            ..Default::default()
        });
    if let Some(p) = &provisioner {
        builder = builder.provisioning(p.clone());
    }
    let runtime = builder.build();

    // §161-§162, §70: the audit log is also written to a durable, hash-chained
    // file (never rewound by a restore, never archived). A failure here must
    // not stop the node: it keeps auditing in memory and says so loudly.
    match runtime.audit.attach_file(
        &data_dir.join("audit"),
        siar_runtime::audit_file::FileLimits::default(),
    ) {
        Ok(None) => {}
        Ok(Some(warning)) => tracing::error!("{warning}"),
        Err(e) => tracing::error!("durable audit log unavailable ({e}); auditing in memory only"),
    }

    // §66-§71, §159: remote administration over its own Iroh endpoint, its own
    // key, its own protocol. Off unless `[admin] enabled`.
    let admin_server = if config.admin.enabled {
        let table = admin::operator_table(&config.admin)?;
        let key = siar_admin::keys::load_or_create(&data_dir.join("admin").join("endpoint.key"))
            .context("loading the admin endpoint key")?;
        let id_hex = siar_admin::keys::public_hex(&key);
        let cfg = siar_admin::AdminServerConfig {
            discovery: config.admin.discovery,
            bind_addr: config
                .admin
                .bind
                .as_deref()
                .map(str::parse)
                .transpose()
                .context("admin.bind")?,
            ..siar_admin::AdminServerConfig::default()
        };
        let server = siar_admin::AdminServer::spawn(
            cfg,
            key,
            table,
            admin::DaemonAdmin::new(runtime.clone()),
        )
        .await
        .map_err(|e| anyhow!("starting the admin endpoint: {e}"))?;
        std::fs::write(data_dir.join("admin").join("endpoint.id"), &id_hex)
            .context("writing admin/endpoint.id")?;
        let addrs: Vec<String> = server
            .addr()
            .addrs
            .iter()
            .filter_map(|a| match a {
                iroh::TransportAddr::Ip(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        std::fs::write(
            data_dir.join("admin").join("endpoint.addrs"),
            addrs.join("\n"),
        )
        .context("writing admin/endpoint.addrs")?;
        tracing::info!(
            endpoint = %id_hex,
            addrs = %addrs.join(","),
            operators = config.admin.operators.len(),
            discovery = config.admin.discovery,
            "remote admin (comm-admin/1) listening"
        );
        Some(server)
    } else {
        None
    };

    // §96: a panic leaves a redacted crash report (location + backtrace,
    // never the panic message) next to the data, newest five kept. The
    // health read must not block inside a panic, hence `try_report`.
    {
        let health = runtime.health.clone();
        siar_runtime::diag::install_panic_hook(
            data_dir.join("crash"),
            concat!("siar-daemon ", env!("CARGO_PKG_VERSION")).to_owned(),
            move || {
                health
                    .try_report()
                    .unwrap_or_else(|| siar_runtime::health::HealthReport {
                        overall: siar_runtime::health::HealthState::Unavailable,
                        subsystems: Vec::new(),
                    })
            },
            || {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            },
        );
    }

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
                if !boot_core {
                    return Ok::<(), String>(()); // enrolment-only: nothing to recover
                }
                let boot = siar_app_core::bootstrap::bootstrap_messaging()
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                // §113: the backup service snapshots this very handle.
                let _ = backup_db_slot.set(boot.db.clone());
                *b1.lock().unwrap() = Some(boot);
                Ok::<(), String>(())
            },
            async move {
                if !boot_core {
                    return Ok::<(), String>(());
                }
                let boot = b2.lock().unwrap().take().ok_or("no bootstrap")?;
                let hub = Arc::new(siar_ui_model::StateHub::new(hub_tx.clone()));
                let core = siar_app_core::core::start(boot, hub).await;
                // Part 16 §25: a window attaches to this — domain-level
                // snapshot + deltas, never UI structs.
                let sync = siar_ui_sync::UiSyncService::new(core.state(), bus.clone());
                install_notifications(&sync, &bus);
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
        let reload_prov = provisioner.clone();
        tokio::spawn(siar_runtime::service::on_hangup(
            runtime.lifecycle.clone(),
            move || match load_config(path.as_deref())
                .map(|local| {
                    siar_runtime::node::overlay_config(
                        &local,
                        reload_prov.as_ref().and_then(|p| p.applied_config()),
                    )
                })
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

    // What the role runs beyond the chat core (e.g. the relay engine). Only on
    // a normal start: an enrolment run exits 75 and restarts into the role.
    if !started_unprovisioned {
        if let Some(siar_runtime::node::Provisioning::Provisioned(done)) =
            provisioner.as_ref().map(|p| p.state())
        {
            if let roles::RolePlan::Child(launch) = roles::plan_for(done.role, &data_dir) {
                roles::supervise(&runtime, launch);
            }
        }
    }
    runtime.lifecycle.shutdown_requested().await;
    if let Some(server) = admin_server {
        server.shutdown().await;
    }
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
    // Enrolment succeeded: the role's features are fixed at startup, so exit
    // non-zero (75, EX_TEMPFAIL) — the generated unit has `Restart=on-failure`,
    // so the service manager brings the node back up provisioned.
    if started_unprovisioned
        && provisioner
            .as_ref()
            .is_some_and(|p| matches!(p.state(), siar_runtime::node::Provisioning::Provisioned(_)))
    {
        tracing::info!("provisioned; exiting 75 so the service manager restarts into the new role");
        std::process::exit(75);
    }
    Ok(())
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
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

async fn ctl(socket: &std::path::Path, ron: bool, args: &[String]) -> Result<()> {
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
        "diagnostics" => Request::Query(Query::Diagnostics),
        "maintenance" | "read-only" => {
            let on = match args.get(1).map(String::as_str) {
                Some("on") if args.len() == 2 => true,
                Some("off") if args.len() == 2 => false,
                _ => bail!("usage: ctl {} <on|off>", args[0]),
            };
            Request::Command {
                id: CommandId(uuid::Uuid::new_v4().as_u128()),
                cmd: if args[0] == "maintenance" {
                    Command::SetMaintenance(on)
                } else {
                    Command::SetReadOnly(on)
                },
            }
        }
        "provision-status" => Request::Query(Query::ProvisionStatus),
        "provision" => {
            if args.len() != 3 {
                bail!("usage: ctl provision <payload-file> <signature-hex>");
            }
            let payload =
                std::fs::read(&args[1]).with_context(|| format!("reading {}", args[1]))?;
            let signature = decode_hex(&args[2]).ok_or_else(|| anyhow!("signature is not hex"))?;
            Request::Command {
                id: CommandId(uuid::Uuid::new_v4().as_u128()),
                cmd: Command::Provision { payload, signature },
            }
        }
        "backup" | "restore" => {
            if args.len() != 2 {
                bail!("usage: ctl {} <absolute-directory>", args[0]);
            }
            let id = CommandId(uuid::Uuid::new_v4().as_u128());
            let cmd = if args[0] == "backup" {
                Command::CreateBackup {
                    target: args[1].clone(),
                }
            } else {
                Command::RestoreBackup {
                    source: args[1].clone(),
                }
            };
            Request::Command { id, cmd }
        }
        "summary" | "ticket" | "conversations" | "pairwise-publish" => app_query(&args[0], ""),
        "pairwise-open" => Request::Command {
            id: CommandId(uuid::Uuid::new_v4().as_u128()),
            cmd: Command::App {
                name: "pairwise-open".into(),
                data: rest.into_bytes(),
            },
        },
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
    let text = render_reply(&c.request(req).await?, ron);
    if !text.is_empty() {
        outln!("{text}");
    }
    Ok(())
}

fn to_ron<T: serde::Serialize>(v: &T) -> String {
    ron::ser::to_string_pretty(v, ron::ser::PrettyConfig::default())
        .unwrap_or_else(|e| format!("/* unrepresentable: {e} */"))
}

fn ron_str(t: &str) -> String {
    ron::ser::to_string(t).unwrap_or_default()
}

/// How `ctl` prints a reply. `ron` yields exactly one RON value — enums,
/// options and unit variants come through losslessly (TOML could not: its
/// root must be a table, and it has no null), and it round-trips through
/// `ron::from_str::<Reply>`. A `Bytes` reply that is UTF-8 (every app query)
/// is shown as a string, not as a list of numbers.
fn render_reply(reply: &Reply, ron: bool) -> String {
    if ron {
        return match reply {
            Reply::Bytes(b) => match std::str::from_utf8(b) {
                Ok(t) => format!("Bytes({})", ron_str(t)),
                Err(_) => to_ron(reply),
            },
            other => to_ron(other),
        };
    }
    match reply {
        Reply::Ok => "ok".into(),
        Reply::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Reply::Status(s) => format!(
            "state: {:?}\nuptime: {} s\nsubscribers: {}",
            s.state,
            s.uptime_ms / 1000,
            s.subscribers
        ),
        Reply::Health(h) => {
            let mut out = format!("overall: {:?}", h.overall);
            for s in &h.subsystems {
                out.push_str(&format!("\n  {}: {:?} {}", s.name, s.state, s.detail));
            }
            out
        }
        Reply::Power(p) => format!("profile: {:?}\nwaiting: {}", p.profile, p.deferred.total()),
        Reply::Subscribed { .. } => String::new(),
    }
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
            Cli::Ctl { socket, args, ron } => {
                assert!(!ron);
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

    #[test]
    fn ron_output_is_lossless_and_shows_utf8_bytes_as_text() {
        assert_eq!(render_reply(&Reply::Ok, false), "ok");
        assert_eq!(render_reply(&Reply::Ok, true), "Ok");
        assert_eq!(
            render_reply(&Reply::Bytes(b"plain".to_vec()), true),
            r#"Bytes("plain")"#
        );
        for r in [
            Reply::Ok,
            Reply::Subscribed { next_seq: 3 },
            Reply::Bytes(vec![0xff, 0xfe, 1]), // not UTF-8: stays a byte list
        ] {
            let text = render_reply(&r, true);
            assert_eq!(ron::from_str::<Reply>(&text).unwrap(), r, "{text}");
        }
        assert_eq!(render_reply(&Reply::Subscribed { next_seq: 3 }, false), "");
    }

    #[test]
    fn ctl_ron_flag_parses_before_the_command() {
        let c = parse(&["ctl".into(), "--ron".into(), "status".into()]).unwrap();
        assert!(matches!(c, Cli::Ctl { ron: true, .. }));
        let c = parse(&["ctl".into(), "status".into()]).unwrap();
        assert!(matches!(c, Cli::Ctl { ron: false, .. }));
    }
}
