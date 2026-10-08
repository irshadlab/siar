//! Part 09 — Crash Recovery Architecture.
//!
//! Built the same way as this workspace's other Part-01–08 crates
//! (see [[resilient-mesh]] project memory): one real, deliberately-
//! scoped, tested slice per pass. This first pass covers §6-9 — the
//! four sections in this range concrete enough to implement for real
//! without a live database (this sandbox's rustc floor blocks a real
//! SQLite backend, the same constraint every other crate in this
//! series already documents).
//!
//! ## This pass — §106-120 (ephemeral state, repair, reporting, injection)
//!
//! - [`ephemeral_state`] — §106 only trusted long-lived peers survive
//!   (rescan always), §107 dynamic capabilities are re-queried, §108
//!   pressure is never persisted as authoritative; storage pressure is
//!   derived from disk numbers in overflow-safe integer math, with
//!   untrustworthy figures treated as critical.
//! - [`cleanup_quarantine`] — §109 `CleanupSequencer` enforces the
//!   six-phase order and `gc_allowed` stays false until every
//!   reference-establishing phase is done; §110 size-bounded
//!   `Quarantine` (oldest evicted first, oversize refused rather than
//!   deleted).
//! - [`repair_tools`] — §111 admin commands, §112 the seven doctor
//!   checks (a missing check makes the run incomplete, never healthy),
//!   §113 bounded + dry-run + backup-aware + logged repair gate, §114
//!   automatic-vs-admin authority per `RepairAction`.
//! - [`reporting`] — §115 `RecoveryReport` whose only payload types
//!   are enum codes and counts (no secret can be put in it), §116 UX
//!   messages with the spec wording and a block-only-if-necessary
//!   rule, §117 exact developer panel, §118 saturating metrics, §119
//!   per-field logging rules.
//! - [`crash_injection`] — §120 the nine `CrashPoint`s and a
//!   deterministic `CrashInjector` that is a zero-sized no-op unless
//!   the `crash-injection` feature (or `cfg(test)`) is on, plus the
//!   crash matrix: the send pipeline is crashed at every point and
//!   must apply exactly once iff the DB commit happened.
//!
//! ## Previous pass — §91-105
//!
//! - [`timing_recovery`] — §91 persisted `WallMillis` with skew
//!   tolerance (`age_millis`, three-valued `expiry_state`,
//!   `check_clock`) and an in-process `MonotonicDeadline` that is
//!   deliberately not `Serialize`; §92 `RetryState` eligibility after
//!   restart; §93 backoff that continues from the persisted attempt
//!   count and never pulls `next_attempt_at` earlier.
//! - [`recovery_scheduler`] — §94 `plan_resume`: bounded batches,
//!   resource admission, deterministic jitter, spread deferred waves
//!   (10 000 pending ops start 50 at a time); §95 the five
//!   `RecoveryClass` priorities; §96 SOS resumes first once identity
//!   and storage checks pass.
//! - [`reevaluation`] — §97 revocations before sends, §98 policy
//!   re-check (revoked > size > mobile-data), §99 capabilities only
//!   from the current session, §100 `DeliveryIntent` with no path
//!   field, §101 DTN→direct upgrade keeping `MessageId`/`BundleId`,
//!   §102 persisted `VerifiedChunks` bitset with path assignment
//!   rebuilt from current paths.
//! - [`ephemeral_recovery`] — §103 live calls become `Interrupted`
//!   (no resume outcome exists), §104 `CallInterrupted` history event,
//!   §105 presence/typing never persisted, stale entries discarded.
//!
//! ## Previous pass — §76-90
//!
//! - [`fault_policy`] — §76 scope→strategy (only worker-local
//!   violations restart a worker; shared-state or durable-write risk
//!   aborts the process) with `fail_fast`; §77 OOM counts as a crash
//!   with no cleanup; §78-80 one `disk_full_response` per stage:
//!   never claim acceptance or progress, transfer lands in
//!   `FailedStorage`, user action stays available; a chunk is
//!   complete only after a durable write.
//! - [`migration_recovery`] — §81 deterministic migration-crash
//!   decision (mark-complete / rerun / rollback-from-backup / manual
//!   repair, never a guess), §82 only the lock-holding daemon
//!   migrates, §83 backup-vs-checkpoint by size for destructive steps,
//!   §84 event content is immutable, §85 open-compatibility
//!   (current / migrate / too-new / too-old), §86
//!   `min_reader`/`min_writer` downgrade check (reject, read-only, or
//!   read-write).
//! - [`recovery_metadata`] — §87 `RecoveryMetadata`, §88 generation
//!   bumped per successful start (checked, never wraps), §89
//!   `OwnershipStamp` classification (current operation / stale orphan
//!   / unclaimed / from-the-future-never-delete).
//! - [`locks`] — §90 a real OS file lock (`File::try_lock`, released
//!   by the kernel on process death) plus `lock_is_stale`, which
//!   needs PID *and* process start time and never steals from an
//!   undeterminable owner.
//!
//! ## Previous pass — §61-75
//!
//! - [`effect_journal`] — §61 five-state journal with validated
//!   transitions; §62/§63 `InFlight` → `UnknownOutcome` after a crash
//!   (never `Failed`), `EffectRecoveryState` verbatim, evidence-only
//!   resolution (`Inconclusive` stays unknown); §64 retry-same-id as
//!   the preferred action, verify or wait otherwise.
//! - [`effects_io`] — §65 export recovery table (temp+atomic commit by
//!   default, or numbered visible duplicates, never overwrite) and
//!   `unique_numbered_name`; §66 notification dedup that is explicitly
//!   non-authoritative.
//! - [`platform_recovery`] — §67 `UiSuccess` requires a
//!   `DurableReceipt`; §68/§69 `remaining_work` /
//!   `select_for_opportunity` (platform callbacks are only execution
//!   opportunities, priority-ordered); §70 only the daemon may repair
//!   storage; §71 `IpcClient` reconnect → fresh snapshot → resubscribe
//!   with daemon-epoch checks so stale streams are rejected; §72
//!   headless outcome (never prompts, only key/corruption cases need
//!   an admin).
//! - [`supervision`] — §73 five supervised workers, §74 bounded
//!   exponential backoff with windowed give-up (degraded-stop, no
//!   restart storm), per-worker isolation; §75 `catch_boundary` for
//!   panic containment at FFI/service edges.
//!
//! ## Previous pass — §47-60
//!
//! - [`recovery_mode`] — §50 `RecoverySeverity` verbatim, plus a fixed
//!   capability set per severity and one [`recovery_mode::gate`] every
//!   operation goes through: ReadOnly allows exactly §47's four read
//!   capabilities, Fatal allows diagnostics only and never starts
//!   network (§49), Degraded runs fully and lists what is down (§48).
//!   Capabilities never widen as severity rises (tested).
//! - [`corruption`] — §51 domain→detection-mechanism map, §52
//!   scope→response table (six distinct outcomes, "do not treat all
//!   corruption the same"), §53 blob plan, §54 journal rebuild from
//!   verifiable chunks only, §55 `verify_contiguous` (a gap is an
//!   error, never skipped) and restore-source order, §56 secure-store
//!   failure = viewable but no trusted comms. Reconciles §49's "fatal"
//!   example with §56 in the doc comment of `response_for`.
//! - [`backup_restore`] — §57 authenticated/versioned/integrity-
//!   checked backup validation, §58 `RestoreGate` that cannot say
//!   "ready" until expiry, delivery, revocation and peer state are all
//!   re-evaluated, and a restored-operation decision where revocation
//!   outranks everything.
//! - [`effect_identity`] — §59 `Effect::retry` keeps the id (no API
//!   mints a new one), §60 `IdempotentReceiver` over all five listed
//!   kinds, reusing the existing `Deduplicator`.
//!
//! ## Previous pass — §34-46
//!
//! - [`session_recovery`] — §34 [`session_recovery::SessionHandle`]
//!   carries its runtime generation, so every pre-crash session is
//!   invalid by construction (and deliberately not `Serialize`); §35
//!   resumption-ticket gate (expiry, revocation, account-generation
//!   and secure-storage binding) that defaults to *Disabled*, with the
//!   secret redacted from `Debug`; §36 `CapabilityHint` that can be
//!   displayed but never promoted to a negotiated contract.
//! - [`route_resource_recovery`] — §37 persisted route hints can only
//!   produce `UnvalidatedCandidate`s; the sole way to a
//!   `ValidatedCandidate` is a live check. §38 restart treatment per
//!   resource class. §39 reservation reconciliation (restore / hold /
//!   defer-to-orphan-cleanup / release) with byte totals.
//! - [`derived_recovery`] — §40 outbox↔event-state reconciliation
//!   (unknown outbox rows are flagged, never deleted), §41 projection
//!   checkpoint/replay/rebuild plan, §42 corruption response (derived →
//!   rebuild, authoritative → halt into `ManualRepairRequired`), §43
//!   search-index lifecycle, §44 recent-first chat-list rebuild order.
//! - [`progressive`] — §45 phases and §46 priority tiers as a `Copy`
//!   bitmask machine: usable once critical tiers are done, secondary
//!   work refused until then.
//!
//! ## Previous pass — §22-33
//!
//! - [`atomic_rename`] — §22: [`atomic_rename::rename_viability`] makes
//!   "never assume atomicity across filesystems" a check that must pass
//!   (unknown filesystem identity is *refused*, not assumed);
//!   [`atomic_rename::publish_atomic`] is the real rename + parent-dir
//!   fsync.
//! - [`temp_scan`] — §23 opaque temp ids + five-way classification,
//!   §24 cleanup policy built from the three levers the spec names
//!   (ownership metadata, age threshold, runtime generation). Corrupt
//!   files are quarantined, never silently deleted; unknown-owner files
//!   survive until hard expiry.
//! - [`blob_refs`] — §25 rebuild/audit/repair of derived reference
//!   counts (drift classified, undercount flagged dangerous) and §26
//!   staged-GC recovery decided from *observed facts*, not from the
//!   persisted stage (a stale "rechecked" is never trusted).
//! - [`dtn_recovery`] — §27 restore plan (expire, restore budgets
//!   without resetting them, honour live tombstones, scheduler last),
//!   §28 relay dedup-and-re-ACK, §29 owed-until-confirmed destination
//!   ACKs with idempotent origin-side apply, §30 bounded forward
//!   history that can only ever make forwarding *more* permissive when
//!   data is missing. Uses the real `siar_dtn_bundle::BundleId`.
//! - [`identity_recovery`] — §31 five-check verdict that fails closed
//!   (unverifiable = failed), §32 fork detection with no auto-merge
//!   path (only a user-confirmed `ExplicitReconciliation`), §33
//!   post-commit revocation recovery plus the one honestly-surfaced
//!   case where a pre-commit crash needs re-approval.
//!
//! ## Earlier passes
//!
//! ### File transfer recovery (§18-21)
//!
//! - [`chunk_recovery`] — §18 "File Transfer Recovery", §19 "Chunk
//!   Commit Ordering", §20 "Chunk State Ambiguity":
//!   [`chunk_recovery::ChunkRecoveryState`] (the durable subset of
//!   §19's commit pipeline) with real transition validation, and
//!   [`chunk_recovery::reconcile_chunk_on_restart`]/
//!   [`chunk_recovery::ChunkRecoveryMap`] implementing §20's exact
//!   rule ("treat chunk as unverified, reverify on restart. Do not
//!   assume") as executable code — a genuine complement to
//!   `siar_blob_manifest::resume::ResumeBitmap` (that type's binary
//!   received/missing bitset has no way to represent §20's actual
//!   hazard; see this module's own doc comment for the full
//!   reasoning), not a competing replacement for it.
//! - [`finalization_recovery`] — §21 "Finalization Recovery":
//!   [`finalization_recovery::FinalizationState`] (§21's own
//!   pipeline) with real transition validation, and
//!   [`finalization_recovery::reconcile_finalization`] — §21 names
//!   three crash scenarios in prose; this function is the exhaustive
//!   5-case decision table those three scenarios are examples of,
//!   tested against each named scenario individually rather than
//!   only the cases the spec happened to call out.
//!
//! ### Messaging recovery (§14-17)
//!
//! - [`messaging_recovery`] — §14 "Messaging Recovery", §15
//!   "Ambiguous Send Result", §16 "Delivery Receipt Recovery", §17
//!   "Inbox Recovery": [`messaging_recovery::Deduplicator`], the one
//!   real mechanism behind three sections that each separately
//!   restate "recipient deduplicates"/"sender applies idempotently"
//!   under different names, built once and reused for all three
//!   rather than three drift-prone hand-rolled `HashSet` checks;
//!   [`messaging_recovery::reload_outbox`] implementing §14's restart
//!   sequence with `OutboundRecoveryAction::RetrySend` structurally
//!   incapable of carrying a freshly-generated id (§14: "do not
//!   create new MessageId"); [`messaging_recovery::apply_receipt`]
//!   for §16's idempotent receipt application; and
//!   [`messaging_recovery::InboundReceiveState`]/
//!   [`messaging_recovery::receive_inbound`] for §17's
//!   validate→persist→commit→ACK sequence and its
//!   crash-before-ACK dedup. Uses this workspace's real
//!   `siar_domain::MessageId` and `siar_event_log::ids::EventId`
//!   rather than local stand-in types, since both concepts §14-17
//!   name already exist as concrete types elsewhere in this
//!   workspace.
//!
//! ### §11-13
//!
//! - [`staged_intent`] — §11 "WAL Is Not Enough", §12 "Cross-Store
//!   Atomicity": [`staged_intent::StagedOperationState`] (§12's
//!   three-stage persisted pipeline) with real transition validation,
//!   and [`staged_intent::reconcile`] — §12's own "recovery checks
//!   intermediate state" turned into an actual decision: the full
//!   3×2 truth table of (persisted state) × (what's actually true on
//!   disk/network), including §11's exact named scenario ("database
//!   row says file exists but file rename failed") as one of the
//!   tested cases rather than only the two cases that already agree.
//! - [`operation_state`] — §13 "Durable Operation State":
//!   [`operation_state::FileOperationState`] (§13's own worked
//!   example, verbatim) with real transition validation, and
//!   [`operation_state::is_definitely_complete`] — §13's own explicit
//!   warning ("never rely on absence of error to infer completion")
//!   made into one callable function instead of prose a caller could
//!   still get wrong at each call site: a missing record is always
//!   treated as incomplete, never as implied success.
//!
//! ### §6-9
//!
//! - [`shutdown_marker`] — §6 "Clean Shutdown Marker":
//!   [`shutdown_marker::ShutdownMarker`]/[`shutdown_marker::ShutdownMarkerStore`]
//!   (trait + an in-memory stand-in, same seam/stand-in split every
//!   other `*Store` trait in this workspace already uses),
//!   [`shutdown_marker::classify_startup`] (§6's "last start had no
//!   clean close → unclean startup path"), and
//!   [`shutdown_marker::recommended_scope`], built so §6's own
//!   caution — "do not depend on this marker for correctness, only
//!   for deciding how much recovery work to run" — is structural:
//!   both possible scopes still run every mandatory step, the
//!   classification only adds optional extra verification, never
//!   removes required work.
//! - [`idempotent_steps`] — §7 "Crash Recovery Must Be Idempotent":
//!   [`idempotent_steps::run_recovery`], directly implementing §7's
//!   own test case ("run recovery, crash halfway, run again — must
//!   be safe") via a completed-steps ledger checked before each step,
//!   tested by literally simulating a crash mid-pipeline and
//!   confirming a second run resumes rather than re-executing.
//! - [`state_machine`] — §8 "Recovery State Machine":
//!   [`state_machine::RecoveryState`] (verbatim happy-path chain and
//!   failure-branch list) and [`state_machine::RecoveryStateMachine`]
//!   with real transition validation — the failure-branch wiring
//!   (which stage can reach which failure) is this module's own
//!   reasoned mapping, since §8 lists the two sets flatly without
//!   specifying the edges; documented per-edge in that module.
//! - [`transaction_group`] — §9 "Storage Transaction Boundaries":
//!   [`transaction_group::TransactionGroup`], a real in-memory
//!   all-or-nothing operation batch, standing in for an actual
//!   database transaction (unavailable in this sandbox) while still
//!   genuinely testing the all-or-nothing property §9 requires,
//!   modeled directly on §9's own three-part worked example (event
//!   append + projection update + outbox enqueue).
//!
//! ## Deliberately not attempted
//!
//! §10 "WAL" has no code here on purpose — its own text is an
//! explicit instruction *not* to build something ("Do not manually
//! implement log semantics already provided by database engine unless
//! needed"), not a design this crate could implement. There is nothing
//! to build faithfully to that section beyond noting the decision,
//! which this doc comment now does. Everything past §120 (failpoint framework §121 onward, fuzzing,
//! simulated-disk and property tests, and the definition-of-done
//! sections — the rest of this 186-section spec) is not yet attempted.

