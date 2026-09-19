#![forbid(unsafe_code)]

//! siar-event-log: a first slice of "Part 04 — Offline Event Log
//! Architecture" (the fourth of the architecture documents supplied so
//! far — Part 01 has `siar-protocol-ext`, Part 02 has
//! `siar-identity-multidevice`, Part 03 has `siar-routing-policy`, all
//! in this same workspace).
//!
//! ## Scope: §92 "Implementation Phases" 1, made real (not just typed)
//!
//! - [`ids`] — §4's ID types plus §6/§28: `EventId` (UUIDv4 — see that
//!   type's own doc comment on why not a time-sortable variant yet),
//!   `StreamId` (§4's `[u8; 32]`, with [`ids::StreamId::from_name`]
//!   bridging §5's structured stream-naming convention via a
//!   deterministic blake3 hash), `CorrelationId`, `EventTypeId`,
//!   `LocalLogOffset`, `Timestamp`.
//! - [`envelope`] — §4 `EventEnvelope`, §7 `EventOrigin` (reusing
//!   `siar_domain::DeviceId`, a real dependency, not a parallel id).
//! - [`store`] — §20 `EventStore` trait, verbatim signatures (via
//!   `async-trait`, already a real workspace dependency for exactly
//!   this — see that module's own doc comment), plus §21's batch
//!   `AppendRequest`/`AppendResult` and §12's `expected_version`
//!   optimistic-concurrency guard.
//! - [`memory_store`] — [`memory_store::InMemoryEventStore`], a real,
//!   fully-tested implementation of the trait above: §11 atomic batch
//!   append (all-or-nothing under one lock), §12 concurrency-conflict
//!   rejection, §24 idempotent no-op on a repeated `EventId`. Not a
//!   durable backend — see [`stoolap_store`] for that.
//! - [`gap`] — §26 gap detection as a pure function, for the remote
//!   ingestion path (§23) to call.
//!
//! ## Phase 2: a real durable backend
//!
//! - [`stoolap_store`] — [`stoolap_store::StoolapEventStore`], §19's
//!   recommended SQLite-class backend made real against `stoolap`
//!   (this workspace's own pure-Rust embedded SQL engine, per the root
//!   `Cargo.toml`'s explicit "replaces SQLite/rusqlite" instruction),
//!   independent of `siar-storage`'s own unrelated schema — see that
//!   module's own doc comment for the full schema, the §22 integrity
//!   checksum, and the concurrency strategy. This is the piece the
//!   crate's own doc comment used to list as "explicitly NOT here";
//!   it now is.
//!
//! Every module above is covered by tests exercising real behavior —
//! actual concurrent-writer rejection, actual duplicate-event
//! deduplication, actual gap detection on the spec's own worked
//! example, actual close-and-reopen disk persistence, actual corrupted-
//! row detection — not just type shapes.
//!
//! ## What's explicitly NOT here
//!
//! Everything past Phase 2: §9's versioned-schema upcasting machinery,
//! §13/§14's local-first command flow and transactional outbox
//! (application-level patterns this crate's trait *enables* but doesn't
//! itself implement), §16-18 projections/checkpoints/read-your-writes
//! (Phase 4), §23 the full remote-ingestion pipeline (protocol/identity/
//! authorization validation — this crate has no dependency on
//! `siar-identity-multidevice` or `siar-protocol-ext` for that reason;
//! only [`gap::detect_gap`] serves that path), §25's hold-for-dependency
//! out-of-order handling (`detect_gap` reports a gap; it doesn't hold or
//! reorder anything), §27 hybrid logical clocks beyond what
//! `stream_version` already provides, §29-32 pure decision functions/
//! effect processing conventions (a pattern this crate's trait supports
//! but doesn't enforce or provide a type for), §33-37 domain-specific
//! event catalogs (messaging/file/identity/DTN/emergency — those belong
//! in the crates that own those domains, defining their own
//! `EventTypeId` constants and payload schemas against this trait, not
//! in this crate), §38-39 snapshotting (Phase 7), §40-41 compaction/
//! retention/deletion, §49-54 replication scope/sync cursors, §55
//! per-event-type size limits, §56 durability classes as an actual type,
//! §62-65 unknown-event handling/namespacing/multi-tenant isolation,
//! §70-71 backup/restore, §81-88 diagnostics/metrics/property-fuzz-
//! crash-injection test harnesses, §89's own suggested finer-grained
//! module split (`codec.rs`/`registry.rs`/`retention.rs`/`replay.rs`/
//! `diagnostics.rs`/`error.rs` as separate files — this crate keeps
//! `store.rs`'s `EventStoreError` as the one error type rather than
//! splitting it out yet), and the schema-migration story `stoolap_store`
//! doesn't have (its `CREATE TABLE IF NOT EXISTS` has no version column
//! — a real gap for whoever makes the first breaking schema change).

pub mod envelope;
pub mod gap;
pub mod ids;
pub mod memory_store;
pub mod stoolap_store;
pub mod store;

pub use envelope::{EventEnvelope, EventOrigin};
pub use gap::{detect_gap, StreamGap};
pub use ids::{CorrelationId, EventId, EventTypeId, LocalLogOffset, StreamId, Timestamp};
pub use memory_store::InMemoryEventStore;
pub use stoolap_store::StoolapEventStore;
pub use store::{AppendRequest, AppendResult, EventStore, EventStoreError, NewEvent, StoredEvent};
