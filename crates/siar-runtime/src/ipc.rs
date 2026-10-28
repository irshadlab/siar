//! §13-§20, §144-§146, §179: local IPC over a Unix domain socket.
//!
//! Security model (§17, §179): the socket lives in a `0700` directory and is
//! itself `0600`; on top of that every connection's peer uid (`SO_PEERCRED`)
//! must equal the socket owner's uid (or be explicitly allow-listed). There
//! is no password to leak and no TCP port to scan. Roles (§19) are granted as
//! `min(requested, configured max)` and every command is checked against its
//! required role (§20).
//!
//! Backpressure (§144-§146): bounded connections, a per-connection request
//! rate limit, a bounded request queue, a write timeout (a client that does
//! not read is disconnected, never buffered without limit), and bounded
//! event subscriptions with gap reporting (see [`crate::events`]).
//!
//! Windows named-pipe transport (§206) is **not implemented** in this crate
//! yet; the protocol layer is transport-agnostic, so it is a new `serve_*`
//! function, not a protocol change.

use std::collections::VecDeque;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, Semaphore};

use crate::dedup::CommandId;
use crate::error::RuntimeError;
use crate::events::{Delivery, Subscription, Topic};
use crate::protocol::{
    negotiate_version, read_frame, write_frame, ClientMsg, FrameError, Reply, Request, Role,
    ServerMsg, MAX_FRAME, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use crate::runtime::Runtime;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_connections: usize,
    pub requests_per_sec: u32,
    pub hello_timeout: Duration,
    pub write_timeout: Duration,
    /// Extra uids allowed besides the socket owner.
    pub allowed_uids: Vec<u32>,
    /// Highest role this server will ever grant.
    pub max_role: Role,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 32,
            requests_per_sec: 200,
            hello_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(10),
            allowed_uids: Vec::new(),
            max_role: Role::Admin,
        }
    }
}

/// Bind `path` safely and serve until the runtime begins shutting down.
pub async fn serve_unix(rt: Runtime, path: &Path, cfg: ServerConfig) -> io::Result<()> {
    let listener = bind_secure(path).await?;
    // Removes the socket file however this future ends — including being
    // aborted by the supervisor at shutdown, which never reaches the tail of
    // the function.
    let _cleanup = SocketFile(path.to_path_buf());
    let owner = std::fs::metadata(path)?.uid();
    let slots = Arc::new(Semaphore::new(cfg.max_connections));
    let cfg = Arc::new(cfg);
    tracing::info!(socket = %path.display(), "ipc listening");

    loop {
        tokio::select! {
            () = rt.lifecycle.shutdown_requested() => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let uid = match stream.peer_cred() {
                    Ok(c) => c.uid(),
                    Err(_) => continue,
                };
                if uid != owner && !cfg.allowed_uids.contains(&uid) {
                    tracing::warn!(uid, "rejected ipc peer: uid not allowed");
                    continue;
                }
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    tracing::warn!("ipc connection limit reached; dropping");
                    continue;
                };
                let (rt, cfg) = (rt.clone(), cfg.clone());
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_connection(rt, stream, uid, &cfg).await {
                        tracing::debug!(error = %e, "ipc connection ended");
                    }
                });
            }
        }
    }
    Ok(())
}

struct SocketFile(PathBuf);

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// §109 stale-socket recovery + §179 permissions.
async fn bind_secure(path: &Path) -> io::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    if path.exists() {
        match UnixStream::connect(path).await {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another instance is serving this socket",
                ))
            }
            Err(_) => std::fs::remove_file(path)?, // nobody home: stale file from a crash
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

struct RateLimit {
    per_sec: u32,
    window_start: Instant,
    count: u32,
}

impl RateLimit {
    fn allow(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count <= self.per_sec
    }
}

