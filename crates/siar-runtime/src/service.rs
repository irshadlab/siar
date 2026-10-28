//! §37-§39, §99, §174-§175: service-manager integration.
//!
//! * [`sd_notify`] — readiness/stopping notification for `Type=notify`
//!   systemd units (§99). A no-op when `NOTIFY_SOCKET` is unset, so the same
//!   binary runs fine under a shell.
//! * [`systemd_user_unit`] / [`desktop_autostart_entry`] — generated unit
//!   text (§38 "Linux user service", §174 autostart). **User** services:
//!   no root needed (§39).

use std::path::Path;

/// Send one `sd_notify` datagram (`READY=1`, `STOPPING=1`, `STATUS=…`).
/// Returns `true` if a message was actually sent.
#[cfg(unix)]
pub fn sd_notify(msg: &str) -> bool {
    let Some(sock) = std::env::var_os("NOTIFY_SOCKET") else {
        return false;
    };
    let Ok(s) = std::os::unix::net::UnixDatagram::unbound() else {
        return false;
    };
    let bytes = std::os::unix::ffi::OsStrExt::as_bytes(sock.as_os_str());
    #[cfg(target_os = "linux")]
    if let Some(name) = bytes.strip_prefix(b"@") {
        use std::os::linux::net::SocketAddrExt;
        let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(name) else {
            return false;
        };
        return s.send_to_addr(msg.as_bytes(), &addr).is_ok();
    }
    s.send_to(msg.as_bytes(), Path::new(&sock)).is_ok()
}

/// §35: resolves on SIGINT or SIGTERM (Ctrl-C on non-Unix). Install once per
/// process; the first signal requests a graceful shutdown.
pub async fn terminate_signal() {
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
    fn sd_notify_delivers_to_a_path_socket_and_is_a_noop_without_one() {
        // Single test mutating the env var, to avoid races between tests.
        let dir = std::env::temp_dir().join(format!("siar-notify-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("n.sock");
        let _ = std::fs::remove_file(&p);
        let rx = std::os::unix::net::UnixDatagram::bind(&p).unwrap();
        std::env::remove_var("NOTIFY_SOCKET");
        assert!(!sd_notify("READY=1"));
        std::env::set_var("NOTIFY_SOCKET", &p);
        assert!(sd_notify("READY=1"));
        let mut buf = [0u8; 32];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        std::env::remove_var("NOTIFY_SOCKET");
    }
}
