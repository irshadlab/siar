//! `04-offline-event-log-architecture.md` §41 "Privacy and Deletion" and
//! §42 "Event Encryption" — §92 Phase 7.
//!
//! §41: "append-only design must not become an excuse to ignore deletion
//! requirements." Its four options, and where each lives:
//!
//! | §41 option | here |
//! |---|---|
//! | logical deletion event | the owning **domain** appends its own `MessageDeleted`-style event; this crate has no domain events (see `lib.rs`) |
//! | projection deletion | the domain's projection reacts to that event; [`DeletionReport::affected_streams`] says which streams' derived state to rebuild |
//! | physical retention policy | [`crate::retention`] (age-based) and [`execute_deletion`] (on request) — both over [`crate::redaction`] |
//! | crypto-erasure for sensitive retained payloads | [`SealedPayload`] + [`PayloadSealer::erase_key`] |
//!
//! "Exact behavior depends on product/legal requirements" — so this is
//! mechanism, not a policy choice about *what* to delete.
//!
//! ## [`execute_deletion`]: every copy this crate owns
//!
//! A payload doesn't live only in the log. One call removes it from:
//!
//! 1. the **log** — payloads redacted (envelope stays; see
//!    [`crate::redaction`] for why);
//! 2. **snapshots** of the affected streams — deleted, since a snapshot
//!    may embed derived content (§38 makes them safe to drop);
//! 3. the **outbox** — listed operations are scrubbed: payload emptied
//!    and open ones dead-lettered, so deleted content is neither retained
//!    nor sent later. The row is kept on purpose so §48 reconciliation
//!    still sees the operation as present.
//!
//! What it cannot reach is reported, not hidden: projections held by
//! other crates (rebuild from [`DeletionReport::affected_streams`]),
//! copies already replicated to peers (§49-53 — a deletion that must
//! propagate needs a domain deletion event they apply), and backups
//! ([`crate::backup`] — a backup taken before deletion still contains
//! the data; that is a retention decision about the backup itself).
//!
//! ## §42: key management stays outside
//!
//! "Keep cryptographic key management outside event-store internals."
//! This crate therefore defines only a *seam*: [`PayloadSealer`], with a
//! self-describing [`SealedPayload`] envelope (`magic ‖ key id ‖
//! ciphertext`). No cipher, key, or keystore lives here; the domain wires
//! in `siar-crypto`. The store keeps treating the sealed bytes as an
//! opaque payload — it neither knows nor cares that they are encrypted.
//!
//! **Crypto-erasure** is then one call: [`PayloadSealer::erase_key`]
//! destroys a key, and every payload sealed under it becomes permanently
//! unreadable while the events — ids, versions, shape of history —
//! remain exactly as they were. That is the right tool when the payload
//! must be *retained* (audit trail shape, replication consistency) but
//! its content must stop being recoverable, and it is the only option
//! that also covers copies in backups and on peers, provided the key
//! never left the sealer.

use thiserror::Error;

