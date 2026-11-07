//! `comm-admin/1` — remote administration of a headless node (spec 16
//! §66-§71, §159-§160, §182).
//!
//! The rules this crate enforces, one per spec line:
//!
//! * **Separate transport, separate identity, separate protocol** (§66, §68,
//!   §159, §182): its own Iroh endpoint with its *own* key, ALPN
//!   `comm-admin/1`, never the local `comm-ipc/1` and never the messaging
//!   identity. The local IPC protocol is not exposed.
//! * **Authenticated** (§68, §70): the QUIC handshake proves the peer's
//!   Ed25519 key; it must be on the node's operator allow-list or the
//!   connection is closed before a byte of it is parsed.
//! * **Authorised** (§69, §70): each operator has one of four ordered roles
//!   (`Viewer < Operator < SecurityAdmin < Owner`); every operation names the
//!   role it needs ([`AdminOp::required_role`]).
//! * **Audited** (§70): every request — executed, forbidden or rate-limited —
//!   is reported to the backend's audit sink.
//! * **Rate-limited** (§70): a per-operator token bucket, a cap on
//!   simultaneous connections, size caps on frames, and timeouts.
//! * **Explicit operations only** (§67, §71): [`AdminOp`] is a closed list —
//!   no shell, no pass-through to application commands, and nothing that
//!   reads message content. Enrolment (provisioning) is deliberately *not*
//!   an admin operation: it stays local.
//! * **Advertised only to the authorised** (§160): the endpoint is never
//!   published unless the operator turns discovery on; by default it is
//!   reachable only by someone who already holds its id and address.
//!
//! The crate knows nothing about the daemon: it talks to an [`AdminBackend`].
#![forbid(unsafe_code)]

pub mod acl;
pub mod client;
pub mod keys;
pub mod limits;
pub mod protocol;
pub mod server;

pub use acl::{AclError, Operator, OperatorTable};
pub use client::{AdminClient, ClientError};
pub use protocol::{AdminError, AdminOp, AdminRole, ALPN};
pub use server::{AdminBackend, AdminServer, AdminServerConfig, AuditKind, OperatorContext};
