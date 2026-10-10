//! `04-offline-event-log-architecture.md` §66 "Security".
//!
//! §66 lists seven threats. Most are already answered by a specific
//! mechanism elsewhere; this module supplies the two that were not —
//! **rollback** and **hash-chain tamper detection** — and this table is
//! the map for all of them:
//!
//! | §66 threat | answered by |
//! |---|---|
//! | malformed imported event | the §23 pipeline (`siar-remote-ingestion`) before storage; [`crate::unknown`] for unrecognised types; [`crate::limits`] for shape |
//! | duplicate / replay | §24 idempotency on `EventId` (survives [`crate::redaction`]); [`crate::reassembly`] duplicate detection |
//! | **rollback** | **this module** — [`StreamAnchor`] |
//! | oversized payload | [`crate::limits`] (§55), checked before any write |
//! | unauthorized remote event | [`crate::replication::ReplicationAuthorizer`] outbound; §23 inbound |
//! | projection poisoning | projections are derived and disposable (§17/§38): [`crate::snapshot`] discards what it can't verify, a poisoned projection is rebuilt from the log; hostile *content* is stopped upstream (size, authorization) |
//! | local corruption | §22 row checksums in [`crate::stoolap_store`] → `Corrupt` → [`crate::read_only`] |
//! | **tampering** in security-critical streams | **this module** — the hash chain |
//!
//! ## The chain
//!
//! [`chain_step`] folds each event into a running hash:
//! `h(n) = H(h(n-1) ‖ event n)`. The hash after event *n* commits to
//! *every* event up to and including it, in order. Changing, removing,
//! reordering or inserting any earlier event changes every later hash.
//! §66: "security-critical event domains **may** use signatures/hash
//! chains" — a domain opts a stream in (device revocations, membership
//! changes); nothing here taxes ordinary message streams.
//!
//! ## Why an anchor, and why it must live elsewhere
//!
//! A chain computed from the log alone proves only *internal*
//! consistency: an attacker who can rewrite the database can recompute
//! the whole chain. What makes tampering and rollback detectable is a
//! [`StreamAnchor`] — `(stream, version, hash, coverage)` — stored
//! **outside the database being protected**: the platform keystore, a
//! file the DB process can't write, a peer's attestation. Verification
//! then asks two questions of the current log:
//!
//! - does it still reach `anchor.version`? If not →
//!   [`IntegrityViolation::RolledBack`]: an older database was swapped
//!   in, or events were deleted. (Rollback is the nastiest case for an
//!   append-only log: every remaining event is individually valid, so
//!   checksums and the chain are both fine — only an external memory of
//!   "we had reached version N" notices.)
//! - does the chain up to `anchor.version` still hash to `anchor.hash`?
//!   If not → [`IntegrityViolation::Tampered`].
//!
//! A log that has simply grown past the anchor verifies, and
//! [`advance_anchor`] moves the anchor forward — but only from a
//! verified state, so an anchor can never be advanced over damage.
//!
//! This module is the mechanism. Where the anchor is kept is the
//! deployment's decision; [`InMemoryAnchorStore`] is the test double
//! and, deliberately, not a place to keep a real one.
//!
//! ## Coverage and legitimate redaction
//!
//! [`ChainCoverage::Full`] hashes payloads, so a legitimate
//! [`crate::redaction`] *does* break the chain (correctly: the bytes
//! changed). Streams that may be redacted use
//! [`ChainCoverage::EnvelopeOnly`], which commits to ids, versions,
//! types, timestamps and origin but not payload bytes — still enough to
//! catch reordering, insertion, deletion and rollback, which are the
//! structural attacks. Security-audit streams should be `Full` *and*
//! [`crate::retention::RetentionClass::SecurityAudit`], which is never
//! compacted.

use std::collections::HashMap;
use std::sync::Mutex;

use thiserror::Error;

use crate::envelope::{EventEnvelope, EventOrigin};
use crate::ids::StreamId;
use crate::store::{EventStore, EventStoreError};

pub type ChainHash = [u8; 32];