pub mod atomic_rename;
pub mod backup_restore;
pub mod blob_refs;
pub mod chunk_recovery;
pub mod cleanup_quarantine;
pub mod corruption;
pub mod crash_injection;
pub mod derived_recovery;
pub mod dtn_recovery;
pub mod effect_identity;
pub mod effect_journal;
pub mod effects_io;
pub mod ephemeral_recovery;
pub mod ephemeral_state;
pub mod fault_policy;
pub mod finalization_recovery;
pub mod idempotent_steps;
pub mod identity_recovery;
pub mod locks;
pub mod messaging_recovery;
pub mod migration_recovery;
pub mod operation_state;
pub mod platform_recovery;
pub mod progressive;
pub mod recovery_metadata;
pub mod recovery_mode;
pub mod recovery_scheduler;
pub mod reevaluation;
pub mod repair_tools;
pub mod reporting;
pub mod route_resource_recovery;
pub mod session_recovery;
pub mod shutdown_marker;
pub mod staged_intent;
pub mod state_machine;
pub mod supervision;
pub mod temp_scan;
pub mod timing_recovery;
pub mod transaction_group;

pub use atomic_rename::{
    filesystem_id_of, publish_atomic, rename_viability, FilesystemId, PublishError, RenameViability,
};
pub use backup_restore::{
    reevaluate_restored, validate_backup, BackupHeader, BackupRejection, Reevaluation, RestoreGate,
    RestoredDecision, RestoredOperation,
};
pub use blob_refs::{
    audit_ref_counts, authoritative_count, rebuild_ref_counts, reconcile_gc, repair_ref_counts,
    DriftKind, GcFacts, GcRecoveryAction, GcStage, RefCountDrift, RefCounts,
};
pub use chunk_recovery::{
    reconcile_chunk_on_restart, ChunkRecoveryAction, ChunkRecoveryMap, ChunkRecoveryState,
    TransferId,
};
pub use cleanup_quarantine::{
    CleanupPhase, CleanupSequencer, OutOfOrder, Quarantine, QuarantineEntry, QuarantineOutcome,
};
pub use corruption::{
    mechanism_for, plan_blob_corruption, rebuild_journal_progress, response_for, restore_sources,
    verify_contiguous, BlobCorruptionPlan, BlobTrust, ChunkVerification, CorruptionAction,
    CorruptionScope, DataDomain, DetectionMechanism, JournalRebuild, RestoreSource, SequenceGap,
};
pub use crash_injection::{CrashInjector, CrashPoint, InjectedCrash};
pub use derived_recovery::{
    chat_list_rebuild_order, plan_projection, reconcile_outbox, reconcile_outbox_entry,
    respond_to_corruption, CorruptionResponse, OutboxAction, ProjectionCheckpoint, ProjectionPlan,
    RebuildReason, SearchIndexRecovery, SearchIndexState, SemanticMessageState, StoreClass,
    StreamActivity,
};
pub use dtn_recovery::{
    plan_dtn_restore, restore_budget, AckApply, AckStatus, BudgetRestore, DestinationAckLedger,
    DestinationReceive, DtnRestorePlan, DtnRestoreStep, ForwardHistory, ForwardRecord,
    OriginAckTracker, RelayLedger, RelayReceiveOutcome, StoredBundle, Tombstone,
};
pub use effect_identity::{
    DeviceEventId, Effect, EffectId, IdempotentReceiver, ReceiveKey, ReceiveOutcome,
};
pub use effect_journal::{
    classify_after_crash, decide_unknown, resolve_unknown, retry_is_safe, EffectJournal,
    EffectRecoveryState, EffectState, JournalError, RemoteEvidence, UnknownOutcomeAction,
};
pub use effects_io::{
    plan_export_recovery, unique_numbered_name, ExportFacts, ExportPolicy, ExportRecoveryAction,
    NotificationDeduper, NotificationKey, NotificationOutcome,
};
pub use ephemeral_recovery::{
    history_event_for_recovery, is_persistable, may_resume_media_session, recover_call,
    recover_presence, CallHistoryEvent, CallId, CallRecovery, CallState, EphemeralKind,
    PresenceRecovery,
};
pub use ephemeral_state::{
    persisted_as_authoritative, pressure_source, recover_discovery, recover_dynamic_capability,
    storage_pressure, CapabilityRecovery, DiscoveredEntry, DiscoveryRecovery, DynamicCapability,
    PressureKind, PressureLevel, PressureSource, StorageThresholds,
};
pub use fault_policy::{
    chunk_may_be_marked_complete, cleanup_ran, disk_full_response, fail_fast, recovery_required,
    strategy_for, ChunkWriteResult, DiskFullOutcome, DiskFullStage, ExitKind, PanicStrategy,
    SpaceAction, TransferStorageState, ViolationScope,
};
pub use finalization_recovery::{
    reconcile_finalization, FinalizationRecoveryAction, FinalizationState,
};
pub use idempotent_steps::{
    run_recovery, RecoveryLedger, RecoveryStep, RecoveryStepError, RecoveryStepId,
};
pub use identity_recovery::{
    detect_identity_forks, recover_revocation, sessions_to_invalidate, verify_identity,
    AccountStateSummary, CheckOutcome, ExplicitReconciliation, IdentityEvidence, IdentityFailure,
    IdentityForkDetected, IdentityVerdict, ReconciliationError, RevocationFacts,
    RevocationRecovery, RevocationStage,
};
pub use locks::{lock_is_stale, LockError, LockOwner, ProcessLock, ProcessProbe};
pub use messaging_recovery::{
    apply_receipt, reload_outbox, Deduplicator, DiscardReason, InboundReceiveOutcome,
    InboundReceiveState, OutboundMessageState, OutboundRecoveryAction, PendingOutboundMessage,
    ReceiptApplyOutcome,
};
pub use migration_recovery::{
    check_downgrade, check_open_compatibility, db_migration_may_change, may_migrate,
    plan_migration_recovery, pre_migration_backup, BackupPolicy, DbAccess, DowngradeRejected,
    MigrationDenied, MigrationFacts, MigrationRecoveryAction, MigrationState, OpenCompatibility,
    PreMigrationBackup, SchemaChange, VersionRequirements,
};
pub use operation_state::{is_definitely_complete, FileOperationState};
pub use platform_recovery::{
    headless_outcome, may_repair_storage, remaining_work, select_for_opportunity, DurableReceipt,
    HeadlessHealth, HeadlessOutcome, IpcClient, IpcClientState, IpcError, ProcessRole, UiSuccess,
    WorkItem,
};
pub use progressive::{ProgressError, ProgressiveRecovery, RecoveryPhase, RecoveryTier};
pub use recovery_metadata::{
    classify_ownership, GenerationExhausted, OwnerStatus, OwnershipStamp, RecoveryMetadata,
};
pub use recovery_mode::{
    capabilities, gate, terminal_state_for, Capabilities, DegradedSubsystem, Operation,
    OperationDenied, RecoverySeverity, SeverityAccumulator,
};
pub use recovery_scheduler::{
    jitter_millis, plan_resume, sos_decision, AdmissionConfig, PendingOp, RecoveryClass,
    ResumePlan, SafetyChecks, SosDecision, SosState,
};
pub use reevaluation::{
    assign_missing_to_paths, capability_decision, choose_path_after_restart, queued_to_reevaluate,
    reevaluate_policy, sends_may_resume, CapabilityDecision, CurrentPolicy, DeliveryIntent,
    PathKind, PathPlan, PendingWorkFacts, PolicyCancel, PolicyDecision, SendResumeStage,
    VerifiedChunks,
};
pub use repair_tools::{
    authorize_repair, may_run_automatically, plan_repair, repair_authority, AdminCommand,
    CheckStatus, DoctorCheck, DoctorFinding, DoctorReport, RepairAction, RepairAuthority,
    RepairLog, RepairLogEntry, RepairMode, RepairRefusal, RepairRequest,
};
pub use reporting::{
    log_rule, may_log, should_block_ui, ux_message, DeveloperDiagnostics, LogField, LogRule,
    MetricEvent, RecoveryAction, RecoveryActionKind, RecoveryMetrics, RecoveryReport,
    RecoveryWarning, UxMessage, WarningCode,
};
pub use route_resource_recovery::{
    bootstrap_candidates, reconcile_reservation, reconcile_reservations, restart_treatment,
    ReservationAction, ReservationId, ReservationKind, ReservationPlan, ResourceClass,
    RestartTreatment, RouteHints, StorageReservation, UnvalidatedCandidate, ValidatedCandidate,
};
pub use session_recovery::{
    validate_ticket, CapabilityHint, ResumptionContext, ResumptionPolicy, ResumptionTicket,
    SessionHandle, TicketRejection,
};
pub use shutdown_marker::{
    classify_startup, recommended_scope, InMemoryShutdownMarkerStore, RecoveryScope,
    RuntimeGeneration, ShutdownMarker, ShutdownMarkerStore, StartupKind,
};
pub use staged_intent::{reconcile, ReconciliationAction, StagedOperationState};
pub use state_machine::{InvalidTransition, RecoveryState, RecoveryStateMachine};
pub use supervision::{
    catch_boundary, PanicReport, RestartDecision, RestartPolicy, RestartTracker, Supervisor, Worker,
};
pub use temp_scan::{
    classify_temp_file, cleanup_decision, plan_cleanup, CleanupDecision, CleanupPolicy, KeepReason,
    OpaqueTempId, ScanContext, TempArea, TempClass, TempFileEntry, TempOwnership,
};
pub use timing_recovery::{
    age_millis, backoff_delay, check_clock, coarse_level, eligibility, expiry_state,
    first_delay_after_restart, next_attempt_after_restart, persistable_wall_deadline, AgeEstimate,
    BackoffPolicy, ClockSanity, Eligibility, ExpiryState, MonotonicDeadline, RetryState,
    WallMillis,
};
pub use transaction_group::{TransactionGroup, TransactionStep};
