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
//!   `AppendRequest`/`AppendResult`, §12's `expected_version`
//!   optimistic-concurrency guard, and §55's own
//!   [`store::validate_payload_size`] — one uniform size ceiling
//!   (§55 itself invites a per-event-type registry; none exists yet
//!   anywhere in this workspace, so this is a real, if simple, first
//!   cut — see that function's own doc comment), enforced by every
//!   real backend before a write is even attempted.
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
//! - [`retry`] — [`retry::append_with_retry`], the optimistic-
//!   concurrency retry loop a real multi-caller stream needs on top of
//!   `append`'s own deliberately-simple "reject a stale
//!   `expected_version`" contract. Not part of Phase 2's own list —
//!   added once Phase 3's real callers (below) needed it, rather than
//!   speculatively ahead of any caller.
//!
//! ## Phase 3: real domain event catalogs, and real callers
//!
//! §33/§34/§35's three named domains each got a typed event catalog in
//! the crate that owns that domain — not in this crate, per §1's own
//! "independent of any one database" reasoning applied the same way to
//! "independent of any one domain": `siar_identity_multidevice::
//! audit_log` (§35), `siar_messaging::events` (§33),
//! `siar_blob_manifest::events` (§34). This crate has no dependency on
//! any of the three (dependencies point the other way), so it doesn't
//! know their event types exist — [`retry::append_with_retry`] above is
//! the one piece of shared, domain-agnostic machinery this crate
//! contributes back for them to use.
//!
//! §33's own real caller closes what used to be this crate's own
//! honestly-named gap ("no real caller anywhere"): `siar_messaging::
//! MessageService::record_messaging_event` calls
//! [`retry::append_with_retry`] from three real places in that crate's
//! send/receive code (`send_text`, `handle_incoming`'s new-message and
//! `DeliveryAck` arms) — see that crate's own `service.rs` for exactly
//! which of §33's nine events currently have a real call site and
//! which don't yet (edit/delete/react and read receipts have no
//! calling feature to hang off yet). Identity's and files' own catalogs
//! remain construct-only for now — same gap, just not this round's.
//!
//! ## Phase 4: projections, checkpoints, read-your-writes
//!
//! - [`projection`] — §16 `Projection`/"Projection Runner", §17
//!   `ProjectionCheckpoint` (verbatim fields),
//!   [`projection::ProjectionRunner::catch_up`] as the pull-based
//!   runner, and an honest accounting of what §18 "read-your-writes"
//!   does and doesn't mean without redesigning `EventStore::append`
//!   itself — see that module's own doc comment for the full picture.
//!   `siar_messaging`'s new `conversation_summary` projection is the
//!   first real [`projection::Projection`] anywhere in this workspace
//!   (see that crate's own `projections.rs`), called synchronously
//!   right after `record_messaging_event`'s own `append` succeeds —
//!   §18's concrete example ("SendMessage succeeds locally →
//!   conversation immediately shows message"), made real.
//! - [`stoolap_checkpoint_store`] — [`stoolap_checkpoint_store::
//!   StoolapCheckpointStore`], the durable counterpart to
//!   [`projection::InMemoryCheckpointStore`], same `stoolap`-backed
//!   pattern [`stoolap_store::StoolapEventStore`] set for
//!   [`store::EventStore`] itself. Built ahead of any caller that
//!   needs it — every real `Projection` in this workspace so far is
//!   itself in-memory, so nothing currently NEEDS a durable checkpoint
//!   — on the reasoning that checkpoint durability is foundational
//!   enough to get right before, not after, the first durable
//!   projection exists; see that module's own doc comment for exactly
//!   why pairing a durable projection with an in-memory checkpoint
//!   would be actively wrong, not just less thorough.
//!
//! Every module above is covered by tests exercising real behavior —
//! actual concurrent-writer rejection, actual duplicate-event
//! deduplication, actual gap detection on the spec's own worked
//! example, actual close-and-reopen disk persistence, actual corrupted-
//! row detection, an actual retry-and-succeed-after-a-conflict run, an
//! actual version-bump-triggers-a-full-rebuild run — not just type
//! shapes.
//!
//! ## What's explicitly NOT here
//!
//! Everything past Phase 4: §9's versioned-schema upcasting machinery,
//! §13/§14's local-first command flow and transactional outbox
//! (application-level patterns this crate's trait *enables* but doesn't
//! itself implement — Phase 5), §23 the full remote-ingestion pipeline
//! (protocol/identity/authorization validation — this crate has no
//! dependency on `siar-identity-multidevice` or `siar-protocol-ext` for
//! that reason; only [`gap::detect_gap`] serves that path), §25's
//! hold-for-dependency out-of-order handling (`detect_gap` reports a
//! gap; it doesn't hold or reorder anything), §27 hybrid logical clocks
//! beyond what `stream_version` already provides, §29-32 pure decision
//! functions/effect processing conventions (a pattern this crate's
//! trait supports but doesn't enforce or provide a type for), §38-39
//! snapshotting (Phase 7), §40-41 compaction/
//! retention/deletion, §49-54 replication scope/sync cursors, §56
//! durability classes as an actual type,
//! §62-65 unknown-event handling/namespacing/multi-tenant isolation,
//! §70-71 backup/restore, §81-88 diagnostics/metrics/property-fuzz-
//! crash-injection test harnesses, §89's own suggested finer-grained
//! module split (`codec.rs`/`registry.rs`/`retention.rs`/`replay.rs`/
//! `diagnostics.rs`/`error.rs` as separate files — this crate keeps
//! `store.rs`'s `EventStoreError` as the one error type rather than
//! splitting it out yet), and the schema-migration story
//! `stoolap_store`/`stoolap_checkpoint_store` don't have (their
//! `CREATE TABLE IF NOT EXISTS` has no version column — a real gap for
//! whoever makes the first breaking schema change to either). A
//! durable `Projection` implementation itself doesn't exist yet either
//! — `stoolap_checkpoint_store` exists ahead of that caller, per that
//! module's own doc comment on why.

pub mod envelope;
pub mod gap;
pub mod ids;
pub mod memory_store;
pub mod projection;
pub mod retry;
pub mod stoolap_checkpoint_store;
pub mod stoolap_store;
pub mod store;

pub use envelope::{EventEnvelope, EventOrigin};
pub use gap::{detect_gap, StreamGap};
pub use ids::{CorrelationId, EventId, EventTypeId, LocalLogOffset, StreamId, Timestamp};
pub use memory_store::InMemoryEventStore;
pub use projection::{
    InMemoryCheckpointStore, Projection, ProjectionCheckpoint, ProjectionCheckpointStore,
    ProjectionError, ProjectionId, ProjectionRunner, DEFAULT_CATCH_UP_BATCH_SIZE,
};
pub use retry::append_with_retry;
pub use stoolap_checkpoint_store::StoolapCheckpointStore;
pub use stoolap_store::StoolapEventStore;
pub use store::{
    validate_payload_size, AppendRequest, AppendResult, EventStore, EventStoreError, NewEvent,
    StoredEvent, DEFAULT_MAX_EVENT_PAYLOAD_BYTES,
};
