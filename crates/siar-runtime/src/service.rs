//! §37-§39, §99, §174-§175: service-manager integration.
//!
//! * [`notify`] (the `sd-notify` crate) — readiness/stopping notification for `Type=notify`
//!   systemd units (§99). A no-op when `NOTIFY_SOCKET` is unset, so the same
//!   binary runs fine under a shell.
//! * [`systemd_user_unit`] / [`desktop_autostart_entry`] — generated unit
//!   text (§38 "Linux user service", §174 autostart). **User** services:
//!   no root needed (§39).

use std::io;
use std::path::Path;

/// What to tell the service manager (re-exported from the maintained
/// [`sd_notify`] crate so callers need no extra dependency).
#[cfg(unix)]
pub use sd_notify::NotifyState;

/// Send state changes to the service manager. Returns `true` if a datagram
/// was actually sent; `false` when `NOTIFY_SOCKET` is unset (running under a
/// shell) or the send failed. Never an error: a daemon must not die because
/// systemd is not listening.
#[cfg(unix)]
pub fn notify(state: &[NotifyState<'_>]) -> bool {
    std::env::var_os("NOTIFY_SOCKET").is_some() && sd_notify::notify(state).is_ok()
}

/// `READY=1` (§99). No-op off Unix.
pub fn notify_ready() -> bool {
    #[cfg(unix)]
    {
        notify(&[NotifyState::Ready])
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// `STOPPING=1` (§99). No-op off Unix.
pub fn notify_stopping() -> bool {
    #[cfg(unix)]
    {
        notify(&[NotifyState::Stopping])
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Reverse-DNS service label used for every service manager.
pub const SERVICE_LABEL: &str = "org.siar.daemon";

/// The install request for a **per-user** service. On systemd the unit text
/// is our hardened [`systemd_user_unit`] (the crate's default template has no
/// `NoNewPrivileges`/sandboxing); on launchd and others the crate's own
/// template is used.
///
/// # Errors
/// If the label does not parse (cannot happen for [`SERVICE_LABEL`]).
pub fn user_service_ctx(
    description: &str,
    exe: &Path,
    args: &[&str],
    systemd: bool,
) -> io::Result<service_manager::ServiceInstallCtx> {
    let label: service_manager::ServiceLabel = SERVICE_LABEL
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?;
    Ok(service_manager::ServiceInstallCtx {
        label,
        program: exe.to_path_buf(),
        args: args.iter().map(Into::into).collect(),
        contents: systemd.then(|| systemd_user_unit(description, exe, args)),
        username: None,
        working_directory: None,
        environment: None,
        autostart: true,
        restart_policy: service_manager::RestartPolicy::OnFailure {
            delay_secs: Some(2),
            max_retries: None,
            reset_after_secs: None,
        },
    })
}

fn user_manager() -> io::Result<(Box<dyn service_manager::ServiceManager>, bool)> {
    use service_manager::{ServiceLevel, ServiceManager, ServiceManagerKind};
    let kind = ServiceManagerKind::native()?;
    let mut m = <dyn ServiceManager>::target(kind);
    // Windows has no per-user services: the SCM registers machine-wide ones
    // (needs an elevated prompt). The binary must then be started with the
    // `service-run` entry point — see `apps/daemon`'s `winservice` module.
    m.set_level(if cfg!(windows) {
        ServiceLevel::System
    } else {
        ServiceLevel::User
    })?;
    Ok((m, kind == ServiceManagerKind::Systemd))
}

/// Install (and enable) the daemon as a per-user service.
///
/// # Errors
/// No supported service manager, or it refused.
pub fn install_user_service(description: &str, exe: &Path, args: &[&str]) -> io::Result<()> {
    let (m, systemd) = user_manager()?;
    m.install(user_service_ctx(description, exe, args, systemd)?)
}

/// Remove the per-user service installed by [`install_user_service`].
///
/// # Errors
/// No supported service manager, or it refused.
pub fn uninstall_user_service() -> io::Result<()> {
    let (m, _) = user_manager()?;
    m.uninstall(service_manager::ServiceUninstallCtx {
        label: SERVICE_LABEL
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?,
    })
}

static EXTERNAL_STOP: std::sync::OnceLock<tokio_util::sync::CancellationToken> =
    std::sync::OnceLock::new();

/// A process-wide "please stop" that is not an OS signal. The Windows
/// service-control handler (which runs on an SCM thread, outside any async
/// runtime) cancels it on `Stop`/`Shutdown`; [`terminate_signal`] observes it.
pub fn external_stop() -> tokio_util::sync::CancellationToken {
    EXTERNAL_STOP.get_or_init(Default::default).clone()
}

/// §35: resolves on SIGINT or SIGTERM (Ctrl-C on non-Unix), or when
/// [`external_stop`] is cancelled (a Windows service `Stop`). Install once per
/// process; the first signal requests a graceful shutdown.
pub async fn terminate_signal() {
    let stop = external_stop();
    tokio::select! {
        () = os_signal() => {}
        () = stop.cancelled() => {}
    }
}

async fn os_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut term), Ok(mut int)) => {
                tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            }
            _ => std::future::pending::<()>().await,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// §87: call `f` every time the process receives SIGHUP (the conventional
/// "re-read your config" signal). Returns when the runtime shuts down.
#[cfg(unix)]
pub async fn on_hangup(lifecycle: crate::lifecycle::Lifecycle, mut f: impl FnMut()) {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut hup) = signal(SignalKind::hangup()) else {
        return;
    };
    loop {
        tokio::select! {
            _ = hup.recv() => f(),
            () = lifecycle.shutdown_requested() => return,
        }
    }
}

/// §97: the watchdog ping interval systemd expects. We ping at **half** the
/// deadline. `None` when no watchdog is configured **for this process**
/// (`sd_notify` also honours `WATCHDOG_PID`, which the hand-rolled version
/// ignored: a watchdog meant for a parent must not be pinged by a child).
#[must_use]
pub fn watchdog_interval() -> Option<std::time::Duration> {
    #[cfg(unix)]
    {
        let d = sd_notify::watchdog_enabled()?;
        (d.as_micros() >= 2).then(|| d / 2)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Ping the watchdog while the runtime is *actually* healthy: stop pinging if
/// a critical subsystem is `Unavailable`, so systemd restarts a wedged daemon
/// instead of being reassured by a loop that proves nothing.
pub async fn watchdog_loop(
    lifecycle: crate::lifecycle::Lifecycle,
    health: crate::health::HealthRegistry,
) {
    let Some(every) = watchdog_interval() else {
        return;
    };
    let mut tick = tokio::time::interval(every);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if watchdog_should_ping(&lifecycle, &health) {
                    #[cfg(unix)]
                    notify(&[NotifyState::Watchdog]);
                }
            }
            () = lifecycle.shutdown_requested() => return,
        }
    }
}

/// The liveness rule, separated so it can be tested.
#[must_use]
pub fn watchdog_should_ping(
    lifecycle: &crate::lifecycle::Lifecycle,
    health: &crate::health::HealthRegistry,
) -> bool {
    use crate::health::HealthState;
    use crate::lifecycle::RuntimeState;
    let critical_down = health
        .report()
        .subsystems
        .iter()
        .any(|s| s.critical && s.state == HealthState::Unavailable);
    !critical_down && !matches!(lifecycle.state(), RuntimeState::Stopped)
}

/// `systemd --user` unit for a long-running node/daemon.
///
/// Hardening follows §117-§118 (least privilege): no new privileges, private
/// tmp, read-only system, writes only to the state directory.
#[must_use]
pub fn systemd_user_unit(description: &str, exec: &Path, args: &[&str]) -> String {
    let args = args.join(" ");
    format!(
        "[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n\
[Service]\nType=notify\nNotifyAccess=main\nExecStart={exec} {args}\nRestart=on-failure\nRestartSec=5\n\
TimeoutStopSec=30\nWatchdogSec=60\nNoNewPrivileges=yes\nPrivateTmp=yes\nProtectSystem=strict\n\
ProtectHome=read-only\nStateDirectory=siar\nRuntimeDirectory=siar\nRuntimeDirectoryMode=0700\n\
ProtectKernelTunables=yes\nRestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK AF_BLUETOOTH\n\n\
[Install]\nWantedBy=default.target\n",
        exec = exec.display()
    )
}

/// XDG autostart entry (§174/§175) for the desktop app or daemon.
#[must_use]
pub fn desktop_autostart_entry(name: &str, exec: &Path, args: &[&str]) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={exec} {args}\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exec = exec.display(),
        args = args.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_is_a_user_service_with_hardening_and_no_root() {
        let u = systemd_user_unit(
            "Siar relay",
            Path::new("/usr/bin/siar-emergency-node"),
            &["--headless"],
        );
        assert!(
            u.contains("Type=notify")
                && u.contains("NoNewPrivileges=yes")
                && u.contains("ProtectSystem=strict")
        );
        assert!(u.contains("ExecStart=/usr/bin/siar-emergency-node --headless"));
        assert!(
            u.contains("WantedBy=default.target"),
            "user target, not multi-user"
        );
        assert!(!u.contains("User=root"));
    }

    #[test]
    fn autostart_entry_shape() {
        let e = desktop_autostart_entry(
            "Siar",
            Path::new("/usr/bin/siar-desktop"),
            &["--background"],
        );
        assert!(
            e.starts_with("[Desktop Entry]")
                && e.contains("Exec=/usr/bin/siar-desktop --background")
        );
    }

    #[cfg(unix)]
    #[test]
    fn notify_delivers_to_a_path_socket_and_is_a_noop_without_one() {
        // Single test mutating the env var, to avoid races between tests.
        let dir = std::env::temp_dir().join(format!("siar-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("n.sock");
        let _ = std::fs::remove_file(&p);
        let rx = std::os::unix::net::UnixDatagram::bind(&p).unwrap();
        std::env::remove_var("NOTIFY_SOCKET");
        assert!(!notify_ready());
        std::env::set_var("NOTIFY_SOCKET", &p);
        assert!(notify_ready());
        let mut buf = [0u8; 32];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\n");
        std::env::remove_var("NOTIFY_SOCKET");
    }

    #[test]
    fn watchdog_interval_is_half_the_deadline_and_absent_by_default() {
        // One test mutates this env var, to avoid races between tests.
        std::env::remove_var("WATCHDOG_USEC");
        assert_eq!(watchdog_interval(), None);
        std::env::set_var("WATCHDOG_USEC", "60000000");
        assert_eq!(
            watchdog_interval(),
            Some(std::time::Duration::from_secs(30))
        );
        std::env::set_var("WATCHDOG_USEC", "garbage");
        assert_eq!(watchdog_interval(), None);
        std::env::set_var("WATCHDOG_USEC", "60000000");
        std::env::set_var("WATCHDOG_PID", "1"); // meant for another process
        assert_eq!(watchdog_interval(), None);
        std::env::remove_var("WATCHDOG_PID");
        std::env::remove_var("WATCHDOG_USEC");
    }

    #[test]
    fn watchdog_withholds_the_ping_when_a_critical_subsystem_is_down() {
        use crate::health::{HealthRegistry, HealthState};
        let (l, h) = (crate::lifecycle::Lifecycle::new(), HealthRegistry::new());
        assert!(watchdog_should_ping(&l, &h));
        h.set("storage", HealthState::Unavailable, "disk gone", true);
        assert!(
            !watchdog_should_ping(&l, &h),
            "a wedged daemon must be allowed to be restarted"
        );
        h.set("storage", HealthState::Healthy, "", true);
        h.set("thumbs", HealthState::Unavailable, "crashed", false);
        assert!(
            watchdog_should_ping(&l, &h),
            "a non-critical failure does not trip the watchdog"
        );
    }

    #[test]
    fn install_ctx_carries_our_hardened_unit_only_on_systemd() {
        let exe = Path::new("/usr/bin/siar-daemon");
        let sd = user_service_ctx("Siar daemon", exe, &["run"], true).unwrap();
        let unit = sd.contents.as_deref().unwrap();
        assert!(
            unit.contains("NoNewPrivileges=yes") && unit.contains("run"),
            "{unit}"
        );
        assert!(sd.autostart);
        let other = user_service_ctx("Siar daemon", exe, &["run"], false).unwrap();
        assert!(other.contents.is_none());
        assert_eq!(other.args, vec![std::ffi::OsString::from("run")]);
    }

    #[tokio::test]
    async fn an_external_stop_ends_terminate_signal() {
        // Process-global by design, so this test is the only one that cancels it.
        let waiter = tokio::spawn(terminate_signal());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        external_stop().cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("a service Stop must end the wait")
            .unwrap();
    }
}
