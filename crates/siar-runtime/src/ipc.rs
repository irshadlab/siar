//! §13-§20, §144-§146, §179-§180: local IPC over an OS-native local socket.
//!
//! The transport is the maintained [`interprocess`] crate's `local_socket`
//! (Unix domain socket on Unix, named pipe on Windows) — one code path, not a
//! `cfg(unix)` server plus a missing Windows one.
//!
//! Security model (§17, §179):
//!
//! * **Unix** — the socket lives in a `0700` directory and is itself `0600`
//!   (set with `fchmod` *before* `bind`, so there is no umask window). Every
//!   connection's peer euid (`SO_PEERCRED`, read through `interprocess`'s
//!   `peer_creds()`) must equal the socket owner's uid or be explicitly
//!   allow-listed.
//! * **Windows** — a named pipe has no peer-uid; the guarantee comes from the
//!   pipe's **DACL** instead: only the pipe's owner may open it, and remote
//!   (SMB) clients are rejected by `interprocess` unless asked for. See
//!   [`owner_only_security_descriptor`]. `allowed_uids` has no meaning there.
//!
//! There is no password to leak and no TCP port to scan. Roles (§19) are
//! granted as `min(requested, configured max)` and every command is checked
//! against its required role (§20).
//!
//! Backpressure (§144-§146): bounded connections, a per-connection request
//! rate limit, a bounded request queue, a write timeout (a client that does
//! not read is disconnected, never buffered without limit), and bounded
//! event subscriptions with gap reporting (see [`crate::events`]).

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::traits::tokio::{Listener as _, Stream as _};
use interprocess::local_socket::traits::StreamCommon as _;
use interprocess::local_socket::{ListenerOptions, Name, ToFsName as _};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, Semaphore};

use crate::audit::{action_label, AuditEntry, Outcome};
use crate::dedup::CommandId;
use crate::error::RuntimeError;
use crate::events::{Delivery, Subscription, Topic};
use crate::metrics::Metrics;
use crate::protocol::{
    negotiate_version, read_frame, write_frame, ClientMsg, FrameError, Reply, Request, Role,
    ServerMsg, MAX_FRAME, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
};
use crate::runtime::Runtime;

/// Audit principal for the owning user on platforms where the peer has no
/// numeric uid (Windows). The DACL already restricts the pipe to that user,
/// so every connection that gets this far *is* the owner.
pub const OWNER_PRINCIPAL_NO_UID: u64 = 0x5749_4e00;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_connections: usize,
    pub requests_per_sec: u32,
    pub hello_timeout: Duration,
    pub write_timeout: Duration,
    /// Extra uids allowed besides the socket owner (Unix only; ignored where
    /// the peer has no uid — access there is the pipe's DACL).
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

/// The §179 same-user rule as a pure function: the peer must be the socket
/// owner or on the allow-list.
#[must_use]
pub fn uid_allowed(peer: u32, owner: u32, allowed: &[u32]) -> bool {
    peer == owner || allowed.contains(&peer)
}

/// Decide whether `stream`'s peer may talk to us, returning its audit
/// principal.
fn authenticate_peer(stream: &Stream, owner: Owner, cfg: &ServerConfig) -> Option<u64> {
    let creds = stream.peer_creds().ok()?;
    #[cfg(unix)]
    {
        let uid = creds.euid()?;
        if !uid_allowed(uid, owner, &cfg.allowed_uids) {
            tracing::warn!(uid, "rejected ipc peer: uid not allowed");
            return None;
        }
        Some(u64::from(uid))
    }
    #[cfg(not(unix))]
    {
        let _ = (owner, cfg);
        tracing::debug!(pid = ?creds.pid(), "ipc peer accepted (pipe DACL is the access check)");
        Some(OWNER_PRINCIPAL_NO_UID)
    }
}

#[cfg(unix)]
type Owner = u32;
#[cfg(not(unix))]
type Owner = ();

