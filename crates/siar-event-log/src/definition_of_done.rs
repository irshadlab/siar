//! `04-offline-event-log-architecture.md` §93 "Definition of Done" — a
//! self-audit, in the same form specs 01-03 close with: every one of the
//! spec's 21 criteria, an honest status, and the evidence for it.
//!
//! "Done" here means **done for this crate** — the mechanism exists, is
//! tested, and matches the spec. Where a criterion is really about a
//! *consumer* wiring the mechanism in (the UI, the file subsystem, the
//! messenger's send path), that is called out rather than counted as
//! done: this crate cannot make `siar-messaging` call it.
//!
//! The table is data ([`DEFINITION_OF_DONE`]) so a test pins its shape
//! and anything marked less than `Done` stays visible in one place.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Implemented and tested in this crate.
    Done,
    /// Mechanism done here; something specific is still open.
    PartiallyDone,
    /// The criterion is satisfied (or not) by a different crate; this
    /// crate provides the seam.
    OutsideThisCrate,
}

#[derive(Debug, Clone, Copy)]
pub struct Criterion {
    pub text: &'static str,
    pub status: Status,
    pub evidence: &'static str,
}

use Status::{Done, OutsideThisCrate, PartiallyDone};

/// The spec's 21 criteria, in its own order.
pub const DEFINITION_OF_DONE: [Criterion; 21] = [
    Criterion {
        text: "accepted local commands survive process death",
        status: Done,
        evidence: "stoolap-backed store + §56 durability; tests/acceptance.rs recovery_acceptance_test and the SOS scenarios drop every object and reopen the file",
    },
    Criterion {
        text: "operations can be accepted without Internet",
        status: Done,
        evidence: "the crate has no network dependency; CommandFlow commits locally and effects run later",
    },
    Criterion {
        text: "IDs require no central server",
        status: Done,
        evidence: "EventId/StreamId are client-generated; OperationId is derived deterministically (outbox.rs)",
    },
    Criterion {
        text: "stream versions provide deterministic ordering",
        status: Done,
        evidence: "§12 expected_version guard; strictly-increasing property in tests/property_invariants.rs",
    },
    Criterion {
        text: "duplicate remote events are idempotent",
        status: Done,
        evidence: "§24 EventId idempotency (survives redaction); reassembly.rs convergence proptest with duplicate deliveries",
    },
    Criterion {
        text: "projections rebuild deterministically",
        status: Done,
        evidence: "tests/property_invariants.rs live==rebuild; search.rs fingerprint equality; snapshot.rs snapshot+tail==full replay",
    },
    Criterion {
        text: "checkpoints recover after crashes",
        status: Done,
        evidence: "tests/crash_injection.rs (after-event-before-projection) and StoolapCheckpointStore",
    },
    Criterion {
        text: "UI reads optimized projections",
        status: PartiallyDone,
        evidence: "boundary.rs gives UIs a read-only handle and no append capability; siar-messaging has ConversationSummaryProjection. NOT verified: that apps/desktop reads only projections (AppEvent is defined but still unconsumed there)",
    },
    Criterion {
        text: "large data is referenced via blobs",
        status: PartiallyDone,
        evidence: "blobref.rs BlobRef + require_blob_for_large + limits.rs hard ceiling enforce it at the journal. The blob subsystem and its call sites are Part 05 (siar-blob-manifest)",
    },
    Criterion {
        text: "message outbox survives restart",
        status: Done,
        evidence: "durable stoolap outbox (tests/outbox_durability.rs, process-death test). Note: siar-messaging's own send path still uses siar-storage's equally durable OutboxRepository — see siar-messaging::event_policy for why it was not swapped",
    },
    Criterion {
        text: "file semantic state survives restart",
        status: OutsideThisCrate,
        evidence: "siar-blob-manifest transfer journal; this crate supplies the event + outbox substrate and the per-chunk-event guard (limits.rs)",
    },
    Criterion {
        text: "device lifecycle remains auditable",
        status: OutsideThisCrate,
        evidence: "siar-identity-audit-recorder records it; this crate supplies SecurityAudit retention (never compacted) and integrity.rs hash-chain anchors to make an audit stream tamper- and rollback-evident",
    },
    Criterion {
        text: "SOS is persisted before transmission",
        status: Done,
        evidence: "CommandFlow commit-before-effect; tests/acceptance.rs asserts the outbox row exists with attempts == 0 before the radio is touched, then survival through restart to delivered / cancelled / expired. (The emergency service's own wiring is siar-emergency-service's)",
    },
    Criterion {
        text: "DTN lifecycle is durable",
        status: OutsideThisCrate,
        evidence: "siar-dtn-bundle-service; same substrate as above",
    },
    Criterion {
        text: "replication scope is explicit",
        status: Done,
        evidence: "replication.rs ScopeTable fails closed (unregistered = LocalOnly); messaging's table in siar-messaging::event_policy",
    },
    Criterion {
        text: "event schemas are versioned",
        status: Done,
        evidence: "schema_version on every event; upcast.rs deterministic upcasting with golden tests; stoolap_store storage-schema stamp refusing newer databases (tests/schema_version.rs)",
    },
    Criterion {
        text: "replay never accidentally re-runs external effects",
        status: Done,
        evidence: "ReplayMode gate in EffectProcessor::run_once; command_flow.rs test rebuilds a projection twice and the handler count stays 0",
    },
    Criterion {
        text: "storage-full is handled safely",
        status: Done,
        evidence: "§68 StorageFull rejects the whole batch before any write; diagnostics.rs reports it; read_only.rs fallback",
    },
    Criterion {
        text: "no external side effect occurs before durable commit",
        status: PartiallyDone,
        evidence: "structurally true for everything routed through CommandFlow (no handler is reachable from it). Not true by construction for callers that bypass it; siar-messaging's existing path commits to siar-storage before sending",
    },
    Criterion {
        text: "crash/property/fuzz tests exist",
        status: PartiallyDone,
        evidence: "crash (5 stages), property, golden and proptest-based decoder-robustness tests all run in CI. cargo-fuzz targets exist in fuzz/ but were NOT run (needs nightly + cargo-fuzz)",
    },
    Criterion {
        text: "the subsystem works outside the messenger",
        status: Done,
        evidence: "no messaging types anywhere in this crate; depends only on siar-domain's id types; tests/acceptance.rs runs generic domains (settings, SOS) with no messenger",
    },
];

