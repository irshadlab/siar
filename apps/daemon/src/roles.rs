//! Role launcher: what a provisioned node *runs* beyond the chat core.
//!
//! The daemon binary hosts the chat core. Relay, DTN and organisation roles
//! need the relay engine, which is its own binary (`siar-emergency-node`).
//! Rather than re-embed it, the daemon runs it as a **supervised child**:
//!
//! * located **only beside the daemon's own executable** — never by `PATH`
//!   search, which would let a planted binary run with the daemon's rights;
//! * restarted by the runtime's supervisor under its normal policy (bounded
//!   restarts, backoff, circuit breaker → the node reports `Degraded`, it
//!   does not crash-loop);
//! * stopped gracefully (SIGTERM, then a kill after a grace period) when the
//!   daemon shuts down, and never left running if the daemon is aborted
//!   (`kill_on_drop`).
//!
//! Roles with no binary yet (storage) are reported as such, not faked.

use std::path::{Path, PathBuf};
use std::time::Duration;

use siar_runtime::lifecycle::Lifecycle;
use siar_runtime::node::{Feature, NodeRole};
use siar_runtime::runtime::Runtime;
use tokio::process::{Child, Command};

pub const RELAY_BINARY: &str = "siar-emergency-node";
const GRACE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub binary: &'static str,
    pub args: Vec<String>,
}

/// What a role needs started *in addition to* (or instead of) the chat core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RolePlan {
    /// Nothing beyond the chat core (a personal device).
    CoreOnly,
    /// A supervised child process.
    Child(Launch),
    /// The role needs a component this workspace does not have yet.
    Unavailable(&'static str),
}

/// Decide from the role's feature set (the single source of truth in
/// `siar_runtime::node`), not from the role's name.
#[must_use]
pub fn plan_for(role: NodeRole, data_dir: &Path) -> RolePlan {
    let f = role.features();
    let relays = f.contains(Feature::RelayForwarding)
        || (f.contains(Feature::Dtn) && !f.contains(Feature::Messaging));
    if relays {
        // Identity lives in the daemon's data folder so it is stable across
        // restarts (peers keep trusting a relay) and is covered by backups.
        let identity = data_dir.join("relay").join("identity.bin");
        return RolePlan::Child(Launch {
            binary: RELAY_BINARY,
            args: vec!["--identity".into(), identity.display().to_string()],
        });
    }
    if f.contains(Feature::Storage) && !f.contains(Feature::Messaging) {
        return RolePlan::Unavailable("storage-node: no storage binary exists yet");
    }
    RolePlan::CoreOnly
}

/// The binary must sit next to the running daemon.
pub fn locate(binary: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find own path: {e}"))?;
    locate_beside(&exe, binary)
}

fn locate_beside(exe: &Path, binary: &str) -> Result<PathBuf, String> {
    let dir = exe.parent().ok_or("daemon path has no directory")?;
    let name = if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    };
    let p = dir.join(name);
    if p.is_file() {
        Ok(p)
    } else {
        Err(format!(
            "{binary} not found next to the daemon ({}); install them together",
            dir.display()
        ))
    }
}

#[cfg(unix)]
fn ask_to_stop(child: &Child) {
    // SIGTERM without `unsafe`: use the system `kill`.
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
}

#[cfg(not(unix))]
fn ask_to_stop(_child: &Child) {}