#[cfg(unix)]
fn socket_owner(path: &Path) -> io::Result<Owner> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.uid())
}
#[cfg(not(unix))]
fn socket_owner(_: &Path) -> io::Result<Owner> {
    Ok(())
}

/// The local-socket name for `path`: the path itself on Unix; on Windows a
/// namespaced pipe name derived from the file stem plus a stable hash of the
/// full path, so two data folders never share a pipe.
fn local_name(path: &Path) -> io::Result<Name<'static>> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::GenericFilePath;
        path.to_path_buf().to_fs_name::<GenericFilePath>()
    }
    #[cfg(not(unix))]
    {
        use interprocess::local_socket::{GenericNamespaced, ToNsName as _};
        pipe_name_for(path).to_ns_name::<GenericNamespaced>()
    }
}

/// Stable (not `DefaultHasher`, which may change between toolchains) pipe
/// name, so a CLI and a daemon built by different compilers still agree.
#[cfg_attr(unix, allow(dead_code))]
fn pipe_name_for(path: &Path) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.to_string_lossy().to_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let stem = path
        .file_stem()
        .map_or_else(|| "ipc".into(), |s| s.to_string_lossy().into_owned());
    format!("siar-{stem}-{h:016x}")
}

/// §180: the Windows pipe ACL — **only the pipe's owner** gets access (no
/// Everyone, no Users, no Administrators-by-group), the Windows counterpart
/// of the Unix `0600` + peer-uid check.
///
/// Not compiled or exercised in this repository's Linux CI: it is the one
/// piece of this module that needs a real Windows machine to verify.
#[cfg(windows)]
pub fn owner_only_security_descriptor(
) -> io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    // D:P = protected DACL (no inheritance); one ACE: Allow, Generic-All, to Owner-Rights.
    SecurityDescriptor::deserialize(widestring::u16cstr!("D:P(A;;GA;;;OW)"))
}

/// Bind the local socket safely and serve until the runtime begins shutting
/// down.
pub async fn serve_local(rt: Runtime, path: &Path, cfg: ServerConfig) -> io::Result<()> {
    // `listener`'s drop unlinks the socket file (interprocess "name
    // reclamation"), however this future ends — including being aborted by the
    // supervisor at shutdown, which never reaches the tail of the function.
    let listener = bind_local(path).await?;
    let owner = socket_owner(path)?;
    let slots = Arc::new(Semaphore::new(cfg.max_connections));
    let cfg = Arc::new(cfg);
    tracing::info!(socket = %path.display(), "ipc listening");

    loop {
        tokio::select! {
            () = rt.lifecycle.shutdown_requested() => break,
            accepted = listener.accept() => {
                let Ok(stream) = accepted else { continue };
                let Some(principal) = authenticate_peer(&stream, owner, &cfg) else {
                    continue;
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    tracing::warn!("ipc connection limit reached; dropping");
                    continue;
                };
                let (rt, cfg) = (rt.clone(), cfg.clone());
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = handle_connection(rt, stream, principal, &cfg).await {
                        tracing::debug!(error = %e, "ipc connection ended");
                    }
                });
            }
        }
    }
    Ok(())
}

/// §109 stale-socket recovery + §179 permissions.
async fn bind_local(path: &Path) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(dir) = path.parent() {
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        if path.exists() {
            // Never displace a live daemon: only a file nobody answers on is stale.
            if Stream::connect(local_name(path)?).await.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another instance is serving this socket",
                ));
            }
            std::fs::remove_file(path)?; // stale file from a crash
        }
    }
    let make = |private_mode: bool| -> io::Result<ListenerOptions<'static>> {
        let opts = ListenerOptions::new().name(local_name(path)?);
        #[cfg(unix)]
        let opts = if private_mode {
            use interprocess::os::unix::local_socket::ListenerOptionsExt as _;
            opts.mode(0o600)
        } else {
            opts
        };
        #[cfg(windows)]
        let opts = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt as _;
            let _ = private_mode;
            opts.security_descriptor(owner_only_security_descriptor()?)
        };
        #[cfg(not(any(unix, windows)))]
        let _ = private_mode;
        Ok(opts)
    };
    match make(true)?.create_tokio() {
        Ok(l) => Ok(l),
        // A platform that cannot choose the file mode at bind time: bind, then
        // tighten. The 0700 parent directory still covers the gap.
        #[cfg(unix)]
        Err(e) if e.kind() == io::ErrorKind::Unsupported => {
            use std::os::unix::fs::PermissionsExt;
            let l = make(false)?.create_tokio()?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(l)
        }
        Err(e) => Err(e),
    }
}

