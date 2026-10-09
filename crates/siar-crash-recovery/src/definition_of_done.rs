//! §184 "Definition of Done" and §186 "Final Principle".
//!
//! An *executable* self-audit, in the same spirit as the Definition-
//! of-Done modules of specs 02, 03 and 08: every one of §184's twenty
//! criteria has a check that exercises this crate's real public API
//! and reports `Done`, `PartiallyDone` or `NotDone` — a status is
//! never written by hand, it is whatever the check finds, downgraded
//! to `PartiallyDone` where the criterion is bigger than this crate
//! can prove alone (and the note says exactly why).
//!
//! The biggest such gap is stated once, in [`Wiring`]: this crate is
//! the *decision logic* of recovery, verified in isolation. Whether
//! any runtime crate actually calls it is a separate, measurable fact.

use crate::backup_restore::{reevaluate_restored, RestoredDecision, RestoredOperation};
use crate::chunk_recovery::TransferId;
use crate::corruption::{
    rebuild_journal_progress, response_for, ChunkVerification, CorruptionScope,
};
use crate::derived_recovery::{
    plan_projection, reconcile_outbox, respond_to_corruption, CorruptionResponse, OutboxAction,
    ProjectionCheckpoint, ProjectionPlan, RebuildReason, SemanticMessageState, StoreClass,
};
use crate::dtn_recovery::{
    plan_dtn_restore, AckApply, DestinationAckLedger, DestinationReceive, OriginAckTracker,
    RelayLedger, RelayReceiveOutcome, StoredBundle,
};
use crate::effect_identity::{Effect, EffectId, IdempotentReceiver, ReceiveKey, ReceiveOutcome};
use crate::effect_journal::{decide_unknown, UnknownOutcomeAction};
use crate::fault_sim::{publish_correct, write_all, SimFs};
use crate::finalization_recovery::{
    reconcile_finalization, FinalizationRecoveryAction, FinalizationState,
};
use crate::identity_recovery::{recover_revocation, RevocationFacts};
use crate::local_first::{open_state, tier_requires_network, OpenState};
use crate::progressive::RecoveryTier;
use crate::recovery_mode::{gate, Operation, RecoverySeverity};
use crate::reevaluation::{capability_decision, CapabilityDecision, VerifiedChunks};
use crate::route_resource_recovery::{
    bootstrap_candidates, reconcile_reservation, ReservationAction, ReservationId, ReservationKind,
    RouteHints, StorageReservation,
};
use crate::session_recovery::SessionHandle;
use crate::shutdown_marker::RuntimeGeneration;
use crate::storage_catalog::{store_class, DataSet};
use crate::temp_scan::{
    classify_temp_file, cleanup_decision, CleanupDecision, CleanupPolicy, OpaqueTempId,
    ScanContext, TempArea, TempClass, TempFileEntry, TempOwnership,
};
use crate::transaction_group::TransactionGroup;
use siar_domain::MessageId;
use siar_dtn_bundle::BundleId;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditStatus {
    Done,
    PartiallyDone,
    NotDone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditItem {
    pub number: u8,
    /// §184's own wording.
    pub criterion: &'static str,
    pub status: AuditStatus,
    /// Empty when `Done`; otherwise what is missing.
    pub note: &'static str,
}

struct Criterion {
    text: &'static str,
    check: fn() -> bool,
    /// If set, the check passing means `PartiallyDone` with this note.
    limit: Option<&'static str>,
}

fn id() -> MessageId {
    MessageId::new()
}

// ---- the checks ----------------------------------------------------

/// Cuts "power" after every step of a send pipeline
/// (commit → network send → result marker) and after every step of the
/// atomic publish, then recovers.
fn kill_anywhere_without_logical_corruption() -> bool {
    // pipeline: cut after 0..=3 steps
    for cut in 0..=3 {
        let m = id();
        let (mut committed, mut marker, mut applied) = (false, false, 0u32);
        let mut remote = IdempotentReceiver::new();
        let deliver = |remote: &mut IdempotentReceiver, applied: &mut u32| {
            if remote.receive(ReceiveKey::Message(m)) == ReceiveOutcome::Process {
                *applied += 1;
            }
        };
        if cut >= 1 {
            committed = true;
        }
        if cut >= 2 {
            deliver(&mut remote, &mut applied);
        }
        if cut >= 3 {
            marker = true;
        }
        // recovery (run twice: it must be idempotent too)
        for _ in 0..2 {
            if committed && !marker {
                deliver(&mut remote, &mut applied);
                marker = true;
            }
        }
        let ok = if committed {
            applied == 1 && marker
        } else {
            applied == 0 && !marker
        };
        if !ok {
            return false;
        }
    }
    // atomic publish: every power-cut point leaves old-or-new
    for cut in 0..=5usize {
        for torn in [0usize, 3, 100] {
            let mut fs = SimFs::new();
            if publish_correct(&mut fs, "o", "dest", b"OLD").is_err() {
                return false;
            }
            let data = b"0123456789";
            let steps: [&dyn Fn(&mut SimFs); 5] = [
                &|f| f.create("n").unwrap(),
                &|f| write_all(f, "n", data).unwrap(),
                &|f| f.fsync("n").unwrap(),
                &|f| f.rename("n", "dest").unwrap(),
                &|f| f.fsync_dir(),
            ];
            for s in steps.iter().take(cut) {
                s(&mut fs);
            }
            fs.power_loss(torn);
            match fs.read("dest") {
                Some(v) if v == b"OLD" || v == data => {}
                _ => return false,
            }
        }
    }
    true
}

fn committed_intent_survives() -> bool {
    // Event log says Queued, outbox row lost in the crash → rebuilt.
    let m = id();
    let events: HashMap<_, _> = [(m, SemanticMessageState::Queued)].into();
    let plan = reconcile_outbox(&events, &HashSet::new());
    plan == vec![(m, OutboxAction::Reconstruct)]
}

fn uncommitted_intent_never_appears_committed() -> bool {
    // A transaction group that fails midway leaves the target
    // untouched: no half-committed intent.
    let mut state = (0u32, 0u32); // (events, outbox)
    let mut g: TransactionGroup<(u32, u32)> = TransactionGroup::new();
    g.add_step(|s| {
        s.0 += 1;
        Ok(())
    });
    g.add_step(|_| Err("crash before outbox write".into()));
    let failed = g.apply(&mut state).is_err();
    // and a revocation that never committed is reported as lost, not
    // silently assumed applied
    let r = recover_revocation(RevocationFacts {
        committed: false,
        sessions_invalid: false,
        propagated: false,
    });
    failed && state == (0, 0) && r.reapproval_required
}

fn ambiguous_send_retries_with_the_same_id() -> bool {
    let e = Effect::first_attempt(EffectId::Message(id()));
    let r = e.retry().retry();
    r.id() == e.id()
        && r.attempt() == 3
        && decide_unknown(true, false) == UnknownOutcomeAction::RetrySameId
}

fn receiver_deduplicates_retries() -> bool {
    let mut r = IdempotentReceiver::new();
    let k = ReceiveKey::Message(id());
    r.receive(k) == ReceiveOutcome::Process
        && r.receive(k) == ReceiveOutcome::Duplicate
        && r.receive(k) == ReceiveOutcome::Duplicate
}

fn transfer_progress_survives_process_death() -> bool {
    // Progress is a persisted bitset of *verified* chunks; a copy
    // stands in for "written to disk and read back".
    let mut v = VerifiedChunks::new();
    for c in [0, 1, 2, 7] {
        v.mark_verified(c);
    }
    let reloaded = v.clone();
    reloaded == v
        && reloaded.verified_indexes() == vec![0, 1, 2, 7]
        && reloaded.missing(4) == vec![3]
}

fn incomplete_chunks_are_reverified() -> bool {
    use ChunkVerification::*;
    let r = rebuild_journal_progress(&[Verified, Mismatch, Missing, Verified]);
    r.verified == vec![0, 3] && r.to_fetch == vec![1, 2]
}

fn blob_finalization_recovers_around_rename() -> bool {
    [false, true].into_iter().all(|object_exists| {
        reconcile_finalization(FinalizationState::Finalizing, object_exists)
            == FinalizationRecoveryAction::ResumeFinalization
    })
}

fn dtn_relay_bundles_survive_a_crash() -> bool {
    let b = BundleId::new();
    // relay stored it durably before the crash
    let mut relay = RelayLedger::from_durable([b]);
    let dup = relay.receive(b) == RelayReceiveOutcome::DuplicateReAck && relay.len() == 1;
    let plan = plan_dtn_restore(
        &[StoredBundle {
            id: b,
            expires_at_millis: 1_000,
            remaining_budget: Some(2),
            initial_budget: 4,
        }],
        &[],
        10,
    );
    dup && plan.keep.len() == 1 && plan.keep[0].0 == b
}

fn ack_ambiguity_is_idempotent() -> bool {
    let b = BundleId::new();
    let mut origin = OriginAckTracker::default();
    origin.expect(b);
    let a =
        origin.apply_ack(b) == AckApply::Applied && origin.apply_ack(b) == AckApply::AlreadyApplied;
    let mut dest = DestinationAckLedger::default();
    let d = dest.receive_payload(b) == DestinationReceive::Deliver
        && dest.receive_payload(b) == DestinationReceive::AlreadyCommittedResendAck;
    a && d
}

fn revocation_survives_crash_immediately() -> bool {
    let r = recover_revocation(RevocationFacts {
        committed: true,
        sessions_invalid: false,
        propagated: false,
    });
    r.invalidate_sessions && r.propagate && !r.reapproval_required
}

fn stale_sessions_capabilities_routes_are_invalidated() -> bool {
    let old = SessionHandle::new(RuntimeGeneration(1), 7);
    let sessions = !old.is_valid_in(RuntimeGeneration(2));
    let caps = capability_decision(false) == CapabilityDecision::StayQueuedUntilRenegotiated;
    let hints = RouteHints {
        last_successful_transport: None,
        endpoint_hints: vec!["x".into()],
    };
    let routes = bootstrap_candidates(&hints)
        .into_iter()
        .all(|c| c.validated(false).is_none());
    sessions && caps && routes
}

fn reservations_and_temp_files_are_reconciled() -> bool {
    let active: HashSet<TransferId> = HashSet::new();
    let res = StorageReservation {
        id: ReservationId::new(),
        kind: ReservationKind::Storage,
        owner: None,
        bytes: 10,
        created_at_millis: 0,
        staged_bytes_present: false,
    };
    let released =
        reconcile_reservation(&res, &active, 10_000, 1_000) == ReservationAction::Release;

    let policy = CleanupPolicy::new(1, 1_000).unwrap();
    let ctx = ScanContext {
        now_millis: 500,
        current_generation: RuntimeGeneration(5),
        active_transfers: &active,
        policy,
    };
    let entry = TempFileEntry {
        area: TempArea::Staging,
        id: Some(OpaqueTempId::new()),
        ownership: Some(TempOwnership {
            transfer: TransferId::new(),
            generation: RuntimeGeneration(2),
        }),
        created_at_millis: 0,
        corrupt: false,
    };
    let class = classify_temp_file(&entry, &ctx);
    released
        && class == TempClass::Orphan
        && cleanup_decision(&entry, class) == CleanupDecision::Delete
}

fn projections_resume_or_rebuild() -> bool {
    let cp = |v, s| {
        Some(ProjectionCheckpoint {
            schema_version: v,
            last_applied_seq: s,
        })
    };
    plan_projection(cp(1, 5), 1, 9)
        == ProjectionPlan::Replay {
            from_exclusive: 5,
            to_inclusive: 9,
        }
        && plan_projection(cp(1, 5), 2, 9) == ProjectionPlan::Rebuild(RebuildReason::SchemaChanged)
        && plan_projection(None, 1, 9) == ProjectionPlan::Rebuild(RebuildReason::NoCheckpoint)
}

fn derived_indexes_can_fail_without_losing_authoritative_data() -> bool {
    let derived = [
        DataSet::SearchIndex,
        DataSet::Projections,
        DataSet::Outbox,
        DataSet::ThumbnailCache,
    ];
    let authoritative = [
        DataSet::EventLog,
        DataSet::IdentityState,
        DataSet::UserBlobs,
    ];
    derived
        .iter()
        .all(|d| respond_to_corruption(store_class(*d)) == CorruptionResponse::DropAndRebuild)
        && authoritative
            .iter()
            .all(|d| store_class(*d) == StoreClass::Authoritative)
}

fn read_only_mode_exists() -> bool {
    gate(RecoverySeverity::ReadOnly, Operation::ViewMessages).is_ok()
        && gate(RecoverySeverity::ReadOnly, Operation::DurableWrite).is_err()
        && gate(RecoverySeverity::ReadOnly, Operation::StartNetwork).is_err()
}

fn severe_corruption_fails_closed() -> bool {
    let db = response_for(CorruptionScope::EntireDb);
    let stream = response_for(CorruptionScope::OneEventStream);
    db.severity == RecoverySeverity::Fatal
        && !db.trusted_comms_allowed
        && !stream.trusted_comms_allowed
        && gate(RecoverySeverity::Fatal, Operation::StartNetwork).is_err()
}

fn recovery_needs_no_internet() -> bool {
    RecoveryTier::ALL.iter().all(|t| !tier_requires_network(*t))
        && open_state(true, RecoverySeverity::Repaired, false) == OpenState::OpenOffline
}

fn recovery_itself_is_idempotent() -> bool {
    // Running reconciliation to convergence leaves nothing to do, and
    // identical durable facts give identical decisions.
    let (a, b) = (id(), id());
    let events: HashMap<_, _> = [
        (a, SemanticMessageState::Queued),
        (b, SemanticMessageState::Delivered),
    ]
    .into();
    let mut outbox: HashSet<MessageId> = [b].into();
    for (m, act) in reconcile_outbox(&events, &outbox) {
        match act {
            OutboxAction::Reconstruct => {
                outbox.insert(m);
            }
            OutboxAction::Remove => {
                outbox.remove(&m);
            }
            _ => {}
        }
    }
    let converged = reconcile_outbox(&events, &outbox).is_empty();
    let f = RevocationFacts {
        committed: true,
        sessions_invalid: true,
        propagated: true,
    };
    let steady = recover_revocation(f) == recover_revocation(f)
        && !recover_revocation(f).invalidate_sessions;
    let op = RestoredOperation {
        expires_at_millis: 5,
        already_delivered: false,
        peer_revoked: false,
        peer_unknown: false,
    };
    converged
        && steady
        && reevaluate_restored(&op, 1) == reevaluate_restored(&op, 1)
        && reevaluate_restored(&op, 1) == RestoredDecision::Reschedule
}

fn process_kill_fault_injection_and_property_tests_exist() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    root.join("tests/process_kill.rs").is_file()
        && root.join("src/fault_sim.rs").is_file()
        && root.join("src/fault_scenarios.rs").is_file()
        && root.join("src/failpoint.rs").is_file()
        && root.join("src/invariants.rs").is_file()
}

