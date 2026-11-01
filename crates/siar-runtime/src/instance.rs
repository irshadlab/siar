//! §100-§102, §106-§110, §167-§168: instance metadata and upgrade handoff.
//!
//! * **Instance file** — `instance.toml` in the runtime directory records the
//!   pid, instance id, build and protocol range of the running daemon, so a
//!   newer binary (or `ctl`) can decide what to do without guessing.
//! * **Upgrade lock** — a short-lived exclusive file so two upgraders cannot
//!   both decide to replace the same daemon (§107). It expires, so a crashed
//!   upgrader never wedges the next one.
//! * **Handoff** — ask the old daemon to shut down gracefully over its own
//!   authenticated socket, then wait until it has actually let go of the
//!   socket (§100). The data-folder lock is released by the kernel when the
//!   old process exits, so a stale lock cannot survive it (§109).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

const META_FILE: &str = "instance.toml";
const UPGRADE_FILE: &str = "upgrade.lock";
/// An upgrade lock older than this is presumed abandoned.
pub const UPGRADE_LOCK_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceMeta {
    /// Hex of the 128-bit instance id (TOML integers are 64-bit).
    pub instance_id: String,
    pub pid: u32,
    pub build: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub started_unix_ms: u64,
}

impl InstanceMeta {
    #[must_use]
    pub fn current(instance_id: u128, build: &str) -> Self {
        Self {
            instance_id: format!("{instance_id:032x}"),
            pid: std::process::id(),
            build: build.to_owned(),
            protocol_min: crate::protocol::MIN_PROTOCOL_VERSION,
            protocol_max: crate::protocol::PROTOCOL_VERSION,
            started_unix_ms: unix_ms(),
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Atomically write (`tmp` + rename), owner-only.
pub fn write_meta(dir: &Path, meta: &InstanceMeta) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let text = toml::to_string(meta).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!("{META_FILE}.tmp{}", std::process::id()));
    fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(tmp, dir.join(META_FILE))
}

#[must_use]
pub fn read_meta(dir: &Path) -> Option<InstanceMeta> {
    toml::from_str(&fs::read_to_string(dir.join(META_FILE)).ok()?).ok()
}

pub fn remove_meta(dir: &Path) {
    let _ = fs::remove_file(dir.join(META_FILE));
}

/// Is a process with this pid alive **and able to hold locks**? Linux reads
/// `/proc/<pid>/stat`: a zombie (`Z`, exited but not yet reaped) or dead (`X`)
/// process holds nothing, so it counts as gone. Elsewhere we cannot tell
/// cheaply and answer `true` (callers then fall back to probing the socket or
/// retrying the lock, which are authoritative anyway).
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Format: `pid (comm) S ...` — comm may contain spaces/parens, so split at the last ')'.
    let state = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.trim_start().chars().next());
    !matches!(state, Some('Z' | 'X'))
}

/// §109: metadata that names a dead process is stale.
#[must_use]
pub fn meta_is_stale(m: &InstanceMeta) -> bool {
    !pid_alive(m.pid)
}

#[derive(Debug)]
pub struct UpgradeLock {
    path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpgradeLockError {
    /// Someone else is upgrading right now.
    Held {
        age: Duration,
    },
    Io(io::ErrorKind),
}

impl UpgradeLock {
    pub fn acquire(dir: &Path) -> Result<Self, UpgradeLockError> {
        Self::acquire_with_ttl(dir, UPGRADE_LOCK_TTL)
    }

    pub fn acquire_with_ttl(dir: &Path, ttl: Duration) -> Result<Self, UpgradeLockError> {
        fs::create_dir_all(dir).map_err(|e| UpgradeLockError::Io(e.kind()))?;
        let path = dir.join(UPGRADE_FILE);
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let age = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .unwrap_or_default();
                    if age >= ttl {
                        let _ = fs::remove_file(&path); // abandoned: retry once
                    } else {
                        return Err(UpgradeLockError::Held { age });
                    }
                }
                Err(e) => return Err(UpgradeLockError::Io(e.kind())),
            }
        }
        Err(UpgradeLockError::Io(io::ErrorKind::AlreadyExists))
    }
}

impl Drop for UpgradeLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
pub use handoff::{request_handoff, Handoff, HandoffError};

#[cfg(unix)]
mod handoff {
    use super::{Duration, Instant, Path};
    use crate::dedup::CommandId;
    use crate::error::RuntimeError;
    use crate::ipc::{ClientError, IpcClient};
    use crate::protocol::{Command, Query, Reply, Role};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Handoff {
        pub old_instance_id: u128,
        pub old_build: String,
        pub old_protocol: u16,
    }

    #[derive(Debug, thiserror::Error)]
    pub enum HandoffError {
        /// Nothing is listening: there is nobody to hand off from.
        #[error("no daemon is running at that socket")]
        NoDaemon,
        #[error("the running daemon refused to stop: {0}")]
        Refused(RuntimeError),
        #[error("the old daemon did not release its socket in time")]
        Timeout,
        #[error(transparent)]
        Client(#[from] ClientError),
    }

