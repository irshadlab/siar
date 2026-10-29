//! §176-§178: on-demand daemon with a safe spawn race.
//!
//! A UI or CLI that finds no daemon starts one, then connects. Two such
//! clients may do this at the same moment; that is harmless **by
//! construction**: the daemon binds a socket and takes a data-folder lock, so
//! exactly one spawn wins and the loser exits. Both clients then connect to
//! the winner. This helper just retries the connection with backoff.

use std::future::Future;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::ipc::{ClientError, IpcClient};
use crate::protocol::Role;

#[derive(Debug, thiserror::Error)]
pub enum EnsureError {
    #[error("could not start the daemon: {0}")]
    Spawn(io::Error),
    #[error("the daemon did not become reachable in time (last error: {0})")]
    Timeout(String),
}

/// Connect to the daemon at `socket`, starting it with `spawn` if needed.
/// `spawn` is called **at most once** per call, and only after a failed first
/// connection attempt.
pub async fn ensure_daemon<F, Fut>(
    socket: &Path,
    role: Role,
    client_name: &str,
    spawn: F,
    timeout: Duration,
) -> Result<IpcClient, EnsureError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    if let Ok(c) = IpcClient::connect_unix(socket, role, client_name).await {
        return Ok(c);
    }
    spawn().await.map_err(EnsureError::Spawn)?;
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(20);
    loop {
        match IpcClient::connect_unix(socket, role, client_name).await {
            Ok(c) => return Ok(c),
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(EnsureError::Timeout(describe(&e)));
                }
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_millis(250));
    }
}

fn describe(e: &ClientError) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{serve_unix, ServerConfig};
    use crate::runtime::Runtime;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn sock(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("siar-ensure-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("d.sock")
    }

    async fn spawn_daemon(path: std::path::PathBuf, started: Arc<AtomicU32>) -> io::Result<()> {
        tokio::spawn(async move {
            let rt = Runtime::builder().build();
            rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
                .await
                .unwrap();
            started.fetch_add(1, Ordering::SeqCst);
            // The loser of a bind race gets AddrInUse and simply exits.
            let _ = serve_unix(rt, &path, ServerConfig::default()).await;
        });
        Ok(())
    }

    #[tokio::test]
    async fn starts_the_daemon_when_absent_and_does_not_when_present() {
        let path = sock("one");
        let spawns = Arc::new(AtomicU32::new(0));
        let (p, s) = (path.clone(), spawns.clone());
        let c = ensure_daemon(
            &path,
            Role::Observer,
            "t",
            || {
                s.fetch_add(1, Ordering::SeqCst);
                spawn_daemon(p, Arc::new(AtomicU32::new(0)))
            },
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        drop(c);
        let s2 = spawns.clone();
        ensure_daemon(
            &path,
            Role::Observer,
            "t",
            || {
                s2.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(
            spawns.load(Ordering::SeqCst),
            1,
            "already running: never spawned again"
        );
    }

    #[tokio::test]
    async fn two_clients_racing_to_spawn_end_up_on_the_same_daemon() {
        let path = sock("race");
        let started = Arc::new(AtomicU32::new(0));
        let mk = |p: std::path::PathBuf, st: Arc<AtomicU32>| async move {
            let p2 = p.clone();
            ensure_daemon(
                &p,
                Role::Observer,
                "racer",
                move || spawn_daemon(p2, st),
                Duration::from_secs(5),
            )
            .await
        };
        let (a, b) = tokio::join!(
            mk(path.clone(), started.clone()),
            mk(path.clone(), started.clone())
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        assert_eq!(
            a.instance_id, b.instance_id,
            "one winner; both clients attached to it"
        );
    }

    #[tokio::test]
    async fn a_spawn_that_never_yields_a_daemon_times_out_cleanly() {
        let path = sock("never");
        let e = ensure_daemon(
            &path,
            Role::Observer,
            "t",
            || async { Ok(()) },
            Duration::from_millis(300),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(e, EnsureError::Timeout(_)));
    }

    #[tokio::test]
    async fn spawn_failure_is_reported() {
        let path = sock("fail");
        let e = ensure_daemon(
            &path,
            Role::Observer,
            "t",
            || async { Err(io::Error::new(io::ErrorKind::NotFound, "no binary")) },
            Duration::from_secs(1),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(e, EnsureError::Spawn(_)));
    }
}