const CRITERIA: [Criterion; 20] = [
    Criterion {
        text: "process can be killed at any documented failpoint without logical corruption",
        check: kill_anywhere_without_logical_corruption,
        limit: Some("real SIGKILL is exercised on the atomic-publish path only; the message, DTN and identity pipelines are crashed in simulation (fault_sim, crash_injection), not in a killed process"),
    },
    Criterion {
        text: "committed message intent survives restart",
        check: committed_intent_survives,
        limit: Some("outbox reconstruction from the event state is proven here; the durable event store itself is siar-event-log's"),
    },
    Criterion {
        text: "uncommitted message intent does not appear as committed",
        check: uncommitted_intent_never_appears_committed,
        limit: None,
    },
    Criterion {
        text: "ambiguous network send retries with same stable ID",
        check: ambiguous_send_retries_with_the_same_id,
        limit: None,
    },
    Criterion {
        text: "receiver deduplicates retries",
        check: receiver_deduplicates_retries,
        limit: None,
    },
    Criterion {
        text: "transfer progress survives process death",
        check: transfer_progress_survives_process_death,
        limit: Some("the progress representation and its reload are verified; writing it to the real transfer journal is the file-transfer service's"),
    },
    Criterion {
        text: "incomplete chunks are reverified rather than trusted blindly",
        check: incomplete_chunks_are_reverified,
        limit: None,
    },
    Criterion {
        text: "blob finalization recovers after crash around rename/commit",
        check: blob_finalization_recovers_around_rename,
        limit: None,
    },
    Criterion {
        text: "DTN relay bundles survive crash",
        check: dtn_relay_bundles_survive_a_crash,
        limit: Some("restore/dedup decisions are proven; the DTN bundle *wire* representation does not exist yet in this workspace (see the emergency-node note in ROADMAP)"),
    },
    Criterion {
        text: "relay/destination ACK ambiguity is idempotent",
        check: ack_ambiguity_is_idempotent,
        limit: None,
    },
    Criterion {
        text: "device revocation survives crash immediately",
        check: revocation_survives_crash_immediately,
        limit: None,
    },
    Criterion {
        text: "stale sessions/capabilities/routes are invalidated",
        check: stale_sessions_capabilities_routes_are_invalidated,
        limit: None,
    },
    Criterion {
        text: "storage reservations and temp files are reconciled",
        check: reservations_and_temp_files_are_reconciled,
        limit: None,
    },
    Criterion {
        text: "projections can resume/rebuild",
        check: projections_resume_or_rebuild,
        limit: None,
    },
    Criterion {
        text: "derived indexes can fail without losing authoritative data",
        check: derived_indexes_can_fail_without_losing_authoritative_data,
        limit: None,
    },
    Criterion {
        text: "read-only recovery mode exists",
        check: read_only_mode_exists,
        limit: None,
    },
    Criterion {
        text: "severe corruption fails closed",
        check: severe_corruption_fails_closed,
        limit: None,
    },
    Criterion {
        text: "recovery does not require Internet",
        check: recovery_needs_no_internet,
        limit: None,
    },
    Criterion {
        text: "recovery itself is idempotent",
        check: recovery_itself_is_idempotent,
        limit: None,
    },
    Criterion {
        text: "process-kill/fault-injection/property tests exist",
        check: process_kill_fault_injection_and_property_tests_exist,
        limit: None,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    pub items: Vec<AuditItem>,
}

impl AuditReport {
    pub fn count(&self, s: AuditStatus) -> usize {
        self.items.iter().filter(|i| i.status == s).count()
    }

    pub fn all_pass(&self) -> bool {
        self.count(AuditStatus::NotDone) == 0
    }
}

/// Runs every §184 check.
pub fn audit() -> AuditReport {
    audit_of(&CRITERIA)
}

fn audit_of(criteria: &[Criterion]) -> AuditReport {
    let items = criteria
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let passed = (c.check)();
            let (status, note) = match (passed, c.limit) {
                (false, _) => (AuditStatus::NotDone, "check failed"),
                (true, Some(n)) => (AuditStatus::PartiallyDone, n),
                (true, None) => (AuditStatus::Done, ""),
            };
            AuditItem {
                number: i as u8 + 1,
                criterion: c.text,
                status,
                note,
            }
        })
        .collect();
    AuditReport { items }
}

