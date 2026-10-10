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
//!   optimistic-concurrency guard, and §55's flat
//!   [`store::validate_payload_size`] backstop (per-type limits are
//!   [`limits::EventLimits`]), enforced by every real backend before a
//!   write is even attempted.
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
//! ## Phase 5: outbox, effects, retry, recovery
//!
//! - [`outbox`] — §14/§15/§31: deterministic [`outbox::OperationId`]s,
//!   the [`outbox::Outbox`] queue (leases, idempotent enqueue), and
//!   [`outbox::TransactionalOutbox`] — events and outbox rows committed
//!   in one transaction by both [`InMemoryEventStore`] and
//!   [`StoolapEventStore`].
//! - [`stoolap_outbox`] — the durable table, sharing the event store's
//!   database and write lock; behaviourally identical to the in-memory
//!   reference (property-tested).
//! - [`command_flow`] — §13/§29/§30: pure `Decider`, `EffectPlanner`,
//!   and `CommandFlow` (decide → plan → atomic commit → project; effects
//!   only ever run later).
//! - [`effects`] — §30/§32/§45/§46: `EffectProcessor`, `BackoffPolicy`,
//!   attempt metrics vs. meaningful-transition sink, and the
//!   `ReplayMode` gate that keeps projection replay from running effects.
//! - [`outbox_reconcile`] — §48/§47: rebuild a lost or drifted outbox
//!   from history with the original operation ids.
//!
//! ## Phase 6: replication scopes, cursors, ordered intake
//!
//! - [`replication`] — §49-52: [`replication::ReplicationScope`] with
//!   a fail-closed [`replication::ScopeTable`], the §51
//!   [`replication::ReplicationAuthorizer`] seam (`Allow`/`Skip`/`Defer`),
//!   the separate wire type [`replication::ReplicationEventV1`], and
//!   [`replication::next_outbound_batch`]. Includes the stream-layout
//!   rule found while testing: a replicated stream must contain only
//!   replicable events, enforced by `OutboundBatch::has_version_holes`.
//! - [`sync_cursor`] — §53: monotonic, acknowledgement-driven cursors,
//!   in-memory and `stoolap`-backed.
//! - [`reassembly`] — §25/§26: hold-then-apply out-of-order intake with
//!   bounded buffers that refuse loudly rather than drop, and
//!   divergence reported rather than resolved (§54).
//! - [`store::EventStore::stream_head`] — a provided method (O(1) in
//!   both real backends), so adding it broke no implementor.
//!
//! ## Phase 7: snapshots, retention, deletion, backup, migration
//!
//! - [`redaction`] — the single physical-removal primitive: empties a
//!   payload, keeps the envelope (so §24 idempotency, §12 versions,
//!   §53 cursors and the §22 checksum all still hold).
//! - [`retention`] — §40: `RetentionClass`, fail-keep `RetentionTable`,
//!   `plan_compaction`/`apply_compaction`, outbox-aware protection.
//! - [`snapshot`] — §38/§39: checksummed snapshots; `load_state` is
//!   provably equivalent to full replay and discards anything it can't
//!   trust.
//! - [`deletion`] — §41/§42: `execute_deletion` across log, snapshots
//!   and outbox; `SealedPayload`/`PayloadSealer` seam for crypto-erasure
//!   with key management kept outside the store.
//! - [`backup`] — §70/§71: hash-chained incremental export, verified
//!   restore, and `restore_outbox` with a `RestoreGuard` so restore
//!   never blindly resends.
//! - [`upcast`] — §9: deterministic, explicit `V(n)→V(n+1)` upcasting
//!   that refuses future versions and never rewrites the log.
//!
//! Every module above is covered by tests exercising real behavior —
//! actual concurrent-writer rejection, actual duplicate-event
//! deduplication, actual gap detection on the spec's own worked
//! example, actual close-and-reopen disk persistence, actual corrupted-
//! row detection, an actual retry-and-succeed-after-a-conflict run, an
//! actual version-bump-triggers-a-full-rebuild run — not just type
//! shapes.
//!
//! ## Phase 7b: closing the §55-66 gaps
//!
//! - [`limits`] — §55 per-event-type size limits with an absolute hard
//!   ceiling; both stores take them via `with_limits`.
//! - [`unknown`] — §62 `Apply` / `StoreAndIgnore` / `Block` policy; an
//!   unknown event blocks by default.
//! - [`namespace`] — §64/§65 isolation by separate stores per
//!   namespace (a shared log cannot be filtered back to one tenant),
//!   filesystem-safe `NamespaceId`s, and a registry that hands each
//!   tenant only its own handle.
//! - [`integrity`] — §66 rollback and tamper detection via hash chains
//!   and externally stored anchors; the module doc maps every §66 threat
//!   to the mechanism that answers it.
//!
//! §58 (indexes), §59 (bounded replay) and §60 (projection isolation)
//! needed no new module: the indexes §58 names exist in
//! [`stoolap_store`]; every reader in this crate pages with an explicit
//! limit (`ProjectionRunner`, [`reassembly`], [`backup::export_since`],
//! [`retention::plan_compaction`], [`snapshot::load_state`]); and
//! [`command_flow::CommandFlow::commit_and_project`] shows a failing
//! secondary projection leaving the durable commit untouched.
//!
//! ## What's still NOT here
//!
//! - §23's remote-ingestion pipeline itself — protocol, identity and
//!   authorization checks live in `siar-remote-ingestion`; this crate
//!   has no dependency on `siar-identity-multidevice` or
//!   `siar-protocol-ext` for that reason.
//! - §27 hybrid logical clocks beyond `stream_version` (the spec itself
//!   says "later"), §43 blob references (Part 05), §44 search as a
//!   projection, §72+ analytics separation.
//! - Per-stream projection checkpoints (checkpoints are per log offset,
//!   which is why [`unknown::triage`] blocks globally rather than per
//!   stream).
//! - Real implementations of the domain-supplied seams
//!   ([`replication::ReplicationAuthorizer`],
//!   [`backup::RestoreGuard`], [`deletion::PayloadSealer`]) — only the
//!   fail-closed placeholders exist here; `siar-messaging::event_policy`
//!   supplies messaging's guard and planner.
//! - A cargo-fuzz harness and criterion benchmarks.
//! - A schema-version column for `stoolap_store`'s own tables: their
//!   `CREATE TABLE IF NOT EXISTS` has no version, a real gap for
//!   whoever makes the first breaking change to one.

