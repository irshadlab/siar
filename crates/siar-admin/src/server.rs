//! The `comm-admin/1` Iroh server.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use tokio::sync::Semaphore;

use crate::acl::{Operator, OperatorTable};
use crate::limits::{RateConfig, RateLimiter};
use crate::protocol::{
    AdminError, AdminOp, AdminRole, Reply, Request, ALPN, MAX_REPLY_BYTES, MAX_REQUEST_BYTES,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Who is asking, for the backend (never the raw key in logs — the label).
#[derive(Debug, Clone)]
pub struct OperatorContext {
    pub key: [u8; 32],
    pub label: String,
    pub role: AdminRole,
    pub request_id: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    /// Ran and succeeded.
    Ok,
    /// Ran and failed.
    Failed(AdminError),
    /// Authenticated but the role is too low.
    Forbidden,
    RateLimited,
    /// Bytes that did not decode as a request.
    Malformed,
    /// A connection from a key that is not an operator.
    UnknownPeer,
    /// Too many simultaneous connections.
    Overloaded,
}

/// What the hosting node provides. `execute` is only ever called for an
/// authenticated, authorised, rate-limit-approved request.
pub trait AdminBackend: Send + Sync + 'static {
    fn execute<'a>(
        &'a self,
        ctx: &'a OperatorContext,
        op: AdminOp,
    ) -> BoxFuture<'a, Result<Vec<u8>, AdminError>>;

    /// Called for **every** request outcome (§70 "audited"), including the
    /// ones that never reach `execute`. `who` is `None` for an unknown peer.
    fn audit(&self, who: Option<&OperatorContext>, label: &str, kind: AuditKind);
}

#[derive(Debug, Clone)]
pub struct AdminServerConfig {
    /// Publish the endpoint through Iroh's public discovery. **Off by
    /// default** (§160): without it only someone who already holds the node's
    /// id and address can reach it.
    pub discovery: bool,
    pub bind_addr: Option<SocketAddr>,
    pub rate: RateConfig,
    /// Simultaneous authenticated connections.
    pub max_connections: usize,
    /// Time to receive a request once a stream is open.
    pub request_timeout: Duration,
    /// Time an operation may run.
    pub exec_timeout: Duration,
    /// How long an idle connection is kept.
    pub idle_timeout: Duration,
}

impl Default for AdminServerConfig {
    fn default() -> Self {
        Self {
            discovery: false,
            bind_addr: None,
            rate: RateConfig::default(),
            max_connections: 8,
            request_timeout: Duration::from_secs(10),
            exec_timeout: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(60),
        }
    }
}

struct Shared {
    table: OperatorTable,
    backend: Arc<dyn AdminBackend>,
    limiter: RateLimiter,
    connections: Arc<Semaphore>,
    /// Requests currently being handled (read → execute → reply written).
    inflight: AtomicUsize,
    cfg: AdminServerConfig,
}

/// Counts a request as in flight for as long as it lives.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn new(c: &'a AtomicUsize) -> Self {
        c.fetch_add(1, Ordering::SeqCst);
        Self(c)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct Handler(Arc<Shared>);

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminHandler")
    }
}

pub struct AdminServer {
    endpoint: Endpoint,
    router: Router,
    shared: Arc<Shared>,
}

impl AdminServer {
    /// Bind the admin endpoint under `secret` (the node's **admin** key —
    /// not its messaging key) and start serving.
    pub async fn spawn(
        cfg: AdminServerConfig,
        secret: SecretKey,
        table: OperatorTable,
        backend: Arc<dyn AdminBackend>,
    ) -> Result<Self, String> {
        let mut builder = if cfg.discovery {
            Endpoint::builder(presets::N0)
        } else {
            Endpoint::builder(presets::Minimal)
        }
        .secret_key(secret);
        if let Some(addr) = cfg.bind_addr {
            builder = builder.bind_addr(addr).map_err(|e| e.to_string())?;
        }
        let endpoint = builder.bind().await.map_err(|e| e.to_string())?;
        let shared = Shared {
            limiter: RateLimiter::new(cfg.rate),
            connections: Arc::new(Semaphore::new(cfg.max_connections.max(1))),
            inflight: AtomicUsize::new(0),
            table,
            backend,
            cfg,
        };
        let shared = Arc::new(shared);
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, Handler(shared.clone()))
            .spawn();
        Ok(Self {
            endpoint,
            router,
            shared,
        })
    }

    #[must_use]
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    #[must_use]
    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }

    /// Stop serving. Requests already being handled are allowed to finish and
    /// have their reply delivered (bounded by `DRAIN`) — otherwise an operator
    /// who asked the node to shut down would see "connection lost" for an
    /// operation that in fact succeeded.
    pub async fn shutdown(self) {
        const DRAIN: Duration = Duration::from_secs(5);
        let deadline = Instant::now() + DRAIN;
        while self.shared.inflight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = self.router.shutdown().await;
    }
}