// ---------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------

/// Separate from the twenty criteria on purpose: it is a fact about
/// the *workspace*, not about this crate's logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wiring {
    /// Workspace crates (other than this one) that depend on it.
    pub callers: Vec<String>,
}

impl Wiring {
    pub fn is_wired(&self) -> bool {
        !self.callers.is_empty()
    }
}

/// Scans `<crates_dir>/*/Cargo.toml` for dependents. `None` when the
/// directory isn't readable (e.g. the crate was vendored on its own).
pub fn measure_wiring(crates_dir: &Path, own_name: &str) -> Option<Wiring> {
    let mut callers = Vec::new();
    for entry in std::fs::read_dir(crates_dir).ok()? {
        let dir = entry.ok()?.path();
        let toml = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&toml) else {
            continue;
        };
        let name = dir.file_name()?.to_string_lossy().into_owned();
        if name == own_name {
            continue;
        }
        if text.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with(&format!("{own_name} ")) || l.starts_with(&format!("{own_name}="))
        }) {
            callers.push(name);
        }
    }
    callers.sort();
    Some(Wiring { callers })
}

// ---------------------------------------------------------------------
// §186
// ---------------------------------------------------------------------

pub const FINAL_PRINCIPLE: &str = "A crash changes execution timing, not logical truth.";