/// Counts by status: `(done, partially, outside)`.
pub fn tally() -> (usize, usize, usize) {
    DEFINITION_OF_DONE
        .iter()
        .fold((0, 0, 0), |(d, p, o), c| match c.status {
            Done => (d + 1, p, o),
            PartiallyDone => (d, p + 1, o),
            OutsideThisCrate => (d, p, o + 1),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_21_spec_criteria_are_present_with_evidence() {
        assert_eq!(DEFINITION_OF_DONE.len(), 21);
        for c in &DEFINITION_OF_DONE {
            assert!(!c.text.is_empty() && c.evidence.len() > 20, "{}", c.text);
        }
    }

    #[test]
    fn the_tally_is_pinned_so_any_change_is_a_visible_decision() {
        // 14 done here, 4 partially done (UI projections, blob call
        // sites, side-effect-before-commit for non-CommandFlow callers,
        // fuzz not run), 3 satisfied outside this crate.
        assert_eq!(tally(), (14, 4, 3));
    }

    #[test]
    fn nothing_is_marked_done_while_its_evidence_admits_a_gap() {
        for c in DEFINITION_OF_DONE.iter().filter(|c| c.status == Done) {
            assert!(
                !c.evidence.contains("NOT ") && !c.evidence.contains("not run"),
                "{} is Done but its evidence names a gap",
                c.text
            );
        }
    }
}