    /// Ask the daemon at `socket` to shut down gracefully and wait until it
    /// has released the socket. After `Ok`, the caller may bind and take the
    /// data-folder lock. Requires the Admin role, so only the same user (or
    /// an allow-listed one) can do this (§173).
    pub async fn request_handoff(
        socket: &Path,
        timeout: Duration,
    ) -> Result<Handoff, HandoffError> {
        let deadline = Instant::now() + timeout;
        let mut c = match IpcClient::connect_local(socket, Role::Admin, "upgrade").await {
            Ok(c) => c,
            Err(ClientError::Io(_)) => return Err(HandoffError::NoDaemon),
            Err(e) => return Err(e.into()),
        };
        let Reply::Status(s) = c.query(Query::Status).await? else {
            return Err(ClientError::Protocol("expected Status").into());
        };
        // The data-folder lock and the database lock belong to the *process*,
        // so releasing the socket is not enough: remember who to wait for.
        let old_pid = socket
            .parent()
            .and_then(super::read_meta)
            .map(|m| m.pid)
            .filter(|p| *p != std::process::id());
        let info = Handoff {
            old_instance_id: s.instance_id,
            old_build: s.build,
            old_protocol: s.protocol_version,
        };
        match c
            .command(CommandId(uuid::Uuid::new_v4().as_u128()), Command::Shutdown)
            .await
        {
            Ok(_) => {}
            Err(ClientError::Runtime(e)) => return Err(HandoffError::Refused(e)),
            Err(e) => return Err(e.into()),
        }
        drop(c);
        // Wait for the old daemon to let go of the socket…
        loop {
            if tokio::net::UnixStream::connect(socket).await.is_err() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(HandoffError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // …and then for its process to exit, which is what frees the data
        // folder and database locks (Linux can check; elsewhere callers retry
        // the lock instead).
        if let Some(pid) = old_pid.filter(|_| cfg!(target_os = "linux")) {
            while super::pid_alive(pid) {
                if Instant::now() >= deadline {
                    return Err(HandoffError::Timeout);
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("siar-inst-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn meta_round_trips_and_is_private() {
        let d = tmp("meta");
        let m = InstanceMeta::current(0xdead_beef, "1.2.3");
        write_meta(&d, &m).unwrap();
        assert_eq!(read_meta(&d), Some(m.clone()));
        assert_eq!(m.instance_id.len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(d.join(META_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        remove_meta(&d);
        assert_eq!(read_meta(&d), None);
    }

    #[test]
    fn garbage_meta_is_none_not_a_panic() {
        let d = tmp("junk");
        fs::write(d.join(META_FILE), "pid = \"nope\"\n\u{0}").unwrap();
        assert_eq!(read_meta(&d), None);
    }

    #[test]
    fn my_own_pid_is_alive_and_an_absurd_one_is_stale() {
        let mut m = InstanceMeta::current(1, "x");
        assert!(!meta_is_stale(&m));
        if cfg!(target_os = "linux") {
            m.pid = u32::MAX - 1;
            assert!(meta_is_stale(&m));
        }
    }

    #[test]
    fn only_one_upgrader_at_a_time_and_stale_locks_expire() {
        let d = tmp("lock");
        let a = UpgradeLock::acquire(&d).unwrap();
        assert!(matches!(
            UpgradeLock::acquire(&d),
            Err(UpgradeLockError::Held { .. })
        ));
        drop(a);
        let b = UpgradeLock::acquire(&d).unwrap();
        // A crashed upgrader leaves the file behind; a zero TTL treats it as abandoned.
        std::mem::forget(b);
        assert!(UpgradeLock::acquire_with_ttl(&d, Duration::ZERO).is_ok());
    }

    #[cfg(unix)]
    mod handoff_tests {
        use super::super::*;
        use super::tmp;
        use crate::ipc::{serve_local, ServerConfig};
        use crate::runtime::Runtime;

        async fn running(path: &Path) -> (Runtime, tokio::task::JoinHandle<io::Result<()>>) {
            let rt = Runtime::builder().build_id("old-1.0").build();
            rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
                .await
                .unwrap();
            let (r2, p2) = (rt.clone(), path.to_path_buf());
            let h =
                tokio::spawn(async move { serve_local(r2, &p2, ServerConfig::default()).await });
            for _ in 0..200 {
                if path.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            (rt, h)
        }

        #[tokio::test]
        async fn new_daemon_replaces_the_old_one_gracefully() {
            let d = tmp("handoff");
            let sock = d.join("d.sock");
            let (old, old_task) = running(&sock).await;
            let info = request_handoff(&sock, Duration::from_secs(5))
                .await
                .unwrap();
            assert_eq!(info.old_build, "old-1.0");
            assert_eq!(info.old_instance_id, old.bus.instance_id());
            tokio::time::timeout(Duration::from_secs(3), old_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(!sock.exists());
            // The new daemon can now take the socket.
            let (new, _t) = running(&sock).await;
            assert_ne!(new.bus.instance_id(), old.bus.instance_id());
            new.shutdown(Duration::from_secs(1)).await;
        }

        #[cfg(target_os = "linux")]
        #[tokio::test]
        async fn handoff_waits_for_the_old_process_to_exit_not_just_the_socket() {
            let d = tmp("pidwait");
            let sock = d.join("d.sock");
            let (_old, old_task) = running(&sock).await;
            // Stand-in for the old daemon's process: it outlives its socket by ~700 ms.
            let mut child = std::process::Command::new("sleep")
                .arg("0.7")
                .spawn()
                .unwrap();
            let mut meta = InstanceMeta::current(1, "old");
            meta.pid = child.id();
            write_meta(&d, &meta).unwrap();
            let t0 = Instant::now();
            request_handoff(&sock, Duration::from_secs(5))
                .await
                .unwrap();
            assert!(
                t0.elapsed() >= Duration::from_millis(600),
                "returned after {:?}, before the process was gone",
                t0.elapsed()
            );
            assert!(
                !pid_alive(child.id()),
                "a zombie holds no locks and must read as gone"
            );
            let _ = child.wait();
            let _ = old_task.await;
        }

        #[tokio::test]
        async fn handoff_with_nobody_running_says_so() {
            let d = tmp("nobody");
            assert!(matches!(
                request_handoff(&d.join("none.sock"), Duration::from_millis(200)).await,
                Err(HandoffError::NoDaemon)
            ));
        }
    }
}