/// §186's four consequences, each verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrincipleClause {
    /// "If the system accepted a durable operation before the crash,
    /// recovery finds it."
    AcceptedOperationIsFound,
    /// "If an external side effect may have happened, recovery retries
    /// safely using the same stable identity."
    AmbiguousEffectRetriesSameIdentity,
    /// "If only derived state was lost, it is rebuilt."
    LostDerivedStateIsRebuilt,
    /// "If authoritative state is corrupt, the system fails closed
    /// rather than inventing data."
    CorruptAuthoritativeStateFailsClosed,
}

impl PrincipleClause {
    pub const ALL: [PrincipleClause; 4] = [
        PrincipleClause::AcceptedOperationIsFound,
        PrincipleClause::AmbiguousEffectRetriesSameIdentity,
        PrincipleClause::LostDerivedStateIsRebuilt,
        PrincipleClause::CorruptAuthoritativeStateFailsClosed,
    ];

    pub fn holds(self) -> bool {
        match self {
            PrincipleClause::AcceptedOperationIsFound => committed_intent_survives(),
            PrincipleClause::AmbiguousEffectRetriesSameIdentity => {
                ambiguous_send_retries_with_the_same_id() && receiver_deduplicates_retries()
            }
            PrincipleClause::LostDerivedStateIsRebuilt => {
                projections_resume_or_rebuild()
                    && derived_indexes_can_fail_without_losing_authoritative_data()
            }
            PrincipleClause::CorruptAuthoritativeStateFailsClosed => {
                severe_corruption_fails_closed()
            }
        }
    }
}

