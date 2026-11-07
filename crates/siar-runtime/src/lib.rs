//! # siar-runtime — daemon & headless runtime (sys-arch Part 16)
//!
//! The piece that lets the communication core run **without a UI**: a
//! lifecycle state machine with a network barrier, supervised workers with a
//! circuit breaker, per-subsystem health, idempotent commands, bounded event
//! streams, a versioned/authenticated/bounded local IPC protocol, and
//! systemd integration — plus a [`siar_power`] scheduler owned by the runtime
//! so battery policy (Part 13) is available to every hosted service (§103).
//!
//! | module | spec | what |
//! |---|---|---|
//! | [`lifecycle`] | §32-36, 111-112 | state table, startup barrier |
//! | [`supervisor`] | §29-31, 97-98, 121, 142 | restarts, backoff, breaker |
//! | [`health`] | §89-90, 140-141 | per-subsystem health |
//! | [`dedup`] | §56-58 | `CommandId` idempotency |
//! | [`events`] | §23-25, 144-148, 169-171 | bounded bus, coalescing, gaps |
//! | [`protocol`] | §14-16, 19-22, 102-105 | framing, roles, versions |
//! | [`runtime`] | §51-54, 204 | `Runtime`, builder, `AppService` |
//! | [`ipc`] | §13-18, 144-146, 179-180 | local socket (Unix socket / Windows pipe) server + client |
//! | [`service`] | §37-39, 99, 174 | `sd-notify`, unit generation |
//! | [`backup`] | §111-115 | quiesced consistent backup, restore in Maintenance |
//! | [`diag`] | §93-96 | structured logs, trace ids, diagnostic bundle, crash report |
//! | [`node`] | §72-76 | node roles, feature sets, headless provisioning |
//!
//! ## Not done (named, not hidden)
//!
//! * Android service hosting and iOS execution windows (§42-46).
//! * Remote admin over Iroh (§66-71, 159-160, 182) lives in the `siar-admin` crate
//!   (its own endpoint, key and protocol); the daemon plugs it in via
//!   `[admin]` config. This crate only provides the local roles, the audit log
//!   and the command set it maps onto.
//! * Multi-account/tenant namespaces *inside one daemon* (§80-84): the
//!   path/namespace layer exists, per-account cores do not.
//! * Plugin-host isolation (§120); Unix FD passing (§63).
//! * [`node::Provisioner`] takes a [`node::ProvisionVerifier`]; **no real
//!   admin-key verifier is wired** (the daemon has no provisioning flow yet),
//!   so a node can only be provisioned by code that supplies one.
//! * [`backup::BackupService`] is a trait the hosting app must implement. The
//!   daemon does **not** implement it yet: `ctl backup` answers
//!   `Unsupported` until app-core exposes a flush-then-copy (stoolap has
//!   `create_snapshot` and a WAL; copying the files without that call would
//!   not be consistent).
//! * The sole-writer DB rule (§11) is by convention here — the hosting app
//!   owns the database.
//! * The desktop GUI attaches to the daemon through `siar-ui-sync`; the
//!   in-process mode (§4) remains the default.

#![forbid(unsafe_code)]

pub mod audit;
pub mod audit_file;
pub mod backup;
pub mod config;
pub mod dedup;
pub mod diag;
pub mod ensure;
pub mod error;
pub mod events;
pub mod health;
pub mod instance;
pub mod ipc;
pub mod lifecycle;
pub mod metrics;
pub mod node;
pub mod paths;
pub mod protocol;
pub mod runtime;
pub mod service;
pub mod supervisor;

pub use dedup::{CommandId, CommandLedger};
pub use error::RuntimeError;
pub use events::{Delivery, Event, EventBus, EventPriority, Subscription, Topic};
pub use health::{HealthRegistry, HealthReport, HealthState};
pub use lifecycle::{Lifecycle, RuntimeState};
pub use protocol::{Command, Query, Reply, Request, Role, StatusSnapshot};
pub use runtime::{topics, AppService, BoxFuture, NoApp, Runtime, RuntimeBuilder};
pub use supervisor::{RestartPolicy, Supervisor};