async fn handle_connection(
    rt: Runtime,
    stream: UnixStream,
    uid: u32,
    cfg: &ServerConfig,
) -> Result<(), FrameError> {
    let (mut rd, mut wr) = stream.into_split();

    // §16 handshake.
    let hello = tokio::time::timeout(cfg.hello_timeout, read_frame::<_, ClientMsg>(&mut rd))
        .await
        .map_err(|_| FrameError::Closed)??;
    let ClientMsg::Hello {
        min_version,
        max_version,
        role,
        ..
    } = hello
    else {
        return Err(FrameError::Malformed);
    };
    let Some(version) = negotiate_version(min_version, max_version) else {
        let _ = write_frame(
            &mut wr,
            &ServerMsg::Response {
                req_id: 0,
                result: Err(RuntimeError::Unsupported),
            },
        )
        .await;
        return Err(FrameError::Malformed);
    };
    let granted = role.min(cfg.max_role);
    write_frame(
        &mut wr,
        &ServerMsg::Welcome {
            version,
            instance_id: rt.bus.instance_id(),
            role: granted,
            next_seq: rt.bus.next_seq(),
        },
    )
    .await?;

    // Reader task: frame reads are not cancel-safe, so they never sit inside `select!`.
    let (tx, mut rx) = mpsc::channel::<ClientMsg>(8);
    let reader = tokio::spawn(async move {
        while let Ok(m) = read_frame::<_, ClientMsg>(&mut rd).await {
            if tx.send(m).await.is_err() {
                break;
            }
        }
    });

    let mut sub: Option<Subscription> = None;
    let mut limit = RateLimit {
        per_sec: cfg.requests_per_sec,
        window_start: Instant::now(),
        count: 0,
    };
    let result = loop {
        let delivery = async {
            match &sub {
                Some(s) => s.recv().await,
                None => std::future::pending().await,
            }
        };
        let out: ServerMsg = tokio::select! {
            () = rt.lifecycle.shutdown_requested(), if rt.state().is_terminal() => break Ok(()),
            m = rx.recv() => match m {
                None => break Ok(()),
                Some(ClientMsg::Ping) => ServerMsg::Pong,
                Some(ClientMsg::Hello { .. }) => break Err(FrameError::Malformed),
                Some(ClientMsg::Request { req_id, body }) => {
                    let result = if limit.allow() {
                        dispatch(&rt, uid, granted, body, &mut sub).await
                    } else {
                        Err(RuntimeError::Busy) // §144 IPC flood
                    };
                    ServerMsg::Response { req_id, result }
                }
            },
            d = delivery => ServerMsg::Delivery(d),
        };
        match tokio::time::timeout(cfg.write_timeout, write_frame(&mut wr, &out)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => break Err(e),
            Err(_) => break Err(FrameError::Closed), // §145 slow client: disconnect, never buffer
        }
    };
    reader.abort();
    result
}

async fn dispatch(
    rt: &Runtime,
    uid: u32,
    role: Role,
    body: Request,
    sub: &mut Option<Subscription>,
) -> Result<Reply, RuntimeError> {
    match body {
        Request::Query(q) => rt.query(q).await,
        Request::Subscribe { topics, capacity } => {
            // Subscribe *first*, then read next_seq: nothing published in
            // between can be lost (at worst it is delivered and also covered
            // by the snapshot the client takes next — duplicates, never gaps).
            *sub = Some(
                rt.bus
                    .subscribe(topics, usize::from(capacity).clamp(1, 1024)),
            );
            Ok(Reply::Subscribed {
                next_seq: rt.bus.next_seq(),
            })
        }
        Request::Unsubscribe => {
            *sub = None;
            Ok(Reply::Ok)
        }
        Request::Command { id, cmd } => {
            if role < cmd.required_role() {
                return Err(RuntimeError::Unauthorized);
            }
            let bytes = rt.command(u64::from(uid), id, cmd).await?;
            Ok(if bytes.is_empty() {
                Reply::Ok
            } else {
                Reply::Bytes(bytes)
            })
        }
    }
}