/// One half of a shared [`Stream`]. `interprocess` deprecated `split()`
/// because `&Stream` is itself `AsyncRead + AsyncWrite`; an `Arc` gives each
/// of the reader task and the writer its own handle with no lock.
struct Half(Arc<Stream>);

impl AsyncRead for Half {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Half {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.0).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_shutdown(cx)
    }
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
    stream: Stream,
    principal: u64,
    cfg: &ServerConfig,
) -> Result<(), FrameError> {
    Metrics::inc(&rt.metrics.ipc_connections);
    let stream = Arc::new(stream);
    let (mut rd, mut wr) = (Half(stream.clone()), Half(stream));

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
                    Metrics::inc(&rt.metrics.ipc_requests);
                    let result = if limit.allow() {
                        dispatch(&rt, principal, granted, body, &mut sub).await
                    } else {
                        Metrics::inc(&rt.metrics.ipc_rate_limited);
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
    principal: u64,
    role: Role,
    body: Request,
    sub: &mut Option<Subscription>,
) -> Result<Reply, RuntimeError> {
    match body {
        Request::Query(q) => {
            if role < q.required_role() {
                Metrics::inc(&rt.metrics.ipc_unauthorized);
                return Err(RuntimeError::Unauthorized);
            }
            rt.query(q).await
        }
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
            let label = action_label(&cmd);
            let entry = |outcome| AuditEntry {
                at_ms: unix_ms(),
                principal,
                role,
                action: label.clone(),
                outcome,
            };
            if role < cmd.required_role() {
                Metrics::inc(&rt.metrics.ipc_unauthorized);
                rt.audit.record(entry(Outcome::Denied));
                return Err(RuntimeError::Unauthorized);
            }
            match rt.command(principal, id, cmd).await {
                Ok(bytes) => {
                    rt.audit.record(entry(Outcome::Ok));
                    Ok(if bytes.is_empty() {
                        Reply::Ok
                    } else {
                        Reply::Bytes(bytes)
                    })
                }
                Err(e) => {
                    rt.audit.record(entry(Outcome::Failed));
                    Err(e)
                }
            }
        }
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
    stream: Stream,
    next_req: u64,
    pending: VecDeque<Delivery>,
    pub instance_id: u128,
    pub role: Role,
    pub version: u16,
}

impl IpcClient {
    pub async fn connect_local(
        path: &Path,
        role: Role,
        client_name: &str,
    ) -> Result<Self, ClientError> {
        let mut stream = Stream::connect(local_name(path)?).await?;
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

/// Default per-user socket path (§78).
///
/// Unix: `$XDG_RUNTIME_DIR/siar/<name>.sock`, falling back to
/// `/tmp/siar-<uid>/<name>.sock` which `bind_local` creates `0700`.
/// Windows: `%LOCALAPPDATA%\siar\<name>.sock` — the file is never created;
/// it only identifies the per-user pipe (see [`pipe_name_for`]).
#[must_use]
pub fn default_socket_path(name: &str) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(d) if !d.is_empty() => PathBuf::from(d).join("siar").join(format!("{name}.sock")),
            _ => {
                let uid = std::fs::metadata("/proc/self").map_or(0, |m| m.uid());
                PathBuf::from(format!("/tmp/siar-{uid}")).join(format!("{name}.sock"))
            }
        }
    }
    #[cfg(not(unix))]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("siar").join(format!("{name}.sock"))
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

    /// Cross-platform "is the server up": on Windows there is no file to look for.
    async fn reachable(path: &Path) -> bool {
        match local_name(path) {
            Ok(n) => Stream::connect(n).await.is_ok(),
            Err(_) => false,
        }
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
        let h = tokio::spawn(async move { serve_local(rt2, &p2, cfg).await });
        for _ in 0..400 {
            if reachable(&path).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        (rt, path, n, h)
    }

    #[tokio::test]
    async fn status_and_app_command_over_a_real_socket() {
        let (rt, path, n, _h) = start("basic", ServerConfig::default()).await;
        let mut c = IpcClient::connect_local(&path, Role::Operator, "test")
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
        let mut c2 = IpcClient::connect_local(&path, Role::Operator, "retry")
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

    #[cfg(unix)]
    #[tokio::test]
    async fn socket_and_dir_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
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

    #[test]
    fn uid_rule_is_owner_or_allow_listed_only() {
        assert!(uid_allowed(1000, 1000, &[]));
        assert!(!uid_allowed(1001, 1000, &[]));
        assert!(uid_allowed(1001, 1000, &[1001]));
        assert!(
            !uid_allowed(0, 1000, &[1001]),
            "root is not implicitly trusted"
        );
    }

    /// The principal in the audit log is the peer's *kernel-reported* euid
    /// (`SO_PEERCRED` via `interprocess`), not anything the client claims.
    #[cfg(unix)]
    #[tokio::test]
    async fn audit_principal_is_the_peers_kernel_reported_uid() {
        let (rt, path, _n, _h) = start("principal", ServerConfig::default()).await;
        let me = socket_owner(&path).unwrap();
        let mut admin = IpcClient::connect_local(&path, Role::Admin, "who")
            .await
            .unwrap();
        admin
            .command(
                CommandId(1),
                Command::App {
                    name: "e".into(),
                    data: vec![],
                },
            )
            .await
            .unwrap();
        let Reply::Bytes(b) = admin.query(Query::Audit).await.unwrap() else {
            panic!()
        };
        let text = String::from_utf8(b).unwrap();
        assert!(text.contains(&format!("uid={me}\t")), "{text}");
        rt.shutdown(Duration::from_secs(1)).await;
    }

    /// `interprocess` exposes the connecting process on both platforms.
    #[tokio::test]
    async fn peer_creds_report_this_process_as_the_peer() {
        use interprocess::local_socket::traits::tokio::Listener as _;
        let path = tmp_sock("creds");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        #[cfg(unix)]
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let l = bind_local(&path).await.unwrap();
        let client = tokio::spawn({
            let p = path.clone();
            async move { Stream::connect(local_name(&p).unwrap()).await.unwrap() }
        });
        let server_side = l.accept().await.unwrap();
        let _client = client.await.unwrap();
        let creds = server_side.peer_creds().unwrap();
        assert_eq!(creds.pid(), Some(std::process::id() as _));
        #[cfg(unix)]
        assert_eq!(creds.euid(), Some(socket_owner(&path).unwrap()));
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
        let mut c = IpcClient::connect_local(&path, Role::Admin, "greedy")
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
        let mut obs = IpcClient::connect_local(&path, Role::Observer, "obs")
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
        let mut c = IpcClient::connect_local(&path, Role::Admin, "admin")
            .await
            .unwrap();
        c.command(CommandId(1), Command::Shutdown).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), h)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!reachable(&path).await);
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn subscribers_get_events_and_a_snapshot_cursor() {
        let (rt, path, _n, _h) = start("sub", ServerConfig::default()).await;
        let mut c = IpcClient::connect_local(&path, Role::Observer, "ui")
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
    async fn a_second_daemon_on_the_same_socket_is_refused() {
        let (rt, path, _n, _h) = start("dup", ServerConfig::default()).await;
        let rt2 = Runtime::builder().build();
        let e = serve_local(rt2, &path, ServerConfig::default())
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AddrInUse);
        rt.shutdown(Duration::from_secs(1)).await;
    }

    // A crash leaves a dead socket file behind (§109): next start reclaims it.
    // (Unix only: a Windows pipe vanishes with its last handle.)
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stale_socket_file_is_reclaimed() {
        let stale = tmp_sock("stale");
        let _ = std::fs::remove_dir_all(stale.parent().unwrap());
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap()); // file stays, nobody listening
        assert!(stale.exists());
        let l = bind_local(&stale).await;
        assert!(l.is_ok());
    }

    #[tokio::test]
    async fn aborting_the_server_still_removes_the_socket() {
        let (rt, path, _n, h) = start("abort", ServerConfig::default()).await;
        assert!(path.exists());
        h.abort();
        let _ = h.await;
        assert!(
            !reachable(&path).await,
            "supervisor aborts at shutdown; the listener must not outlive it"
        );
        #[cfg(unix)]
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
        let mut c = IpcClient::connect_local(&path, Role::Observer, "flood")
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
        let mut raw = Stream::connect(local_name(&path).unwrap()).await.unwrap();
        raw.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        raw.write_all(&[0xde, 0xad]).await.unwrap();
        let mut ok = IpcClient::connect_local(&path, Role::Observer, "good")
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
        let a = IpcClient::connect_local(&path, Role::Observer, "a")
            .await
            .unwrap();
        let b = IpcClient::connect_local(&path, Role::Observer, "b")
            .await
            .unwrap();
        let third = tokio::time::timeout(
            Duration::from_millis(500),
            IpcClient::connect_local(&path, Role::Observer, "c"),
        )
        .await;
        assert!(
            third.is_err() || third.unwrap().is_err(),
            "third connection must not be served"
        );
        drop((a, b));
        rt.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn audit_records_actions_never_payloads_and_is_admin_only() {
        let (rt, path, _n, _h) = start("audit", ServerConfig::default()).await;
        let mut op = IpcClient::connect_local(&path, Role::Operator, "op")
            .await
            .unwrap();
        // `max_role` defaults to Admin, so request Operator explicitly above.
        op.command(
            CommandId(1),
            Command::App {
                name: "e".into(),
                data: b"TOP-SECRET-TEXT".to_vec(),
            },
        )
        .await
        .unwrap();
        let denied = op
            .command(CommandId(2), Command::Shutdown)
            .await
            .unwrap_err();
        assert!(matches!(
            denied,
            ClientError::Runtime(RuntimeError::Unauthorized)
        ));
        assert!(
            matches!(
                op.query(Query::Audit).await.unwrap_err(),
                ClientError::Runtime(RuntimeError::Unauthorized)
            ),
            "Operator cannot read the audit log"
        );

        let mut admin = IpcClient::connect_local(&path, Role::Admin, "admin")
            .await
            .unwrap();
        let Reply::Bytes(b) = admin.query(Query::Audit).await.unwrap() else {
            panic!()
        };
        let text = String::from_utf8(b).unwrap();
        assert!(text.contains("app:e") && text.contains("Ok"), "{text}");
        assert!(
            text.contains("shutdown") && text.contains("Denied"),
            "{text}"
        );
        assert!(
            !text.contains("TOP-SECRET"),
            "§162: no content in the audit log"
        );

        let Reply::Bytes(m) = admin.query(Query::Metrics).await.unwrap() else {
            panic!()
        };
        let m = String::from_utf8(m).unwrap();
        assert!(m.contains("siar_ipc_unauthorized_total 2"), "{m}");
        rt.shutdown(Duration::from_secs(1)).await;
    }
}