pub fn final_principle_holds() -> bool {
    PrincipleClause::ALL.iter().all(|c| c.holds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_lists_twenty_criteria() {
        assert_eq!(CRITERIA.len(), 20);
        let r = audit();
        assert_eq!(r.items.len(), 20);
        for (i, it) in r.items.iter().enumerate() {
            assert_eq!(it.number as usize, i + 1);
        }
    }

    #[test]
    fn every_criterion_check_passes() {
        let r = audit();
        let failed: Vec<_> = r
            .items
            .iter()
            .filter(|i| i.status == AuditStatus::NotDone)
            .map(|i| (i.number, i.criterion))
            .collect();
        assert!(failed.is_empty(), "failing criteria: {failed:?}");
        assert!(r.all_pass());
    }

    #[test]
    fn partial_items_always_say_what_is_missing() {
        for it in audit().items {
            match it.status {
                AuditStatus::Done => assert!(it.note.is_empty(), "{}", it.criterion),
                AuditStatus::PartiallyDone => {
                    assert!(it.note.len() > 20, "{}: vague note", it.criterion)
                }
                AuditStatus::NotDone => panic!("{} failed", it.criterion),
            }
        }
    }

    #[test]
    fn the_honest_counts() {
        let r = audit();
        assert_eq!(r.count(AuditStatus::NotDone), 0);
        assert_eq!(r.count(AuditStatus::PartiallyDone), 4);
        assert_eq!(r.count(AuditStatus::Done), 16);
    }

    #[test]
    fn a_failing_check_is_reported_not_hidden() {
        fn never() -> bool {
            false
        }
        fn always() -> bool {
            true
        }
        let doctored = [
            Criterion {
                text: "good",
                check: always,
                limit: None,
            },
            Criterion {
                text: "bad",
                check: never,
                limit: Some("a limit that must not mask the failure"),
            },
            Criterion {
                text: "limited",
                check: always,
                limit: Some("a documented limit"),
            },
        ];
        let r = audit_of(&doctored);
        assert_eq!(r.items[0].status, AuditStatus::Done);
        assert_eq!(
            r.items[1].status,
            AuditStatus::NotDone,
            "a limit never turns a failure into a pass"
        );
        assert_eq!(r.items[2].status, AuditStatus::PartiallyDone);
        assert!(!r.all_pass());
        assert_eq!(
            (r.count(AuditStatus::Done), r.count(AuditStatus::NotDone)),
            (1, 1)
        );
    }

    #[test]
    fn wiring_is_measured_against_the_real_workspace() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let w = measure_wiring(&crates, "siar-crash-recovery").expect("workspace crates dir");
        // Honest current state, pinned so it is noticed the day it
        // changes: nothing else depends on this crate yet.
        assert!(
            !w.is_wired(),
            "now wired into: {:?} — update the audit note",
            w.callers
        );
        assert!(measure_wiring(Path::new("/nonexistent-dir-xyz"), "x").is_none());
    }

    #[test]
    fn measure_wiring_finds_a_dependent() {
        let dir =
            std::env::temp_dir().join(format!("siar-wiring-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(
            dir.join("a/Cargo.toml"),
            "[dependencies]\nmy-crate = { path = \"../my-crate\" }\n",
        )
        .unwrap();
        std::fs::write(dir.join("b/Cargo.toml"), "[dependencies]\nother = \"1\"\n").unwrap();
        let w = measure_wiring(&dir, "my-crate").unwrap();
        assert_eq!(w.callers, vec!["a".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_final_principle_is_stated_and_all_four_clauses_hold() {
        assert_eq!(
            FINAL_PRINCIPLE,
            "A crash changes execution timing, not logical truth."
        );
        for c in PrincipleClause::ALL {
            assert!(c.holds(), "{c:?}");
        }
        assert!(final_principle_holds());
    }
}