pub mod backup;
pub mod command_flow;
pub mod deletion;
pub mod durability;
pub mod effects;
pub mod envelope;
pub mod fault_injection;
pub mod gap;
pub mod ids;
pub mod integrity;
pub mod limits;
pub mod memory_store;
pub mod metrics;
pub mod namespace;
pub mod outbox;
pub mod outbox_reconcile;
pub mod projection;
pub mod read_only;
pub mod reassembly;
pub mod redaction;
pub mod replication;
pub mod retention;
pub mod retry;
pub mod snapshot;
pub mod stoolap_checkpoint_store;
pub mod stoolap_outbox;
pub mod stoolap_store;
pub mod store;
pub mod sync_cursor;
pub mod unknown;
pub mod upcast;

pub use durability::DurabilityClass;
pub use envelope::{EventEnvelope, EventOrigin};
pub use fault_injection::{FaultInjectingEventStore, FaultMode};
pub use gap::{detect_gap, StreamGap};
pub use ids::{CorrelationId, EventId, EventTypeId, LocalLogOffset, StreamId, Timestamp};
pub use memory_store::InMemoryEventStore;
pub use metrics::{MetricsEventStore, MetricsSnapshot};
pub use projection::{
    CatchUpOutcome, InMemoryCheckpointStore, Projection, ProjectionCheckpoint,
    ProjectionCheckpointStore, ProjectionError, ProjectionId, ProjectionRunner, ReplayMode,
    DEFAULT_CATCH_UP_BATCH_SIZE,
};
pub use read_only::ReadOnlyEventStore;
pub use retry::append_with_retry;
pub use stoolap_checkpoint_store::StoolapCheckpointStore;
pub use stoolap_store::StoolapEventStore;
pub use store::{
    validate_payload_size, AppendRequest, AppendResult, EventStore, EventStoreError, NewEvent,
    StoredEvent, DEFAULT_MAX_EVENT_PAYLOAD_BYTES,
};
