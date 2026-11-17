# Part 04 — Offline Event Log Architecture

Source spec: `sys-arch/04-offline-event-log-architecture.md` — 95 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**27 ✅ done / 24 🟡 partial / 35 ⬜ not started / 9 ◇ conceptual, of 95** (as of 2026-09-22).

Phases 1-4 of the spec's own §92 phase structure are done. Phase 5 (transactional outbox/effects/retry/recovery) is **deliberately deferred, not decided** — see "Phase 5 decision" below. Phases 6 (replication) and 7 (snapshots/compaction/crash/fuzz/bench) haven't been started.

## Implementing crate(s)

- `siar-event-log` — the core crate: `EventStore` trait, `InMemoryEventStore`, `StoolapEventStore`, `Projection`/`ProjectionRunner`, `append_with_retry`, `ReadOnlyEventStore`, `MetricsEventStore`, `FaultInjectingEventStore`.
- `siar-messaging` — first, and by far most complete, real domain caller (§33 Messaging Events).
- `siar-blob-manifest` (§34 File Events, `events.rs`), `siar-identity-multidevice` (§35 Identity Events, pre-existing `audit_log`), `siar-dtn-bundle` (§36 DTN Events), `siar-emergency` (§37 Emergency Events) — the other four domain event catalogs.
- `siar-file-transfer-service`, `siar-identity-audit-recorder`, `siar-dtn-bundle-service`, `siar-emergency-service` — real caller wiring from each domain's own state machine to `EventStore::append_with_retry` (constructed; not yet driven by a live `apps/*` call site).
- `siar-remote-ingestion` (§23 Remote Event Ingestion), `siar-startup-recovery` (§47/§48), `siar-event-notify` (§61), `siar-event-registry` (§63) — cross-cutting infrastructure crates.

## Full section-by-section tracker

Built 2026-09-22, at the user's own explicit request ("track section in 04 offline event log specs, along update it how much section in which specs completed"), after Phases 1-4 were already done by round. Every one of the spec's 95 numbered sections, checked against what's actually in the crate/its domain callers, not assumed from which Phase it nominally belongs to.