fn reject(connection: &Connection, code: u32, why: &'static [u8]) {
    connection.close(code.into(), why);
}

impl ProtocolHandler for Handler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let s = &self.0;
        let remote = *connection.remote_id().as_bytes();

        // §68/§70 authentication: not on the list → closed before reading anything.
        let Some(op) = s.table.lookup(&remote).cloned() else {
            tracing::warn!("comm-admin: connection from a key that is not an operator; closed");
            s.backend.audit(None, "connect", AuditKind::UnknownPeer);
            reject(&connection, 1, b"unauthorized");
            return Ok(());
        };
        let ctx0 = context(&op, 0);
        let Ok(_permit) = s.connections.clone().try_acquire_owned() else {
            s.backend
                .audit(Some(&ctx0), "connect", AuditKind::Overloaded);
            reject(&connection, 2, b"busy");
            return Ok(());
        };

        loop {
            let streams = tokio::time::timeout(s.cfg.idle_timeout, connection.accept_bi()).await;
            let Ok(Ok((mut send, mut recv))) = streams else {
                break; // idle, or the peer closed
            };
            self.one_request(&op, &mut send, &mut recv).await;
        }
        Ok(())
    }
}

fn context(op: &Operator, request_id: u128) -> OperatorContext {
    OperatorContext {
        key: op.key,
        label: op.label.clone(),
        role: op.role,
        request_id,
    }
}

impl Handler {
    async fn one_request(
        &self,
        op: &Operator,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) {
        let s = &self.0;
        let _counted = InFlight::new(&s.inflight);
        let bytes =
            match tokio::time::timeout(s.cfg.request_timeout, recv.read_to_end(MAX_REQUEST_BYTES))
                .await
            {
                Ok(Ok(b)) => b,
                _ => {
                    s.backend
                        .audit(Some(&context(op, 0)), "unreadable", AuditKind::Malformed);
                    return;
                }
            };
        // Exactly one request, nothing trailing.
        let req = match postcard::take_from_bytes::<Request>(&bytes) {
            Ok((r, [])) => r,
            _ => {
                s.backend
                    .audit(Some(&context(op, 0)), "malformed", AuditKind::Malformed);
                respond(send, 0, Err(AdminError::Invalid)).await;
                return;
            }
        };
        let ctx = context(op, req.id);
        let label = req.op.label();

        // Cheap checks first, in a fixed order: rate limit, then role.
        if !s.limiter.allow(&op.key, Instant::now()) {
            s.backend.audit(Some(&ctx), label, AuditKind::RateLimited);
            respond(send, req.id, Err(AdminError::RateLimited)).await;
            return;
        }
        if op.role < req.op.required_role() {
            s.backend.audit(Some(&ctx), label, AuditKind::Forbidden);
            respond(send, req.id, Err(AdminError::Forbidden)).await;
            return;
        }

        let result =
            match tokio::time::timeout(s.cfg.exec_timeout, s.backend.execute(&ctx, req.op.clone()))
                .await
            {
                Ok(r) => r,
                Err(_) => Err(AdminError::Timeout),
            };
        s.backend.audit(
            Some(&ctx),
            label,
            match &result {
                Ok(_) => AuditKind::Ok,
                Err(e) => AuditKind::Failed(*e),
            },
        );
        respond(send, req.id, result).await;
    }
}

async fn respond(
    send: &mut iroh::endpoint::SendStream,
    id: u128,
    result: Result<Vec<u8>, AdminError>,
) {
    let mut reply = Reply { id, result };
    let mut bytes = postcard::to_allocvec(&reply).unwrap_or_default();
    if bytes.len() > MAX_REPLY_BYTES {
        // Never send something the client will refuse to read.
        reply.result = Err(AdminError::Internal);
        bytes = postcard::to_allocvec(&reply).unwrap_or_default();
    }
    if send.write_all(&bytes).await.is_ok() {
        let _ = send.finish();
        // Let the peer read the reply before the stream is dropped.
        let _ = send.stopped().await;
    }
}