/// Run `exe` until it exits or the daemon shuts down. An exit the daemon did
/// not ask for is an **error** (even exit 0): a relay is not meant to stop,
/// so the supervisor should restart it.
pub async fn run_child(
    exe: PathBuf,
    args: Vec<String>,
    lifecycle: Lifecycle,
    grace: Duration,
) -> Result<(), String> {
    let mut child = Command::new(&exe)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", exe.display()))?;
    tokio::select! {
        status = child.wait() => {
            let status = status.map_err(|e| e.to_string())?;
            Err(format!("{} exited unexpectedly ({status})", exe.display()))
        }
        () = lifecycle.shutdown_requested() => {
            ask_to_stop(&child);
            if tokio::time::timeout(grace, child.wait()).await.is_err() {
                tracing::warn!("role child did not stop within the grace period; killing it");
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            Ok(())
        }
    }
}

/// Start supervising the role's child under the runtime's restart policy.
pub fn supervise(runtime: &Runtime, launch: Launch) {
    let lifecycle = runtime.lifecycle.clone();
    runtime.supervisor.spawn(
        "role-relay",
        false,
        siar_runtime::RestartPolicy::default(),
        move || {
            let (launch, lifecycle) = (launch.clone(), lifecycle.clone());
            async move {
                let exe = locate(launch.binary)?;
                run_child(exe, launch.args, lifecycle, GRACE).await
            }
        },
    );
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-roles-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn lifecycle() -> Lifecycle {
        Lifecycle::new()
    }

    #[test]
    fn the_plan_follows_the_feature_set_not_the_role_name() {
        let d = Path::new("/data");
        assert_eq!(plan_for(NodeRole::PersonalDevice, d), RolePlan::CoreOnly);
        for r in [
            NodeRole::RelayGateway,
            NodeRole::DtnNode,
            NodeRole::OrganizationNode,
            NodeRole::TestNode,
        ] {
            match plan_for(r, d) {
                RolePlan::Child(l) => {
                    assert_eq!(l.binary, RELAY_BINARY, "{r}");
                    assert!(l.args.iter().any(|a| a.ends_with("relay/identity.bin")));
                }
                other => panic!("{r}: {other:?}"),
            }
        }
        assert!(matches!(
            plan_for(NodeRole::StorageNode, d),
            RolePlan::Unavailable(_)
        ));
    }

    #[test]
    fn the_binary_is_found_only_beside_the_daemon() {
        let d = tmp("locate");
        let exe = d.join("siar-daemon");
        assert!(locate_beside(&exe, RELAY_BINARY)
            .unwrap_err()
            .contains("not found next to the daemon"));
        script(&d, RELAY_BINARY, "exit 0");
        assert_eq!(
            locate_beside(&exe, RELAY_BINARY).unwrap(),
            d.join(RELAY_BINARY)
        );
        // A same-named binary elsewhere (e.g. on PATH) is never used.
        let other = tmp("elsewhere");
        script(&other, "siar-other", "exit 0");
        assert!(locate_beside(&exe, "siar-other").is_err());
    }

    #[tokio::test]
    async fn shutdown_stops_the_child_gracefully_with_sigterm() {
        let d = tmp("graceful");
        let marker = d.join("got-term");
        // Traps TERM, records it, exits 0; otherwise sleeps.
        let exe = script(
            &d,
            "child",
            &format!(
                "trap 'echo yes > {}; exit 0' TERM\nwhile true; do sleep 0.1; done",
                marker.display()
            ),
        );
        let lc = lifecycle();
        let task = tokio::spawn(run_child(exe, vec![], lc.clone(), Duration::from_secs(5)));
        tokio::time::sleep(Duration::from_millis(400)).await;
        lc.begin_shutdown();
        assert_eq!(task.await.unwrap(), Ok(()));
        assert!(
            marker.exists(),
            "the child must have been asked to stop with SIGTERM"
        );
    }

    #[tokio::test]
    async fn a_child_that_ignores_sigterm_is_killed_after_the_grace_period() {
        let d = tmp("stubborn");
        let exe = script(&d, "child", "trap '' TERM\nwhile true; do sleep 0.1; done");
        let lc = lifecycle();
        let task = tokio::spawn(run_child(
            exe,
            vec![],
            lc.clone(),
            Duration::from_millis(500),
        ));
        tokio::time::sleep(Duration::from_millis(400)).await;
        let t0 = std::time::Instant::now();
        lc.begin_shutdown();
        assert_eq!(task.await.unwrap(), Ok(()));
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "must not hang on a stubborn child"
        );
    }

    #[tokio::test]
    async fn an_unrequested_exit_is_an_error_even_with_status_zero() {
        let d = tmp("exit");
        for (body, what) in [("exit 0", "0"), ("exit 3", "3")] {
            let exe = script(&d, &format!("c{what}"), body);
            let r = run_child(exe, vec![], lifecycle(), Duration::from_secs(1)).await;
            let e = r.unwrap_err();
            assert!(e.contains("exited unexpectedly"), "{e}");
        }
        let missing = run_child(d.join("nope"), vec![], lifecycle(), Duration::from_secs(1)).await;
        assert!(missing.unwrap_err().contains("starting"));
    }

    #[tokio::test]
    async fn args_reach_the_child() {
        let d = tmp("args");
        let out = d.join("args.txt");
        let exe = script(
            &d,
            "child",
            &format!("echo \"$@\" > {}\nsleep 5", out.display()),
        );
        let lc = lifecycle();
        let task = tokio::spawn(run_child(
            exe,
            vec!["--identity".into(), "/x/id.bin".into()],
            lc.clone(),
            Duration::from_secs(2),
        ));
        tokio::time::sleep(Duration::from_millis(500)).await;
        lc.begin_shutdown();
        let _ = task.await;
        assert_eq!(
            std::fs::read_to_string(out).unwrap().trim(),
            "--identity /x/id.bin"
        );
    }
}