/// The chain's starting value, before any event.
pub const CHAIN_GENESIS: ChainHash = [0; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainCoverage {
    /// Commits to payload bytes. Legitimate redaction breaks it.
    Full,
    /// Commits to everything except payload bytes.
    EnvelopeOnly,
}

pub fn chain_step(prev: &ChainHash, e: &EventEnvelope, coverage: ChainCoverage) -> ChainHash {
    let mut h = blake3::Hasher::new();
    h.update(b"siar/stream-chain/v1");
    h.update(&[match coverage {
        ChainCoverage::Full => 1,
        ChainCoverage::EnvelopeOnly => 2,
    }]);
    h.update(prev);
    h.update(e.event_id.as_uuid().as_bytes());
    h.update(e.stream_id.as_bytes());
    h.update(&e.stream_version.to_le_bytes());
    h.update(&e.event_type.0.to_le_bytes());
    h.update(&e.schema_version.to_le_bytes());
    h.update(&e.created_at.0.to_le_bytes());
    match e.origin {
        EventOrigin::LocalDevice(d) => {
            h.update(&[1]);
            h.update(d.as_uuid().as_bytes());
        }
        EventOrigin::RemoteDevice(d) => {
            h.update(&[2]);
            h.update(d.as_uuid().as_bytes());
        }
        EventOrigin::Imported => {
            h.update(&[3]);
        }
        EventOrigin::Recovery => {
            h.update(&[4]);
        }
        EventOrigin::System => {
            h.update(&[5]);
        }
    }
    if coverage == ChainCoverage::Full {
        h.update(&(e.payload.len() as u64).to_le_bytes());
        h.update(&e.payload);
    }
    *h.finalize().as_bytes()
}

/// A trusted memory of how far a stream had provably got. See the
/// module doc for where it must be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamAnchor {
    pub stream: StreamId,
    pub version: u64,
    pub hash: ChainHash,
    pub coverage: ChainCoverage,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrityViolation {
    /// The log no longer reaches the anchored version.
    #[error("stream rolled back: anchored at version {anchor_version}, log head is {head}")]
    RolledBack { anchor_version: u64, head: u64 },
    /// An event at or before the anchored version differs from the one
    /// the anchor committed to. `first_divergent_version` is the first
    /// version at which the *running chain* can no longer match — it
    /// is the version at which the mismatch was **detected**, which is
    /// the anchor's own version (a chain hash commits to everything
    /// before it, so the mismatch cannot be localized further without a
    /// per-event anchor trail).
    #[error(
        "stream tampered: chain through version {anchor_version} no longer matches its anchor"
    )]
    Tampered { anchor_version: u64 },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IntegrityError {
    #[error(transparent)]
    Violation(#[from] IntegrityViolation),
    #[error(transparent)]
    Store(#[from] EventStoreError),
}

const PAGE: usize = 512;

/// Chain hash through `up_to` (or the head if `None`), plus the version
/// actually reached.
async fn fold_chain<S: EventStore + ?Sized>(
    store: &S,
    stream: StreamId,
    coverage: ChainCoverage,
    start_hash: ChainHash,
    start_version: u64,
    up_to: Option<u64>,
) -> Result<(ChainHash, u64), EventStoreError> {
    let mut hash = start_hash;
    let mut version = start_version;
    loop {
        if up_to.is_some_and(|t| version >= t) {
            break;
        }
        let page = store.read_stream(stream, version, PAGE).await?;
        if page.is_empty() {
            break;
        }
        for e in &page {
            if up_to.is_some_and(|t| e.envelope.stream_version > t) {
                return Ok((hash, version));
            }
            hash = chain_step(&hash, &e.envelope, coverage);
            version = e.envelope.stream_version;
        }
    }
    Ok((hash, version))
}

/// Anchors `stream` at its current head. Call it when you have reason
/// to trust the log *right now* (stream creation, a freshly verified
/// state) and persist the result somewhere the database can't reach.
pub async fn compute_anchor<S: EventStore + ?Sized>(
    store: &S,
    stream: StreamId,
    coverage: ChainCoverage,
) -> Result<StreamAnchor, EventStoreError> {
    let (hash, version) = fold_chain(store, stream, coverage, CHAIN_GENESIS, 0, None).await?;
    Ok(StreamAnchor {
        stream,
        version,
        hash,
        coverage,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub head: u64,
    /// Events appended since the anchor (0 = unchanged).
    pub extended_by: u64,
}

pub async fn verify_against_anchor<S: EventStore + ?Sized>(
    store: &S,
    anchor: &StreamAnchor,
) -> Result<Verified, IntegrityError> {
    let head = store.stream_head(anchor.stream).await?;
    if head < anchor.version {
        return Err(IntegrityViolation::RolledBack {
            anchor_version: anchor.version,
            head,
        }
        .into());
    }
    let (hash, reached) = fold_chain(
        store,
        anchor.stream,
        anchor.coverage,
        CHAIN_GENESIS,
        0,
        Some(anchor.version),
    )
    .await?;
    if reached != anchor.version || hash != anchor.hash {
        return Err(IntegrityViolation::Tampered {
            anchor_version: anchor.version,
        }
        .into());
    }
    Ok(Verified {
        head,
        extended_by: head - anchor.version,
    })
}

/// Verifies `anchor`, then returns a new anchor at the current head.
/// Refuses (returns the violation) rather than ever anchoring over a
/// log that failed verification.
pub async fn advance_anchor<S: EventStore + ?Sized>(
    store: &S,
    anchor: &StreamAnchor,
) -> Result<StreamAnchor, IntegrityError> {
    verify_against_anchor(store, anchor).await?;
    // Continue the chain from the verified anchor instead of
    // re-reading the whole stream.
    let (hash, version) = fold_chain(
        store,
        anchor.stream,
        anchor.coverage,
        anchor.hash,
        anchor.version,
        None,
    )
    .await?;
    Ok(StreamAnchor {
        stream: anchor.stream,
        version,
        hash,
        coverage: anchor.coverage,
    })
}

/// Where anchors are kept. A real implementation must be *outside* the
/// protected database — see the module doc.
pub trait AnchorStore: Send + Sync {
    fn load(&self, stream: StreamId) -> Option<StreamAnchor>;
    fn save(&self, anchor: StreamAnchor);
}

/// Test double. Holding the anchor in the same process memory as the
/// store protects nothing against an attacker with process access.
#[derive(Debug, Default)]
pub struct InMemoryAnchorStore {
    anchors: Mutex<HashMap<StreamId, StreamAnchor>>,
}

impl InMemoryAnchorStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AnchorStore for InMemoryAnchorStore {
    fn load(&self, stream: StreamId) -> Option<StreamAnchor> {
        self.anchors.lock().unwrap().get(&stream).copied()
    }
    fn save(&self, anchor: StreamAnchor) {
        self.anchors.lock().unwrap().insert(anchor.stream, anchor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{EventId, EventTypeId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::redaction::RedactableStore;
    use crate::store::{AppendRequest, NewEvent};
    use proptest::prelude::*;
    use siar_domain::DeviceId;

    fn ev(n: u8) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp(n as u64),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: vec![n],
        }
    }

    async fn filled(stream: StreamId, n: u8) -> (InMemoryEventStore, Vec<NewEvent>) {
        let store = InMemoryEventStore::new();
        let evs: Vec<_> = (1..=n).map(ev).collect();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: evs.clone(),
            })
            .await
            .unwrap();
        (store, evs)
    }

    fn s() -> StreamId {
        StreamId::from_name("revocations")
    }

    #[tokio::test]
    async fn an_unchanged_or_grown_log_verifies_and_the_anchor_advances() {
        let (store, _) = filled(s(), 5).await;
        let a = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();
        assert_eq!(a.version, 5);
        assert_eq!(
            verify_against_anchor(&store, &a).await.unwrap(),
            Verified {
                head: 5,
                extended_by: 0
            }
        );

        store
            .append(AppendRequest {
                stream_id: s(),
                expected_version: 5,
                events: vec![ev(6), ev(7)],
            })
            .await
            .unwrap();
        assert_eq!(
            verify_against_anchor(&store, &a).await.unwrap(),
            Verified {
                head: 7,
                extended_by: 2
            }
        );
        let a2 = advance_anchor(&store, &a).await.unwrap();
        assert_eq!(a2.version, 7);
        // Advancing incrementally equals recomputing from scratch.
        assert_eq!(
            a2,
            compute_anchor(&store, s(), ChainCoverage::Full)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_swapped_in_older_database_is_a_rollback() {
        let (store, evs) = filled(s(), 6).await;
        let anchor = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();

        // Attacker restores an older copy: same first 3 events only.
        let older = InMemoryEventStore::new();
        older
            .append(AppendRequest {
                stream_id: s(),
                expected_version: 0,
                events: evs[..3].to_vec(),
            })
            .await
            .unwrap();
        // Every remaining event is individually valid and the prefix
        // chain is intact — only the anchor notices.
        assert_eq!(
            verify_against_anchor(&older, &anchor).await.unwrap_err(),
            IntegrityError::Violation(IntegrityViolation::RolledBack {
                anchor_version: 6,
                head: 3
            })
        );
    }

    #[tokio::test]
    async fn rewriting_history_is_tampering_even_at_the_same_length() {
        let (store, evs) = filled(s(), 4).await;
        let anchor = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();

        // Same length, but event 2 replaced by a different event.
        let forged = InMemoryEventStore::new();
        let mut forged_evs = evs.clone();
        forged_evs[1] = ev(99);
        forged
            .append(AppendRequest {
                stream_id: s(),
                expected_version: 0,
                events: forged_evs,
            })
            .await
            .unwrap();
        assert_eq!(
            verify_against_anchor(&forged, &anchor).await.unwrap_err(),
            IntegrityError::Violation(IntegrityViolation::Tampered { anchor_version: 4 })
        );
        // And an anchor is never advanced over damage.
        assert!(advance_anchor(&forged, &anchor).await.is_err());
    }

    #[tokio::test]
    async fn reordering_is_detected() {
        let (_, evs) = filled(s(), 3).await;
        let a = {
            let (st, _) = (InMemoryEventStore::new(), ());
            st.append(AppendRequest {
                stream_id: s(),
                expected_version: 0,
                events: evs.clone(),
            })
            .await
            .unwrap();
            compute_anchor(&st, s(), ChainCoverage::EnvelopeOnly)
                .await
                .unwrap()
        };
        let reordered = InMemoryEventStore::new();
        let mut r = evs.clone();
        r.swap(0, 2);
        reordered
            .append(AppendRequest {
                stream_id: s(),
                expected_version: 0,
                events: r,
            })
            .await
            .unwrap();
        assert!(matches!(
            verify_against_anchor(&reordered, &a).await,
            Err(IntegrityError::Violation(
                IntegrityViolation::Tampered { .. }
            ))
        ));
    }

    #[tokio::test]
    async fn payload_tampering_is_caught_by_full_coverage_only() {
        let (store, evs) = filled(s(), 3).await;
        let full = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();
        let env_only = compute_anchor(&store, s(), ChainCoverage::EnvelopeOnly)
            .await
            .unwrap();
        assert_ne!(full.hash, env_only.hash);

        store.tamper_payload(evs[1].event_id, b"forged".to_vec());
        assert!(matches!(
            verify_against_anchor(&store, &full).await,
            Err(IntegrityError::Violation(
                IntegrityViolation::Tampered { .. }
            ))
        ));
        // By design EnvelopeOnly does not commit to payload bytes.
        assert!(verify_against_anchor(&store, &env_only).await.is_ok());
    }

    #[tokio::test]
    async fn legitimate_redaction_breaks_full_but_not_envelope_only() {
        let (store, evs) = filled(s(), 3).await;
        let full = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();
        let env_only = compute_anchor(&store, s(), ChainCoverage::EnvelopeOnly)
            .await
            .unwrap();

        store.redact_payloads(&[evs[0].event_id]).await.unwrap();
        assert!(verify_against_anchor(&store, &full).await.is_err());
        assert!(verify_against_anchor(&store, &env_only).await.is_ok());
    }

    #[tokio::test]
    async fn an_empty_stream_anchors_at_genesis_and_the_store_trait_object_works() {
        let store = InMemoryEventStore::new();
        let a = compute_anchor(&store, s(), ChainCoverage::Full)
            .await
            .unwrap();
        assert_eq!((a.version, a.hash), (0, CHAIN_GENESIS));
        let dynstore: &dyn EventStore = &store;
        assert!(verify_against_anchor(dynstore, &a).await.is_ok());

        let anchors = InMemoryAnchorStore::new();
        assert!(anchors.load(s()).is_none());
        anchors.save(a);
        assert_eq!(anchors.load(s()), Some(a));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        /// Mutating ANY single event of an anchored prefix is detected,
        /// whichever one and however the payload changes.
        #[test]
        fn any_single_mutation_of_the_anchored_prefix_is_detected(
            n in 2usize..20,
            victim in 0usize..20,
            flip in 1u8..255,
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            rt.block_on(async {
                let (store, evs) = filled(s(), n as u8).await;
                let anchor = compute_anchor(&store, s(), ChainCoverage::Full).await.unwrap();
                let victim = victim % n;
                store.tamper_payload(evs[victim].event_id, vec![evs[victim].payload[0] ^ flip]);
                prop_assert!(verify_against_anchor(&store, &anchor).await.is_err());
                Ok(())
            })?;
        }
    }
}