**Legend**: ✅ done and real (code exists, is tested, matches the section) · 🟡 partial (something real exists but doesn't fully satisfy the section — see the note) · ⬜ not started · ◇ conceptual/guidance section with no code artifact of its own.

| § | Title | Status | Note |
|---|---|---|---|
| 1 | Purpose | ◇ | Informs everything; no artifact of its own. |
| 2 | Do Not Event-Source Everything | ◇ | Followed: only 3 domains (messaging/files/identity) have catalogs, not "everything." (Now 5, since DTN/emergency closed later this session.) |
| 3 | Command vs Event | ◇ | Conceptual distinction; reflected in `NewEvent` vs domain command handling, not a type. |
| 4 | Core Event Envelope | ✅ | `EventEnvelope` (Phase 1). |
| 5 | Streams | ✅ | `StreamId`, per-stream version (Phase 1/2). |
| 6 | Local Global Offset | ✅ | `LocalLogOffset` (Phase 1/2). |
| 7 | Event Origin | ✅ | `EventOrigin` (Phase 1). |
| 8 | Correlation and Causation | 🟡 | Real end-to-end in `siar-messaging` (`MessageCorrelation` registry, in-memory only — a process restart between two correlated events loses the chain by design, not bug). Identity/files/DTN/emergency still always `None` — no real workflow to correlate yet. |
| 9 | Versioned Event Schemas | ✅ | `schema_version`-checked decode in all 3 original domain catalogs (found as a real latent bug in `siar-messaging` first — `postcard::from_bytes` was decoding with zero regard for the stored version — then fixed everywhere it applied); DTN/emergency built with the check from day one. |
| 10 | Append-Only Semantics | ✅ | No update/delete API on stored events, by construction. |
| 11 | Atomic Append | ✅ | `stoolap` transaction + process mutex (Phase 2). |
| 12 | Optimistic Concurrency | ✅ | Version check + `ConcurrencyConflict` + `append_with_retry` (Phase 2). |
| 13 | Local-First Command Flow | ✅ | `send_text`'s existing persist→record→network-send ordering already matched this; confirmed and named in comments. |
| 14 | Transactional Outbox | 🟡 | Resilience test (`send_text_survives_a_completely_broken_event_log`) proves a broken event log doesn't break the real outbox. True cross-store atomicity, and the "extend vs build a second outbox" decision, both still open — **see Phase 5 decision below.** |
| 15 | Event Log Is Not a Job Queue | ✅ | Outbox retry logic uses its own `due()`, never scans the event log. |
| 16 | Projection Architecture | ✅ | `Projection` trait + `ProjectionRunner` (Phase 4). |
| 17 | Projection Checkpoints | ✅ | `ProjectionCheckpoint` + in-memory AND durable (`StoolapCheckpointStore`) backends. |
| 18 | Read-Your-Writes | ✅ | Real, via synchronous catch-up after append — honestly documented as "sync-after," not "same transaction" (a real, smaller guarantee than the section's own ideal). |
| 19 | Event Store Backend | ✅ | `StoolapEventStore` (Phase 2). |
| 20 | Event Store Trait | ✅ | `EventStore` (Phase 1). |
| 21 | Batch Append | ✅ | `AppendRequest.events: Vec<NewEvent>`, all-or-nothing (Phase 1/2). |
| 22 | Integrity | ✅ | blake3 checksum per row, verified on read (Phase 2). |
| 23 | Remote Event Ingestion | 🟡 | `RemoteIngestionPipeline` in `siar-remote-ingestion` enforcing the 9-step safe flow (receive → protocol validation → identity verification → authorization → deduplication → domain validation → append → projection → durable ACK); gates ACK strictly after persistence. |
| 24 | Idempotency | ✅ | Duplicate `event_id` is a no-op, tested (Phase 1/2). |
| 25 | Out-of-Order Events | 🟡 | `detect_gap` reports a gap; nothing holds or reorders — reporting only, no remediation. |
| 26 | Gap Detection | ✅ | `detect_gap` (Phase 1), tested against the spec's own worked example. |
| 27 | Logical Clocks | 🟡 | `stream_version` provides real per-stream ordering; no hybrid logical clock beyond that. |
| 28 | Offline IDs | ✅ | `EventId`/`CorrelationId` are locally-generated `Uuid` v4 — collision-resistant, offline, stable across retries. Not time-sortable (not v7) — the section calls this an optional locality improvement, not a correctness requirement. |
| 29 | Pure Decision Functions | 🟡 | Pure `decide(state, command, id) -> Result<(State, Vec<DomainEvent>), Error>` implemented and tested across files (`siar-blob-manifest`), emergency (`siar-emergency`), and DTN (`siar-dtn-bundle`), with caller services delegating to them. |
| 30 | Effect Processing | 🟡 | `record_messaging_event`'s catch-up call is effect-adjacent but ad hoc — no formal effect-processing pattern/type exists. |
| 31 | Exactly-Once Is Not the Goal | ◇ | Reflected in §24's own idempotent-no-op design; no artifact of its own. |
| 32 | Retry Event Granularity | ◇ | Reflected in `append_with_retry`'s own per-event retry design; no artifact of its own. |
| 33 | Messaging Events | ✅ | Full: catalog, real `append` caller (3 call sites), real correlation, real durable projection. The most complete domain by far. |
| 34 | File Events | 🟡 | Catalog real and tested, §9 schema-version fix applied. `siar-file-transfer-service` provides real `FileTransferService` caller wiring `TransferState` transitions to `EventStore::append` via `append_with_retry`. |
| 35 | Identity Events | 🟡 | Catalog real and tested, §9 fixed. `siar-identity-audit-recorder` provides real `IdentityAuditRecorder` caller wiring `audit_log` event construction to `EventStore::append` via `append_with_retry`. |
| 36 | DTN Events | 🟡 | `siar-dtn-bundle::events` (2026-09-22): 9 events, `EventTypeId` 300-308, tested. `siar-dtn-bundle-service` provides real `DtnBundleService` caller wiring `BundleState` transitions to `EventStore::append` via `append_with_retry`. |
| 37 | Emergency Events | 🟡 | `siar-emergency::events` (2026-09-22): 6 events, `EventTypeId` 400-405, tested; new `ReportId` added. `siar-emergency-service` provides real `EmergencyReportService` caller wiring `ReportStatus` transitions to `EventStore::append` via `append_with_retry`. |
| 38 | Snapshotting | ⬜ | Phase 7, not started. |
| 39 | Snapshot Structure | ⬜ | Phase 7, not started. |
| 40 | Compaction | ⬜ | Not started. |
| 41 | Privacy and Deletion | ⬜ | Not started. |
| 42 | Event Encryption | 🟡 | Messaging's own `ciphertext` field is already application-encrypted upstream (by `siar-crypto`, before it reaches this crate). No local-database-at-rest encryption exists — `stoolap` files this crate writes are plain, unencrypted files. |
| 43 | Blob References | ✅ | File events reference `BlobId`/`ManifestId`, never raw bytes. Messaging embeds small ciphertext directly by design (text content, not "large binary data"). |
| 44 | Search as Projection | ⬜ | No FTS projection exists. |
| 45 | Replay Must Not Re-run Side Effects | 🟡 | True by construction today — the only real `Projection` has zero side effects, so replay is safe. Never stress-tested against a side-effecting projection, since none exists yet. |
| 46 | Replay Modes | 🟡 | `ReplayMode` enum (`ProjectionOnly`, `Recovery`, `Live`), `CatchUpOutcome`, `ProjectionRunner::catch_up_with_mode` implemented and tested. External work gating ready for orchestration callers (§47/§48). |
| 47 | Startup Recovery | 🟡 | `StartupSequence` in `siar-startup-recovery` enforcing the 6-phase sequential pipeline (`OpenDb` → `VerifyMigrations` → `ResumeProjections` → `ReconcileWorkQueues` → `LoadReadModels` → `StartNetworking`). Ready for application startup integration. |
| 48 | Work Queue Reconciliation | 🟡 | `reconcile`/`pending_from_history` in `siar-startup-recovery` computing `ReconciliationReport` against historical event streams. Ready for domain work queue wiring. |
| 49 | Replication Scope | ⬜ | No `ReplicationScope` enum. |
| 50 | Own-Device Sync | ⬜ | Not started. |
| 51 | Peer and Group Sync | ⬜ | Not started. |
| 52 | Local Storage Envelope vs Network Envelope | ⬜ | `StoredEvent` used directly everywhere a wire form would be needed — no `ReplicationEventV1` transform layer (moot until real replication exists). |
| 53 | Sync Cursors | ⬜ | No `SyncCursor` type. |
| 54 | Conflicts Are Domain-Specific | ◇ | Correctly not building one generic resolver — nothing exists yet to point to as "done" either, since no real conflicts have arisen. |
| 55 | Event Size Limits | ✅ | `validate_payload_size`/`DEFAULT_MAX_EVENT_PAYLOAD_BYTES` (256 KiB, 2026-09-22), enforced by both real backends before any write. One uniform ceiling, not yet the per-event-type registry the section's own text invites — a deliberate, named first cut. |
| 56 | Durability Classes | 🟡 | `DurabilityClass` enum (`Critical`, `Durable`, `BestEffort`, 2026-09-25), with per-variant classification implemented and tested across all 5 domain event catalogs. |
| 57 | SQL Schema | ✅ | `events`/`stream_heads` (Phase 2) plus `projection_checkpoints`/`conversation_summaries` (Phase 4). |
| 58 | Indexes | ✅ | Unique indexes on offset/event_id/(stream,version) plus each new table's own (Phase 2/4). |
| 59 | Memory Discipline | ✅ | `ProjectionRunner::catch_up` reads bounded batches (`batch_size`), discards between them — matches the section's own diagram exactly. |
| 60 | Projection Isolation | 🟡 | A broken event log doesn't stop already-succeeded appends (tested); a projection catch-up failure is logged and swallowed, not fatal. Cross-projection isolation isn't demonstrated — only one real projection exists. |
| 61 | Internal Event Notifications | 🟡 | `siar-event-notify` provides decoupled wake/notify via `ChangeNotifier`/`NotifyingEventStore`, broadcasting on real non-duplicate appends. Still needs live subscribers/projections wired in `apps/*`. |
| 62 | Unknown Events | 🟡 | "Optional unknown → store/ignore safely" real and tested (projection skips foreign event-type ranges). "Required semantic unknown → block stream until upgrade" doesn't exist. |
| 63 | Namespaced Custom Events | 🟡 | `siar-event-registry` (2026-09-22): a real, running, tested cross-domain collision check over all 41 `EventTypeId` constants across all five domains — proven to actually fail on a real collision. Still manually maintained (nothing auto-adds a new domain's constant). |
| 64 | Multi-Tenant Isolation | ⬜ | No `TenantId` concept anywhere. |
| 65 | Multiple Identities | ⬜ | No isolation between personal/work identities on one device. |
| 66 | Security | 🟡 | Malformed-imported-event, duplicate/replay, rollback, oversized-payload, and unauthorized-remote-event validated and tested via `RemoteIngestionPipeline` (§23); local corruption covered via checksums (§22) and `ReadOnlyEventStore` (§69); projection poisoning explicitly documented as requiring future `apply` sandboxing. |
| 67 | Event Store Errors | 🟡 | `EventStoreError` includes `ConcurrencyConflict`/`Backend`/`Corrupt`/`StreamNotFound` plus `StorageFull` (§68) and `ReadOnly` (§69). |
| 68 | Storage Full Behavior | 🟡 | `EventStoreError::StorageFull` implemented; `InMemoryEventStore::with_capacity(max)` simulates exhaustion, rejects appends crossing capacity before state mutation, tested. |
| 69 | Read-Only Recovery Mode | 🟡 | `ReadOnlyEventStore<S>` wrapper (2026-09-25) — blocks `append` with `EventStoreError::ReadOnly` while allowing unrestricted `read_stream`/`read_log`, tested. |
| 70 | Backup | ⬜ | `read_log(from_offset, ...)` gives exactly the primitive §70 asks for, but no backup tooling is built on top of it. |
| 71 | Restore Safety | ⬜ | Not started. |
| 72 | Analytics Separation | ◇ | No analytics pipeline exists yet to separate anything from. |
| 73 | Dioxus Boundary | ⬜ | No UI crate in this workspace's uploaded scope to assess against. |
| 74 | Kotlin / iOS Boundary | ⬜ | No mobile platform code in this workspace's uploaded scope. |
| 75 | Daemon Compatibility | ⬜ | Not assessed — no daemon architecture seen in the uploaded crates. |
| 76 | Headless Compatibility | 🟡 | Satisfied by omission — nothing built so far couples to a UI — but never explicitly declared or tested as a requirement. |
| 77 | Routing Integration | ⬜ | No wiring between `siar-routing-policy` and `MessageQueued`/the event log. |
| 78 | File Integration | ✅ | `FileEvent`'s catalog is transfer-level only — no per-chunk event — matching this section's own "semantic transfer events, not one event per chunk" exactly, by design. |
| 79 | DTN Integration | ⬜ | Not started (as its own tracked item; DTN's own event catalog is §36 above). |
| 80 | Emergency Integration | ⬜ | Not started (as its own tracked item; Emergency's own event catalog is §37 above). |
| 81 | Diagnostics | ⬜ | Not started. |
| 82 | Metrics | 🟡 | `MetricsEventStore<S>` wrapper (2026-09-25) — captures 4 of the spec's 8 metrics at the store append boundary: append latency, events/sec, duplicate rate, event-store size, tested. |
| 83 | Property Tests | ✅ | All 6 spec invariants covered via `proptest` arbitrary-case generation: invariants 1–5 in `siar-event-log/tests/property_invariants.rs`; invariant 6 (expired pending work not resurrected) via `pending_from_history_with_expiry` in `siar-startup-recovery`. |
| 84 | Crash Injection Tests | ✅ | `FaultInjectingEventStore` wrapper and all 5 spec crash points (before append, inside transaction, after event before projection, after projection before network effect, after network effect before success marker) tested in `tests/crash_injection.rs` with deterministic recovery. |
| 85 | Fuzzing | ⬜ | Not started. |
| 86 | Golden Event Tests | ⬜ | No fixed-byte-encoding tests for any event schema. |
| 87 | Recovery Acceptance Test | ⬜ | The exact composed scenario doesn't exist as one test, though several of its pieces are covered separately by other tests. |
| 88 | Multi-Device Offline Test | ⬜ | Not started. |
| 89 | Suggested Crate Structure | 🟡 | Deliberately deviated, and said so from the first Phase 2 round onward: kept one `EventStoreError` rather than splitting into the suggested `codec.rs`/`registry.rs`/`retention.rs`/`replay.rs`/`diagnostics.rs`/`error.rs`. |
| 90 | Public API | 🟡 | `EventStore`/`ProjectionRunner` match the suggested short list; no separate `EventAppender`/`EventReader`/`SnapshotStore` types — `EventStore` covers append+read in one trait. |
| 91 | Initial Production Scope | 🟡 | Of the 11 "implement first" items: 9 done (SQLite store, stream versioning, global offset, unique IDs, batch append, projection checkpoints, replay, schema versioning, and — partially — critical projections). Outbox integration and basic snapshots are the two genuinely missing. |
| 92 | Implementation Phases | 🟡 | This section IS the master phase structure this document tracks by. Phases 1-4 done; 5 (outbox/effects/retry/recovery), 6 (replication), 7 (snapshots/compaction/crash/fuzz/bench) remain. |
| 93 | Definition of Done | 🟡 | Self-audited against all 21 items — see the dedicated subsection below; roughly 7 done, 9 partial, 5 not started. |
| 94 | Relationship to Other Parts | ◇ | Cross-references to other specs — see the Phase 5 decision notes below for the one place this actually mattered (Part 03/`siar-routing-policy`, and `siar-storage`'s own pre-existing outbox). |
| 95 | Final Principle | ◇ | The one-sentence guarantee ("if the app says a durable operation was accepted, that intent survives network loss and process termination") this whole spec exists to make true. Partially true today: real for messaging's own send path; not yet extended to files, identity, DTN, or emergency, none of which have a real caller. |

### §93 Definition of Done — full 21-item self-audit

Reading the section's own bullets literally, in order:

1. accepted local commands survive process death — ✅ (real file close/reopen durability tests, three different `stoolap`-backed stores)
2. operations can be accepted without Internet — ✅ (local-first by construction; `append` has no network dependency)
3. IDs require no central server — ✅ (`Uuid` v4, generated locally)
4. stream versions provide deterministic ordering — ✅
5. duplicate remote events are idempotent — ✅ (tested)
6. projections rebuild deterministically — 🟡 (mechanism real and tested; determinism depends on each `Projection`'s own `apply` being pure, not generally enforced)
7. checkpoints recover after crashes — ✅ (durable checkpoint store, tested)
8. UI reads optimized projections — ⬜ (no UI layer in scope here to wire this to)
9. large data is referenced via blobs — ✅ (file events; message text ciphertext is small content, not "large data," by design)
10. message outbox survives restart — ✅ (`siar-storage`'s own outbox, pre-existing, confirmed still working)
11. file semantic state survives restart — 🟡 (`siar-file-transfer-service` wires transitions to durable events; live `apps/*` caller remaining)
12. device lifecycle remains auditable — 🟡 (`siar-identity-audit-recorder` wires `audit_log` to durable append; live caller remaining)
13. SOS is persisted before transmission — 🟡 (`siar-emergency-service` persists report/SOS lifecycle before network transmission; live caller remaining)
14. DTN lifecycle is durable — 🟡 (`siar-dtn-bundle-service` wires `BundleState` transitions to durable append; live caller remaining)
15. replication scope is explicit — ⬜ (§49 not built)
16. event schemas are versioned — ✅ (§9, all domains)
17. replay never accidentally re-runs external effects — 🟡 (true by construction today; never stress-tested against a side-effecting projection)
18. storage-full is handled safely — 🟡 (simulated via `InMemoryEventStore::with_capacity`; live disk-full handling remains dependent on OS/filesystem backend)
19. no external side effect occurs before durable commit — ✅ (confirmed for messaging's own send path — persist, record, network send, in that order; not exhaustively audited elsewhere)
20. crash/property/fuzz tests exist — 🟡 (crash-adjacent and property tests exist; no fuzz tests)
21. the subsystem works outside the messenger — 🟡 (the core `siar-event-log` crate has zero dependency on any domain crate, proven structurally; the only domain with real end-to-end usage is messaging)

Honest overall read: roughly a third of this checklist is genuinely done, a third is real-but-partial, and a third hasn't been started — consistent with the section tracker above being closer to "40% of the letter of the spec" than "40% of a production-ready subsystem," since the sections concentrated in ⬜ (remote ingestion, replication, snapshots, most test-harness sections) are disproportionately hard relative to their count.

### Phase 5 decision — deliberately deferred, not decided

Recorded rather than acted on, per the user's own explicit instruction: build the note, not the code, so this is ready to act on "when there's a better time or a development which requires writing the code."

**The question**: does §14's transactional-outbox/effects/retry machinery (Phase 5) extend `siar-storage`'s existing messaging outbox (`OutboxRepository`, already real, tested, already what `send_text`/`retry_due` use today), or build a second, event-log-native outbox alongside it?

**Why it's a real fork, not a default**:
- *Extend the existing one*: less duplication, one source of truth for "what needs retrying." But couples `siar-event-log`'s own retry story to a table `siar-storage` owns, and that outbox currently only knows about messaging — files/identity/DTN/emergency would need either their own outbox tables (repeating the per-domain pattern the event catalogs already follow) or a shared generic one that doesn't exist yet either way.
- *Build a second one on the event log*: matches the event-sourcing pattern Phases 1-4 have followed throughout, could plausibly become the ONE outbox every domain uses. But now two systems both think they own "pending work" for messaging specifically — a split-brain risk if they ever disagree about a message's state.

**What's already true, and doesn't disappear whichever way this goes**: `siar-storage`'s outbox works today, is tested, and `send_text`'s own resilience test already proves messaging's actual delivery doesn't depend on the event log at all — no urgency pressure to resolve this from a "something is broken" angle.

**What would make this decision easier later**: whichever domain gets a real `append` caller next will surface the actual shape of the problem — if files or identity end up needing their own retry/outbox logic too, that's real evidence for "build it once on the event log." Revisit the next time Phase 5, or a second domain's outbox need, actually comes up — not before.

## Round-by-round history

All rounds below verified against the actual uploaded `Cargo.lock` with rustc 1.91.1, with real `cargo test`/`cargo clippy -D warnings`/`cargo fmt --check`/`cargo doc` runs — not assumed to still hold from a prior round.

1. **`siar-event-log` — Phase 2 round 1 (2026-09-19)**: `stoolap_store.rs` — `StoolapEventStore`, a real `EventStore` backed by `stoolap` 0.4.0. §57 schema (`events`/`stream_heads`), §22 blake3 integrity checksum verified on every read, §11 atomic append via `stoolap` transactions + a process-local `Mutex`. Fixed two latent gaps: `EventId`/`CorrelationId` gained `as_uuid`/`from_uuid`; `StreamId` gained `from_bytes`. 17/17 tests.
2. **`siar-event-log`/`siar-messaging` — Phase 3 round 1 (2026-09-19)**: §33 Messaging Events — new `siar-messaging::events` module, `EventTypeId` 100-108, `MessagingEvent` enum (9 variants, each its own `V1` schema), `into_new_event`/`decode_messaging_event`. Identity's own catalog (`audit_log`, `EventTypeId` 1-8) turned out to already exist from an earlier session — found, left alone. 15/15 tests in the new module.
3. **`siar-blob-manifest` — Phase 3 round 2 (2026-09-19)**: §34 File Events — `events.rs`, `EventTypeId` 200-208, `FileEvent` enum lined up one-for-one with `TransferState`/`TransferEvent` without merging into it. New `TransferId` newtype added to `ids.rs` (a transfer is distinct from the blob it moves). `BlobVerified` modeled as an outcome, not a state. 6 new tests, 37/37 total. Phase 3's three original named domains (§33/§34/§35) done.
4. **`siar-event-log`/`siar-messaging` — closing the "no real caller" gap (2026-09-19)**: new `siar-event-log::retry` — `append_with_retry`, the optimistic-concurrency retry loop. A real bug caught by this module's own tests before shipping (wrong version reported in a returned `ConcurrencyConflict`). Wired into `MessageService` via a new `with_event_log` builder — three real call sites (`send_text`, `handle_incoming`'s new-message and ack branches), the first `append` callers anywhere in the workspace. 21/21 event-log tests; 26/26 messaging tests.
5. **`siar-event-log`/`siar-messaging` — Phase 4 (2026-09-19)**: `Projection` trait (`apply`/`reset`), `ProjectionCheckpoint`, `InMemoryCheckpointStore`, `ProjectionRunner::catch_up`. `conversation_summary` (`ConversationSummaryProjection`) is the first real `Projection` in the workspace, wired automatically into `MessageService::with_event_log`. 27/27 event-log tests; 31/31 messaging tests.
6. **`siar-messaging` — audit pass over spec §5-15 (2026-09-19)**: read the sections in order rather than by phase, per explicit user request. §5-7/§10-13/§15 confirmed already satisfied as a Phase 1/2 side effect. Two real gaps found and fixed: §8 Correlation/Causation (new `MessageCorrelation` registry — `into_new_event` now takes real `correlation_id`/`causation_id` instead of hardcoded `None`) and §9 Versioned Event Schemas (a real latent bug — `decode_messaging_event` ignored the stored `schema_version` entirely; fixed with an explicit version match and rejection of unrecognized versions). Also closed §14's outbox-resilience test (`send_text_survives_a_completely_broken_event_log`). 34/34 messaging tests.
7. **`siar-blob-manifest`/`siar-identity-multidevice` — closing the §9 gap everywhere (2026-09-19)**: the same schema-version fix applied to `decode_file_event` and `decode_audit_payload`. 38/38 blob-manifest tests; 257/257 identity-multidevice tests. §9 now consistently enforced across all three original domain catalogs.
8. **`siar-event-log` — durable checkpoint storage (2026-09-19)**: `stoolap_checkpoint_store.rs` — `StoolapCheckpointStore`, the durable counterpart to `InMemoryCheckpointStore`, built deliberately ahead of any caller that strictly needed it yet (checkpoint durability has to exist *before* the first durable projection, not after, or a restart would silently resume against stale state). 32/32 event-log tests.
9. **`siar-messaging` — a real durable `ConversationSummaryProjection` (2026-09-19)**: `stoolap_projections.rs` — `StoolapConversationSummaryProjection`, a genuine drop-in for the in-memory version via a new `ConversationSummaryQuery: Projection` supertrait and Rust's stable trait-object upcasting. A real correctness bug caught and fixed *before* shipping (a naive delete-then-insert would have blanked `last_message_id` on every non-creation event). 40/40 messaging tests (26 unit + 14 integration).
10. **`siar-dtn-bundle`/`siar-emergency` — §36/§37, the last two Phase 3 domains (2026-09-22)**: `siar-dtn-bundle::events` (`EventTypeId` 300-308, 9 events lining up with `BundleState`); `siar-emergency::events` (`EventTypeId` 400-405, 6 events; new `ReportId` added; `AlertTrust` gained `Serialize`/`Deserialize`). §92 Phase 3 now closed across all five named domains. 51/51 dtn-bundle tests; 17/17 emergency tests.
11. **`siar-event-log` — §55 Event Size Limits (2026-09-22)**: `validate_payload_size`/`DEFAULT_MAX_EVENT_PAYLOAD_BYTES` (256 KiB), enforced by both real backends before any state mutation; new `EventStoreError::PayloadTooLarge`. One uniform ceiling, named honestly as a deliberate first cut versus the section's fuller per-type-registry ask. 35/35 event-log tests; zero regressions confirmed across all six real dependents.
12. **`siar-event-registry` — §63 Namespaced Custom Events (2026-09-22)**: a brand-new crate (the first entirely new crate this session) — the only shape that can compare all five domains' `EventTypeId` ranges without creating a dependency cycle. `ALL_REGISTERED_EVENT_TYPES` (41 constants) + `find_collisions`, proven to actually fail on a deliberately-injected fake collision, not just pass vacuously. Manually maintained — named honestly as the real, conditional limit of this guarantee. 5/5 tests. Also fixed a separate found gap: `siar-emergency` had never been added to `[workspace.dependencies]`.

The §1-95 tracker, §93 self-audit, and Phase 5 decision above were authored as their own pass on 2026-09-22, after round 12, at the user's explicit request to have a durable, re-readable status map that doesn't require re-reading the whole codebase to reconstruct.

## Known gaps / open questions

- Phase 5 (outbox/effects/retry/recovery) — deliberately deferred; see the decision write-up above.
- No shared, automatically-enforced cross-crate `EventTypeId` registry — `siar-event-registry`'s roster is manually maintained.
- Neither DTN nor Emergency (nor files/identity) has a real `EventStore::append` caller driven by a live `apps/*` code path yet — each domain's own `*-service` crate constructs the wiring but isn't yet called from an application.
- No schema-version column on `stoolap`-backed SQL tables themselves (a lower-level versioning question than §9's payload-level one) — named since the first Phase 2 round, still open.
- No fuzz harness, no golden-byte-encoding tests, no full composed recovery-acceptance test (§85-88).

## Note

Fully transcribed from ROADMAP.md's Tier 0 narrative and the dedicated end-of-document tracker/audit/decision sections on 2026-09-27, superseding the prior 2026-09-01 stub (which read "10/95, Partial, earliest-stage"). This file, not ROADMAP.md, is now the authoritative detail record for spec 04 going forward.