// ───────────────────────────── client ─────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("daemon refused: {0}")]
    Runtime(#[from] RuntimeError),
    #[error("protocol violation: {0}")]
    Protocol(&'static str),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct IpcClient {
    stream: UnixStream,
    next_req: u64,
    pending: VecDeque<Delivery>,
    pub instance_id: u128,
    pub role: Role,
    pub version: u16,
}

impl IpcClient {
    pub async fn connect_unix(
        path: &Path,
        role: Role,
        client_name: &str,
    ) -> Result<Self, ClientError> {
        let mut stream = UnixStream::connect(path).await?;
        write_frame(
            &mut stream,
            &ClientMsg::Hello {
                min_version: MIN_PROTOCOL_VERSION,
                max_version: PROTOCOL_VERSION,
                role,
                client_name: client_name.to_owned(),
            },
        )
        .await?;
        match read_frame::<_, ServerMsg>(&mut stream).await? {
            ServerMsg::Welcome {
                version,
                instance_id,
                role,
                ..
            } => Ok(Self {
                stream,
                next_req: 1,
                pending: VecDeque::new(),
                instance_id,
                role,
                version,
            }),
            ServerMsg::Response { result: Err(e), .. } => Err(e.into()),
            _ => Err(ClientError::Protocol("expected Welcome")),
        }
    }

    pub async fn request(&mut self, body: Request) -> Result<Reply, ClientError> {
        let req_id = self.next_req;
        self.next_req += 1;
        write_frame(&mut self.stream, &ClientMsg::Request { req_id, body }).await?;
        loop {
            match read_frame::<_, ServerMsg>(&mut self.stream).await? {
                ServerMsg::Response { req_id: r, result } if r == req_id => return Ok(result?),
                ServerMsg::Response { .. } => {
                    return Err(ClientError::Protocol("unexpected response id"))
                }
                ServerMsg::Delivery(d) => {
                    if self.pending.len() < 4096 {
                        self.pending.push_back(d);
                    }
                }
                ServerMsg::Pong | ServerMsg::Welcome { .. } => {}
            }
        }
    }

    /// Mutating commands should carry a fresh id **per intent**; reuse the
    /// same id when retrying after a dropped connection (§57).
    pub async fn command(
        &mut self,
        id: CommandId,
        cmd: crate::protocol::Command,
    ) -> Result<Reply, ClientError> {
        self.request(Request::Command { id, cmd }).await
    }

    pub async fn query(&mut self, q: crate::protocol::Query) -> Result<Reply, ClientError> {
        self.request(Request::Query(q)).await
    }

    pub async fn subscribe(
        &mut self,
        topics: Vec<Topic>,
        capacity: u16,
    ) -> Result<u64, ClientError> {
        match self
            .request(Request::Subscribe { topics, capacity })
            .await?
        {
            Reply::Subscribed { next_seq } => Ok(next_seq),
            _ => Err(ClientError::Protocol("expected Subscribed")),
        }
    }

    pub async fn next_delivery(&mut self) -> Result<Delivery, ClientError> {
        if let Some(d) = self.pending.pop_front() {
            return Ok(d);
        }
        loop {
            if let ServerMsg::Delivery(d) = read_frame::<_, ServerMsg>(&mut self.stream).await? {
                return Ok(d);
            }
        }
    }
}

/// Default per-user socket path: `$XDG_RUNTIME_DIR/siar/<name>.sock` (§78),
/// falling back to `/tmp/siar-<uid>/<name>.sock` which `bind_secure` creates
/// `0700`.
#[must_use]
pub fn default_socket_path(name: &str) -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("siar").join(format!("{name}.sock")),
        _ => {
            let uid = std::fs::metadata("/proc/self").map_or(0, |m| m.uid());
            PathBuf::from(format!("/tmp/siar-{uid}")).join(format!("{name}.sock"))
        }
    }
}