use crate::ids::{EventId, StreamId, Timestamp};
use crate::outbox::{OperationId, Outbox, OutboxError};
use crate::redaction::{RedactableStore, RedactionReport};
use crate::snapshot::{SnapshotError, SnapshotStore};
use crate::store::EventStoreError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionRequest {
    pub events: Vec<EventId>,
    /// Streams those events belong to (their snapshots are purged and
    /// they are reported for projection rebuild).
    pub streams: Vec<StreamId>,
    /// Outbox operations derived from those events. Compute with the
    /// deployment's [`crate::command_flow::EffectPlanner`] (the ids are
    /// deterministic: [`OperationId::derive`]).
    pub operations: Vec<OperationId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionReport {
    pub redaction: RedactionReport,
    pub snapshots_purged: u64,
    pub outbox_scrubbed: u64,
    /// Derived state this crate does not own and the caller must
    /// rebuild or purge.
    pub affected_streams: Vec<StreamId>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeletionError {
    #[error(transparent)]
    Store(#[from] EventStoreError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
}

/// Order matters: derived copies are removed **before** the source of
/// truth is redacted. If the process dies midway, a retry still finds
/// the events and finishes the job; the reverse order could leave a
/// snapshot or outbox payload that nothing points back to.
/// Every step is idempotent, so retrying the whole request is safe.
pub async fn execute_deletion<S, N, O>(
    store: &S,
    snapshots: &N,
    outbox: &O,
    request: &DeletionRequest,
    now: Timestamp,
) -> Result<DeletionReport, DeletionError>
where
    S: RedactableStore + ?Sized,
    N: SnapshotStore + ?Sized,
    O: Outbox + ?Sized,
{
    let mut snapshots_purged = 0;
    for stream in &request.streams {
        snapshots_purged += snapshots.delete_stream(*stream).await?;
    }
    let outbox_scrubbed = outbox
        .scrub(&request.operations, "source event deleted (§41)", now)
        .await?;
    let redaction = store.redact_payloads(&request.events).await?;

    let mut affected_streams = request.streams.clone();
    affected_streams.sort_unstable_by_key(|s| *s.as_bytes());
    affected_streams.dedup();
    Ok(DeletionReport {
        redaction,
        snapshots_purged,
        outbox_scrubbed,
        affected_streams,
    })
}

/// Names the key a payload was sealed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyId(pub [u8; 16]);

const SEAL_MAGIC: &[u8; 5] = b"SEAL1";

/// `SEAL1 ‖ key_id(16) ‖ ciphertext`. Self-describing so a reader can
/// tell a sealed payload from a plain one and find the key to ask for —
/// without this crate knowing anything about the cipher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedPayload {
    pub key_id: KeyId,
    pub ciphertext: Vec<u8>,
}

impl SealedPayload {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SEAL_MAGIC.len() + 16 + self.ciphertext.len());
        out.extend_from_slice(SEAL_MAGIC);
        out.extend_from_slice(&self.key_id.0);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// `None` if `bytes` is not a sealed payload (including: too short).
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let rest = bytes.strip_prefix(SEAL_MAGIC)?;
        let (key, ciphertext) = rest.split_at_checked(16)?;
        Some(Self {
            key_id: KeyId(key.try_into().ok()?),
            ciphertext: ciphertext.to_vec(),
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SealError {
    /// The key was erased (§41) or never existed here. Permanent by
    /// design — the whole point of crypto-erasure.
    #[error("key {0:?} is unavailable (erased or unknown)")]
    KeyUnavailable(KeyId),
    #[error("sealer failure: {0}")]
    Failed(String),
}

/// The §42 seam. Implemented outside this crate (against
/// `siar-crypto` and the platform keystore); the event store never
/// holds a key.
pub trait PayloadSealer: Send + Sync {
    fn seal(&self, plaintext: &[u8]) -> Result<SealedPayload, SealError>;
    fn open(&self, sealed: &SealedPayload) -> Result<Vec<u8>, SealError>;
    /// Irreversibly destroy `key`. Idempotent.
    fn erase_key(&self, key: KeyId) -> Result<(), SealError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_flow::tests_support::*;
    use crate::command_flow::CommandFlow;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventTypeId, LocalLogOffset};
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::OutboxState;
    use crate::snapshot::{InMemorySnapshotStore, Snapshot};
    use crate::store::{AppendRequest, EventStore};
    use siar_domain::DeviceId;
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    #[tokio::test]
    async fn deletion_removes_every_owned_copy_and_reports_the_rest() {
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("conversation/a");
        let other = StreamId::from_name("conversation/b");

        let secret = event(QUEUED, b"my secret message");
        let keep = event(QUEUED, b"unrelated");
        flow.commit(stream, 0, vec![secret.clone()], Timestamp(1))
            .await
            .unwrap();
        flow.commit(other, 0, vec![keep.clone()], Timestamp(1))
            .await
            .unwrap();
        snaps
            .save(Snapshot::new(stream, 1, 1, b"derived from secret".to_vec()))
            .await
            .unwrap();
        snaps
            .save(Snapshot::new(other, 1, 1, b"fine".to_vec()))
            .await
            .unwrap();

        let secret_op = OperationId::derive(secret.event_id, SEND_EFFECT, 0);
        let keep_op = OperationId::derive(keep.event_id, SEND_EFFECT, 0);

        let report = execute_deletion(
            &store,
            &snaps,
            &store.outbox(),
            &DeletionRequest {
                events: vec![secret.event_id],
                streams: vec![stream, stream],
                operations: vec![secret_op],
            },
            Timestamp(50),
        )
        .await
        .unwrap();

        assert_eq!(report.redaction.redacted, 1);
        assert_eq!(
            report.snapshots_purged, 1,
            "stream listed twice: 2nd delete is a no-op"
        );
        assert_eq!(report.outbox_scrubbed, 1);
        assert_eq!(report.affected_streams, vec![stream], "deduplicated");

        // Log: payload gone, slot intact.
        let got = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].envelope.payload.is_empty());
        // Snapshot of the affected stream gone; the other untouched.
        assert!(snaps.load_latest(stream).await.unwrap().is_none());
        assert!(snaps.load_latest(other).await.unwrap().is_some());
        // Outbox: scrubbed + dead-lettered, never sendable.
        let row = store.outbox().get(secret_op).await.unwrap().unwrap();
        assert!(row.payload.is_empty());
        assert_eq!(row.state, OutboxState::DeadLettered);
        assert!(store
            .outbox()
            .claim_due(Timestamp(1_000_000), 10, 10)
            .await
            .unwrap()
            .iter()
            .all(|e| e.operation_id == keep_op));
        // The unrelated message is completely untouched.
        assert_eq!(
            store.outbox().get(keep_op).await.unwrap().unwrap().payload,
            b"unrelated"
        );
    }

    #[tokio::test]
    async fn reconciliation_does_not_resurrect_a_deleted_operation() {
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("s");
        let secret = event(QUEUED, b"secret");
        flow.commit(stream, 0, vec![secret.clone()], Timestamp(1))
            .await
            .unwrap();
        let op = OperationId::derive(secret.event_id, SEND_EFFECT, 0);

        execute_deletion(
            &store,
            &InMemorySnapshotStore::new(),
            &store.outbox(),
            &DeletionRequest {
                events: vec![secret.event_id],
                streams: vec![stream],
                operations: vec![op],
            },
            Timestamp(2),
        )
        .await
        .unwrap();

        // §48 reconciliation sees the operation as present (row kept),
        // so it does not rebuild a send from the now-empty history.
        let r = crate::outbox_reconcile::reconcile_outbox(
            &store,
            &store.outbox(),
            &MessagePlanner,
            crate::outbox_reconcile::RepairMode::Repair,
            Timestamp(3),
        )
        .await
        .unwrap();
        assert!(r.missing.is_empty(), "{r:?}");
        assert_eq!(
            store.outbox().get(op).await.unwrap().unwrap().state,
            OutboxState::DeadLettered
        );
    }

    #[tokio::test]
    async fn deletion_is_idempotent_so_a_crashed_run_can_simply_repeat() {
        let store = InMemoryEventStore::new();
        let stream = StreamId::from_name("s");
        let e = event(QUEUED, b"x");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![e.clone()],
            })
            .await
            .unwrap();
        let req = DeletionRequest {
            events: vec![e.event_id],
            streams: vec![stream],
            operations: vec![],
        };
        let snaps = InMemorySnapshotStore::new();
        let first = execute_deletion(&store, &snaps, &store.outbox(), &req, Timestamp(1))
            .await
            .unwrap();
        let second = execute_deletion(&store, &snaps, &store.outbox(), &req, Timestamp(2))
            .await
            .unwrap();
        assert_eq!(first.redaction.redacted, 1);
        assert_eq!(
            second.redaction,
            RedactionReport {
                redacted: 0,
                already_redacted: 1,
                not_found: 0
            }
        );
    }

    #[test]
    fn sealed_payload_envelope_round_trips_and_rejects_non_sealed_bytes() {
        let s = SealedPayload {
            key_id: KeyId([7; 16]),
            ciphertext: vec![1, 2, 3],
        };
        assert_eq!(SealedPayload::decode(&s.encode()), Some(s));
        assert_eq!(SealedPayload::decode(b"plain text payload"), None);
        assert_eq!(SealedPayload::decode(b"SEAL1short"), None);
        // Empty ciphertext is still a valid envelope.
        assert!(SealedPayload::decode(
            &SealedPayload {
                key_id: KeyId([0; 16]),
                ciphertext: vec![]
            }
            .encode()
        )
        .is_some());
    }

    /// Toy, **insecure** XOR sealer — exists only to exercise the seam.
    struct ToySealer {
        keys: Mutex<HashMap<KeyId, u8>>,
        next: Mutex<u8>,
    }
    impl ToySealer {
        fn new() -> Self {
            Self {
                keys: Mutex::default(),
                next: Mutex::new(1),
            }
        }
    }
    impl PayloadSealer for ToySealer {
        fn seal(&self, plaintext: &[u8]) -> Result<SealedPayload, SealError> {
            let mut n = self.next.lock().unwrap();
            let id = KeyId([*n; 16]);
            let key = *n ^ 0x5a;
            *n += 1;
            self.keys.lock().unwrap().insert(id, key);
            Ok(SealedPayload {
                key_id: id,
                ciphertext: plaintext.iter().map(|b| b ^ key).collect(),
            })
        }
        fn open(&self, s: &SealedPayload) -> Result<Vec<u8>, SealError> {
            let key = *self
                .keys
                .lock()
                .unwrap()
                .get(&s.key_id)
                .ok_or(SealError::KeyUnavailable(s.key_id))?;
            Ok(s.ciphertext.iter().map(|b| b ^ key).collect())
        }
        fn erase_key(&self, key: KeyId) -> Result<(), SealError> {
            self.keys.lock().unwrap().remove(&key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn crypto_erasure_makes_payloads_unreadable_while_the_log_stays_intact() {
        let store = InMemoryEventStore::new();
        let sealer = ToySealer::new();
        let stream = StreamId::from_name("audit");

        let sealed = sealer.seal(b"sensitive audit detail").unwrap();
        let other = sealer.seal(b"another record").unwrap();
        let mut a = event(EventTypeId(9), b"");
        a.payload = sealed.encode();
        let mut b = event(EventTypeId(9), b"");
        b.payload = other.encode();
        a.origin = EventOrigin::LocalDevice(DeviceId::new());
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![a.clone(), b.clone()],
            })
            .await
            .unwrap();

        async fn read(
            store: &InMemoryEventStore,
            sealer: &ToySealer,
            stream: StreamId,
            i: usize,
        ) -> Option<Result<Vec<u8>, SealError>> {
            let e = &store.read_stream(stream, 0, 10).await.unwrap()[i];
            SealedPayload::decode(&e.envelope.payload).map(|s| sealer.open(&s))
        }
        assert_eq!(
            read(&store, &sealer, stream, 0).await.unwrap().unwrap(),
            b"sensitive audit detail"
        );

        sealer.erase_key(sealed.key_id).unwrap();
        sealer.erase_key(sealed.key_id).unwrap(); // idempotent

        assert_eq!(
            read(&store, &sealer, stream, 0).await.unwrap(),
            Err(SealError::KeyUnavailable(sealed.key_id))
        );
        assert_eq!(
            read(&store, &sealer, stream, 1).await.unwrap().unwrap(),
            b"another record",
            "other keys unaffected"
        );
        // The history is exactly as it was: ids, versions, offsets.
        let all = store.read_log(LocalLogOffset(0), 10).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].envelope.event_id, a.event_id);
        assert_eq!(all[0].envelope.stream_version, 1);
        let ids: HashSet<_> = all.iter().map(|e| e.envelope.event_id).collect();
        assert!(ids.contains(&b.event_id));
    }
}
