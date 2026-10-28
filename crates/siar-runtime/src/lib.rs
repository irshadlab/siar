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
//! | `ipc` (unix) | §13-18, 144-146, 179 | socket server + client |
//! | [`service`] | §37-39, 99, 174 | `sd_notify`, unit generation |
//!
//! ## Not done (named, not hidden)
//!
//! Windows named-pipe transport (§180); Android service hosting and iOS
//! execution windows (§42-46); remote admin over Iroh (§66-70, 159-162);
//! multi-account/tenant namespaces (§80-84); plugin-host isolation (§120);
//! graceful *upgrade* handoff (§100-101, 106-107); Unix FD passing (§63);
//! the sole-writer DB enforcement (§11) is by convention here — the hosting
//! app owns the database; and the **desktop GUI is not yet a daemon client**:
//! `apps/desktop` still embeds its core in-process (§4 In-Process Mode),
//! because its `Core` is coupled to the Slint hub (see ROADMAP).

#![forbid(unsafe_code)]

pub mod dedup;
pub mod error;
pub mod events;
pub mod health;
#[cfg(unix)]
pub mod ipc;
pub mod lifecycle;
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