const _: () = assert!(MAX_FRAME >= 64 * 1024);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Command, Query};
    use crate::runtime::{AppService, BoxFuture};
    use std::sync::atomic::{AtomicU32, Ordering};

    struct Echo(Arc<AtomicU32>);
    impl AppService for Echo {
        fn command<'a>(
            &'a self,
            _: &'a str,
            d: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async move {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(d)
            })
        }
        fn query<'a>(
            &'a self,
            _: &'a str,
            d: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async move { Ok(d) })
        }
    }

    fn tmp_sock(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("siar-ipc-{tag}-{}", std::process::id()))
            .join("d.sock")
    }

    async fn start(
        tag: &str,
        cfg: ServerConfig,
    ) -> (
        Runtime,
        PathBuf,
        Arc<AtomicU32>,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let n = Arc::new(AtomicU32::new(0));
        let rt = Runtime::builder().app(Echo(n.clone())).build();
        rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
            .await
            .unwrap();
        let path = tmp_sock(tag);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        let (rt2, p2) = (rt.clone(), path.clone());
        let h = tokio::spawn(async move { serve_unix(rt2, &p2, cfg).await });
        for _ in 0..200 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (rt, path, n, h)
    }

    #[tokio::test]
    async fn status_and_app_command_over_a_real_socket() {
        let (rt, path, n, _h) = start("basic", ServerConfig::default()).await;
        let mut c = IpcClient::connect_unix(&path, Role::Operator, "test")
            .await
            .unwrap();
        assert_eq!(c.role, Role::Operator);
        assert_eq!(c.instance_id, rt.bus.instance_id());
        let Reply::Status(s) = c.query(Query::Status).await.unwrap() else {
            panic!()
        };
        assert!(s.state.network_open());
        let r = c
            .command(
                CommandId(1),
                Command::App {
                    name: "e".into(),
                    data: vec![1, 2, 3],
                },
            )
            .await
            .unwrap();
        assert_eq!(r, Reply::Bytes(vec![1, 2, 3]));
        // Retry with the same id (as after a dropped connection): applied once.
        let mut c2 = IpcClient::connect_unix(&path, Role::Operator, "retry")
            .await
            .unwrap();
        c2.command(
            CommandId(1),
            Command::App {
                name: "e".into(),
                data: vec![1, 2, 3],
            },
        )
        .await
        .unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 1, "§57/§58 across reconnects");
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn socket_and_dir_permissions_are_private() {
        let (rt, path, _n, _h) = start("perm", ServerConfig::default()).await;
        let f = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let d = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(f, 0o600);
        assert_eq!(d, 0o700);
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn roles_are_enforced_and_capped_by_the_server() {
        let (rt, path, _n, _h) = start(
            "roles",
            ServerConfig {
                max_role: Role::Operator,
                ..Default::default()
            },
        )
        .await;
        let mut c = IpcClient::connect_unix(&path, Role::Admin, "greedy")
            .await
            .unwrap();
        assert_eq!(c.role, Role::Operator, "granted = min(requested, max)");
        let e = c
            .command(CommandId(1), Command::Shutdown)
            .await
            .unwrap_err();
        assert!(matches!(
            e,
            ClientError::Runtime(RuntimeError::Unauthorized)
        ));
        let mut obs = IpcClient::connect_unix(&path, Role::Observer, "obs")
            .await
            .unwrap();
        let e = obs
            .command(CommandId(2), Command::SetEmergency(true))
            .await
            .unwrap_err();
        assert!(matches!(
            e,
            ClientError::Runtime(RuntimeError::Unauthorized)
        ));
        assert!(obs.query(Query::Health).await.is_ok());
        assert_eq!(rt.state(), crate::lifecycle::RuntimeState::Ready);
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn admin_shutdown_stops_the_server_and_removes_the_socket() {
        let (rt, path, _n, h) = start("stop", ServerConfig::default()).await;
        let mut c = IpcClient::connect_unix(&path, Role::Admin, "admin")
            .await
            .unwrap();
        c.command(CommandId(1), Command::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), h)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!path.exists());
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn subscribers_get_events_and_a_snapshot_cursor() {
        let (rt, path, _n, _h) = start("sub", ServerConfig::default()).await;
        let mut c = IpcClient::connect_unix(&path, Role::Observer, "ui")
            .await
            .unwrap();
        let next = c
            .subscribe(vec![crate::runtime::topics::POWER], 16)
            .await
            .unwrap();
        rt.bus.publish(
            crate::runtime::topics::POWER,
            crate::events::EventPriority::Normal,
            None,
            vec![7],
        );
        let d = tokio::time::timeout(Duration::from_secs(2), c.next_delivery())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(d, Delivery::Event(e) if e.seq >= next && e.payload == vec![7]));
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn a_second_daemon_on_the_same_socket_is_refused_but_a_stale_file_is_reclaimed() {
        let (rt, path, _n, _h) = start("dup", ServerConfig::default()).await;
        let rt2 = Runtime::builder().build();
        let e = serve_unix(rt2, &path, ServerConfig::default())
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AddrInUse);
        rt.shutdown(Duration::from_secs(1)).await;

        // A crash leaves a dead socket file behind (§109): next start reclaims it.
        let stale = tmp_sock("stale");
        let _ = std::fs::remove_dir_all(stale.parent().unwrap());
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap()); // file stays, nobody listening
        assert!(stale.exists());
        let l = bind_secure(&stale).await;
        assert!(l.is_ok());
    }

    #[tokio::test]
    async fn aborting_the_server_still_removes_the_socket() {
        let (rt, path, _n, h) = start("abort", ServerConfig::default()).await;
        assert!(path.exists());
        h.abort();
        let _ = h.await;
        assert!(
            !path.exists(),
            "supervisor aborts at shutdown; the file must not outlive it"
        );
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn flood_is_rate_limited_not_fatal() {
        let (rt, path, _n, _h) = start(
            "flood",
            ServerConfig {
                requests_per_sec: 5,
                ..Default::default()
            },
        )
        .await;
        let mut c = IpcClient::connect_unix(&path, Role::Observer, "flood")
            .await
            .unwrap();
        let mut busy = 0;
        for _ in 0..30 {
            if matches!(
                c.query(Query::Status).await,
                Err(ClientError::Runtime(RuntimeError::Busy))
            ) {
                busy += 1;
            }
        }
        assert!(busy >= 20, "only {} of 30 limited", busy);
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn garbage_and_oversize_frames_close_only_that_connection() {
        use tokio::io::AsyncWriteExt;
        let (rt, path, _n, _h) = start("garbage", ServerConfig::default()).await;
        let mut raw = UnixStream::connect(&path).await.unwrap();
        raw.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        raw.write_all(&[0xde, 0xad]).await.unwrap();
        let mut ok = IpcClient::connect_unix(&path, Role::Observer, "good")
            .await
            .unwrap();
        assert!(
            ok.query(Query::Status).await.is_ok(),
            "daemon survived the hostile peer"
        );
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn too_many_connections_are_shed() {
        let (rt, path, _n, _h) = start(
            "cap",
            ServerConfig {
                max_connections: 2,
                ..Default::default()
            },
        )
        .await;
        let a = IpcClient::connect_unix(&path, Role::Observer, "a")
            .await
            .unwrap();
        let b = IpcClient::connect_unix(&path, Role::Observer, "b")
            .await
            .unwrap();
        let third = tokio::time::timeout(
            Duration::from_millis(500),
            IpcClient::connect_unix(&path, Role::Observer, "c"),
        )
        .await;
        assert!(
            third.is_err() || third.unwrap().is_err(),
            "third connection must not be served"
        );
        drop((a, b));
        rt.shutdown(Duration::from_secs(1)).await;
    }
}
