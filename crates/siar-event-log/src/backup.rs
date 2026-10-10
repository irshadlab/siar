//! `04-offline-event-log-architecture.md` §70 "Backup" and §71
//! "Restore Safety" — §92 Phase 7.
//!
//! §70: "incremental backup can use `LocalLogOffset` to export new
//! events since last backup. Restore should: verify → restore log →
//! restore blobs → rebuild projections → reconcile pending work
//! carefully." This module is the **log** half of that sequence (blobs
//! are Part 05; projections rebuild through the ordinary
//! [`crate::projection`] machinery) plus the careful reconciliation of
//! pending work.
//!
//! ## Export: a hash chain over offset ranges
//!
//! [`export_since`] returns a [`BackupChunk`] covering the events after
//! the last exported [`LocalLogOffset`]. Each chunk carries `digest =
//! H(prev_digest ‖ chunk contents)`, so a sequence of incremental
//! chunks forms a chain: a chunk that was altered, dropped from the
//! middle, or substituted from another backup fails verification
//! *before* anything is written. The caller persists
//! `(last offset, last digest)` as the backup cursor — the same
//! "resume from where we left off" shape as a projection checkpoint.
//!
//! The digest covers every semantic field of every event, including the
//! payload (or its tombstone — a redacted event backs up as the
//! tombstone it is). It deliberately **does** cover `local_offset`,
//! unlike the §22 row checksum: within a backup the offsets are the
//! ordering the chain asserts.
//!
//! ## Restore: offsets are reassigned, versions are not
//!
//! `LocalLogOffset` is device-local (§6). Restoring into a fresh store
//! assigns new offsets; stream versions, event ids and every other
//! envelope field are preserved exactly. Events keep their original
//! [`crate::envelope::EventOrigin`] — restoration does not turn
//! history written by this device into "imported" history. Restoring is
//! idempotent (§24): a chunk applied twice, or a target that already has
//! part of it, simply reports those events as already present.
//!
//! A target stream whose version disagrees with the backup (someone
//! appended different events there) is [`RestoreError::TargetDiverged`]
//! and restore stops — it will not interleave two histories.
//!
//! ## §71: the dangerous part is the outbox, not the log
//!
//! "Do not blindly re-run all previously pending effects after
//! restore." Rebuilding the outbox from restored history
//! ([`crate::outbox_reconcile`]) would, taken naively, resend every
//! message that was merely *queued* at backup time — including ones
//! delivered an hour after the backup, ones that have since expired, and
//! ones addressed to a device revoked in the meantime.
//! [`restore_outbox`] therefore puts a [`RestoreGuard`] between "history
//! says owed" and "the outbox will do it":
//!
//! - **already completed**: history's own `closes` (§48) removes
//!   discharged operations; the guard can additionally consult delivery
//!   state the log doesn't have;
//! - **expiry / revocation / current policy**: the guard's call —
//!   [`GuardVerdict::Drop`] or [`GuardVerdict::Hold`].
//!
//! A dropped operation is still written to the outbox, immediately
//! dead-lettered. That is deliberate: if it were simply not enqueued,
//! the next ordinary reconciliation would see "history says owed, no
//! row" and rebuild it, undoing the decision.

use thiserror::Error;

use crate::command_flow::EffectPlanner;
use crate::ids::{LocalLogOffset, Timestamp};
use crate::outbox::{EnqueueOutcome, OperationId, Outbox, OutboxEntry, OutboxError};
use crate::outbox_reconcile::{scan_history, ReconcileError};
use crate::store::{AppendRequest, EventStore, EventStoreError, NewEvent, StoredEvent};

pub type Digest = [u8; 32];

/// The chain's genesis `prev_digest`.
pub const BACKUP_GENESIS: Digest = [0; 32];

#[derive(Debug, Clone, PartialEq)]
pub struct BackupChunk {
    /// Exclusive lower bound: events with offset `> after_offset`.
    pub after_offset: LocalLogOffset,
    /// Offset of the last event included.
    pub last_offset: LocalLogOffset,
    pub events: Vec<StoredEvent>,
    pub prev_digest: Digest,
    pub digest: Digest,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BackupError {
    #[error(transparent)]
    Store(#[from] EventStoreError),
    #[error("backup chunk digest does not match its contents")]
    BadDigest,
    #[error("backup chunk does not continue the previous chunk's digest")]
    BrokenChain,
    #[error("backup chunk offsets are not strictly increasing within (after, last]")]
    BadOffsets,
}

fn hash_event(h: &mut blake3::Hasher, e: &StoredEvent) {
    use crate::envelope::EventOrigin;
    let env = &e.envelope;
    h.update(&e.local_offset.0.to_le_bytes());
    h.update(env.event_id.as_uuid().as_bytes());
    h.update(env.stream_id.as_bytes());
    h.update(&env.stream_version.to_le_bytes());
    h.update(&env.event_type.0.to_le_bytes());
    h.update(&env.schema_version.to_le_bytes());
    h.update(&env.created_at.0.to_le_bytes());
    match env.origin {
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
    for opt in [
        env.correlation_id.map(|c| *c.as_uuid().as_bytes()),
        env.causation_id.map(|c| *c.as_uuid().as_bytes()),
    ] {
        match opt {
            Some(b) => {
                h.update(&[1]);
                h.update(&b);
            }
            None => {
                h.update(&[0]);
            }
        }
    }
    h.update(&(env.payload.len() as u64).to_le_bytes());
    h.update(&env.payload);
}

fn chunk_digest(prev: &Digest, after: LocalLogOffset, events: &[StoredEvent]) -> Digest {
    let mut h = blake3::Hasher::new();
    h.update(b"siar/backup-chunk/v1");
    h.update(prev);
    h.update(&after.0.to_le_bytes());
    h.update(&(events.len() as u64).to_le_bytes());
    for e in events {
        hash_event(&mut h, e);
    }
    *h.finalize().as_bytes()
}

impl BackupChunk {
    /// Checks digest, offset ordering, and (if `expected_prev` is
    /// given) chain continuity. Does not touch any store.
    pub fn verify(&self, expected_prev: Option<&Digest>) -> Result<(), BackupError> {
        if let Some(p) = expected_prev {
            if *p != self.prev_digest {
                return Err(BackupError::BrokenChain);
            }
        }
        let mut last = self.after_offset;
        for e in &self.events {
            if e.local_offset <= last {
                return Err(BackupError::BadOffsets);
            }
            last = e.local_offset;
        }
        if self
            .events
            .last()
            .is_some_and(|e| e.local_offset != self.last_offset)
        {
            return Err(BackupError::BadOffsets);
        }
        if chunk_digest(&self.prev_digest, self.after_offset, &self.events) != self.digest {
            return Err(BackupError::BadDigest);
        }
        Ok(())
    }
}

/// The next incremental chunk after `after_offset`, or `None` if the
/// log has nothing newer. Pass `LocalLogOffset(0)` and
/// [`BACKUP_GENESIS`] for the first (full) backup.
pub async fn export_since<S: EventStore + ?Sized>(
    store: &S,
    after_offset: LocalLogOffset,
    prev_digest: Digest,
    limit: usize,
) -> Result<Option<BackupChunk>, BackupError> {
    let events = store.read_log(after_offset, limit).await?;
    let Some(last) = events.last() else {
        return Ok(None);
    };
    let last_offset = last.local_offset;
    let digest = chunk_digest(&prev_digest, after_offset, &events);
    Ok(Some(BackupChunk {
        after_offset,
        last_offset,
        events,
        prev_digest,
        digest,
    }))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RestoreError {
    #[error(transparent)]
    Backup(#[from] BackupError),
    #[error(transparent)]
    Store(#[from] EventStoreError),
    #[error("target stream disagrees with the backup at version {version}; refusing to interleave histories")]
    TargetDiverged { version: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RestoreReport {
    pub restored: u64,
    pub already_present: u64,
}

fn to_new_event(e: &StoredEvent) -> NewEvent {
    let env = &e.envelope;
    NewEvent {
        event_id: env.event_id,
        event_type: env.event_type,
        schema_version: env.schema_version,
        created_at: env.created_at,
        origin: env.origin,
        correlation_id: env.correlation_id,
        causation_id: env.causation_id,
        payload: env.payload.clone(),
    }
}

/// Verifies the **whole chain first** — nothing is written unless
/// every chunk checks out — then restores in order. `chain_start` is
/// the digest the first chunk must continue
/// ([`BACKUP_GENESIS`] for a full restore).
pub async fn restore_chunks<S: EventStore + ?Sized>(
    target: &S,
    chain_start: &Digest,
    chunks: &[BackupChunk],
) -> Result<RestoreReport, RestoreError> {
    let mut prev = *chain_start;
    for c in chunks {
        c.verify(Some(&prev))?;
        prev = c.digest;
    }

    let mut report = RestoreReport::default();
    for chunk in chunks {
        for e in &chunk.events {
            let env = &e.envelope;
            let head = target.stream_head(env.stream_id).await?;
            if env.stream_version <= head {
                // Present already — but is it the *same* event?
                let at = target
                    .read_stream(env.stream_id, env.stream_version - 1, 1)
                    .await?;
                match at.first() {
                    Some(s) if s.envelope.event_id == env.event_id => {
                        report.already_present += 1;
                        continue;
                    }
                    _ => {
                        return Err(RestoreError::TargetDiverged {
                            version: env.stream_version,
                        })
                    }
                }
            }
            if env.stream_version != head + 1 {
                return Err(RestoreError::TargetDiverged {
                    version: env.stream_version,
                });
            }
            let r = target
                .append(AppendRequest {
                    stream_id: env.stream_id,
                    expected_version: head,
                    events: vec![to_new_event(e)],
                })
                .await?;
            match r.local_offsets.first() {
                Some(Some(_)) => report.restored += 1,
                _ => report.already_present += 1,
            }
        }
    }
    Ok(report)
}

/// §71's verdict on one operation history says is still owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardVerdict {
    /// Still valid: let the outbox run it.
    Proceed,
    /// Not yet: enqueue it, due at `until`.
    Hold { until: Timestamp },
    /// Must not run (expired, recipient revoked, already delivered
    /// per state the log doesn't hold): recorded as dead-lettered.
    Drop { reason: &'static str },
}

/// Where §71's expiry / delivery-state / revocation checks live.
pub trait RestoreGuard: Send + Sync {
    fn assess(&self, entry: &OutboxEntry, now: Timestamp) -> GuardVerdict;
}

/// Proceeds with everything — correct only where the caller has proven
/// nothing could have changed since the backup. Exists so the *absence*
/// of a guard is a visible, greppable choice.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrustEverything;

impl RestoreGuard for TrustEverything {
    fn assess(&self, _e: &OutboxEntry, _now: Timestamp) -> GuardVerdict {
        GuardVerdict::Proceed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreOutboxReport {
    pub events_scanned: u64,
    pub enqueued: Vec<OperationId>,
    pub held: Vec<OperationId>,
    pub dropped: Vec<(OperationId, &'static str)>,
    pub already_present: u32,
    /// History said these were discharged: not rebuilt at all.
    pub discharged_in_history: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RestoreOutboxError {
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error(transparent)]
    Outbox(#[from] OutboxError),
}

/// §71 in one call: rebuild the outbox from restored history, but let
/// `guard` veto or delay each operation first. Run after
/// [`restore_chunks`] and *instead of* a plain
/// [`crate::outbox_reconcile::reconcile_outbox`] with `Repair` — that
/// would enqueue everything `Proceed`-style.
pub async fn restore_outbox<S, O, P, G>(
    store: &S,
    outbox: &O,
    planner: &P,
    guard: &G,
    now: Timestamp,
) -> Result<RestoreOutboxReport, RestoreOutboxError>
where
    S: EventStore + ?Sized,
    O: Outbox + ?Sized,
    P: EffectPlanner + ?Sized,
    G: RestoreGuard + ?Sized,
{
    let history = scan_history(store, planner).await?;
    let mut report = RestoreOutboxReport {
        events_scanned: history.events_scanned,
        discharged_in_history: history.closed.len() as u32,
        ..Default::default()
    };

    let mut ids: Vec<OperationId> = history.opened.keys().copied().collect();
    ids.sort_unstable();
    let mut opened = history.opened;

    for id in ids {
        if outbox.get(id).await?.is_some() {
            report.already_present += 1;
            continue;
        }
        let mut entry = opened.remove(&id).expect("id came from this map");
        match guard.assess(&entry, now) {
            GuardVerdict::Proceed => {
                // Due now, regardless of the event's (old) timestamp.
                entry.not_before = now;
                if outbox.enqueue(entry).await? == EnqueueOutcome::Inserted {
                    report.enqueued.push(id);
                }
            }
            GuardVerdict::Hold { until } => {
                entry.not_before = until;
                if outbox.enqueue(entry).await? == EnqueueOutcome::Inserted {
                    report.held.push(id);
                }
            }
            GuardVerdict::Drop { reason } => {
                entry.payload.clear();
                if outbox.enqueue(entry).await? == EnqueueOutcome::Inserted {
                    outbox.dead_letter(id, reason.to_string(), now).await?;
                    report.dropped.push((id, reason));
                }
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_flow::tests_support::*;
    use crate::command_flow::CommandFlow;
    use crate::ids::{EventId, StreamId};
    use crate::memory_store::InMemoryEventStore;
    use crate::outbox::{InMemoryOutbox, OutboxState, TransactionalOutbox};
    use std::collections::HashMap;

    async fn source_with_messages(n: u8) -> (InMemoryEventStore, StreamId, Vec<NewEvent>) {
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("conversation");
        let mut evs = Vec::new();
        for i in 0..n {
            let e = event(QUEUED, &[i]);
            flow.commit(stream, i as u64, vec![e.clone()], Timestamp(10))
                .await
                .unwrap();
            evs.push(e);
        }
        (store, stream, evs)
    }

    #[tokio::test]
    async fn incremental_chunks_chain_and_restore_reproduces_the_log() {
        let (src, stream, evs) = source_with_messages(5).await;

        let c1 = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 3)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((c1.events.len(), c1.last_offset), (3, LocalLogOffset(3)));
        let c2 = export_since(&src, c1.last_offset, c1.digest, 3)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(c2.events.len(), 2);
        assert!(export_since(&src, c2.last_offset, c2.digest, 3)
            .await
            .unwrap()
            .is_none());

        let dst = InMemoryEventStore::new();
        let r = restore_chunks(&dst, &BACKUP_GENESIS, &[c1.clone(), c2.clone()])
            .await
            .unwrap();
        assert_eq!(
            r,
            RestoreReport {
                restored: 5,
                already_present: 0
            }
        );

        let a = src.read_stream(stream, 0, 10).await.unwrap();
        let b = dst.read_stream(stream, 0, 10).await.unwrap();
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(
                x.envelope, y.envelope,
                "every field preserved, origin included"
            );
        }
        assert_eq!(b.len(), evs.len());

        // Idempotent.
        let again = restore_chunks(&dst, &BACKUP_GENESIS, &[c1, c2])
            .await
            .unwrap();
        assert_eq!(
            again,
            RestoreReport {
                restored: 0,
                already_present: 5
            }
        );
    }

    #[tokio::test]
    async fn tampering_reordering_and_missing_chunks_are_all_rejected_before_any_write() {
        let (src, _, _) = source_with_messages(6).await;
        let c1 = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 2)
            .await
            .unwrap()
            .unwrap();
        let c2 = export_since(&src, c1.last_offset, c1.digest, 2)
            .await
            .unwrap()
            .unwrap();
        let c3 = export_since(&src, c2.last_offset, c2.digest, 2)
            .await
            .unwrap()
            .unwrap();

        // Altered payload.
        let mut bad = c2.clone();
        bad.events[0].envelope.payload.push(1);
        let dst = InMemoryEventStore::new();
        assert_eq!(
            restore_chunks(&dst, &BACKUP_GENESIS, &[c1.clone(), bad, c3.clone()])
                .await
                .unwrap_err(),
            RestoreError::Backup(BackupError::BadDigest)
        );
        // Missing middle chunk.
        assert_eq!(
            restore_chunks(&dst, &BACKUP_GENESIS, &[c1.clone(), c3.clone()])
                .await
                .unwrap_err(),
            RestoreError::Backup(BackupError::BrokenChain)
        );
        // Wrong starting point.
        assert_eq!(
            restore_chunks(&dst, &BACKUP_GENESIS, std::slice::from_ref(&c2))
                .await
                .unwrap_err(),
            RestoreError::Backup(BackupError::BrokenChain)
        );
        // Out-of-order events inside a chunk.
        let mut shuffled = c1.clone();
        shuffled.events.reverse();
        assert_eq!(shuffled.verify(None), Err(BackupError::BadOffsets));

        // Nothing was written by any failed attempt, including the
        // ones whose *first* chunks were individually valid.
        assert!(dst
            .read_log(LocalLogOffset(0), 100)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn restoring_into_a_diverged_target_stops_instead_of_interleaving() {
        let (src, stream, _) = source_with_messages(3).await;
        let c = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 10)
            .await
            .unwrap()
            .unwrap();

        let dst = InMemoryEventStore::new();
        dst.append(AppendRequest {
            stream_id: stream,
            expected_version: 0,
            events: vec![event(QUEUED, b"someone else's v1")],
        })
        .await
        .unwrap();
        assert_eq!(
            restore_chunks(&dst, &BACKUP_GENESIS, &[c])
                .await
                .unwrap_err(),
            RestoreError::TargetDiverged { version: 1 }
        );
    }

    #[tokio::test]
    async fn a_redacted_event_backs_up_and_restores_as_a_tombstone() {
        use crate::redaction::RedactableStore;
        let (src, stream, evs) = source_with_messages(2).await;
        src.redact_payloads(&[evs[0].event_id]).await.unwrap();
        let c = export_since(&src, LocalLogOffset(0), BACKUP_GENESIS, 10)
            .await
            .unwrap()
            .unwrap();
        assert!(c.events[0].envelope.payload.is_empty());
        let dst = InMemoryEventStore::new();
        restore_chunks(&dst, &BACKUP_GENESIS, &[c]).await.unwrap();
        let got = dst.read_stream(stream, 0, 10).await.unwrap();
        assert!(got[0].envelope.payload.is_empty());
        assert_eq!(got[1].envelope.payload, vec![1]);
    }

    /// Delivered-since-backup and expired messages, as a real guard
    /// would know them from state the log doesn't have.
    struct Guard {
        delivered: HashMap<EventId, ()>,
        expired_before: Timestamp,
        revoked_payloads: Vec<u8>,
    }
    impl RestoreGuard for Guard {
        fn assess(&self, e: &OutboxEntry, now: Timestamp) -> GuardVerdict {
            if self.delivered.contains_key(&e.source_event_id) {
                GuardVerdict::Drop {
                    reason: "already delivered",
                }
            } else if e.created_at < self.expired_before {
                GuardVerdict::Drop { reason: "expired" }
            } else if self.revoked_payloads.contains(&e.payload[0]) {
                GuardVerdict::Drop {
                    reason: "recipient revoked",
                }
            } else if e.payload[0] == 99 {
                GuardVerdict::Hold {
                    until: Timestamp(now.0 + 5_000),
                }
            } else {
                GuardVerdict::Proceed
            }
        }
    }

    #[tokio::test]
    async fn restored_pending_work_is_vetted_not_blindly_resent() {
        // Build history: m0 delivered (closed in log), m1 delivered
        // *after* the backup (log doesn't know), m2 expired, m3
        // recipient revoked, m4 held, m5 fine.
        let store = InMemoryEventStore::new();
        let flow = CommandFlow::new(&store, &MessagePlanner);
        let stream = StreamId::from_name("c");
        let mk = |p: u8, created: u64| {
            let mut e = event(QUEUED, &[p]);
            e.created_at = Timestamp(created);
            e
        };
        let evs = [
            mk(0, 100),
            mk(1, 100),
            mk(2, 1),
            mk(3, 100),
            mk(99, 100),
            mk(5, 100),
        ];
        for (i, e) in evs.iter().enumerate() {
            flow.commit(stream, i as u64, vec![e.clone()], Timestamp(e.created_at.0))
                .await
                .unwrap();
        }
        let mut sent = event(SENT, b"");
        sent.causation_id = Some(evs[0].event_id);
        store
            .append_with_outbox(
                AppendRequest {
                    stream_id: stream,
                    expected_version: 6,
                    events: vec![sent],
                },
                vec![],
            )
            .await
            .unwrap();

        // The restored node starts with an EMPTY outbox.
        let outbox = InMemoryOutbox::new();
        let guard = Guard {
            delivered: HashMap::from([(evs[1].event_id, ())]),
            expired_before: Timestamp(50),
            revoked_payloads: vec![3],
        };
        let now = Timestamp(1_000);
        let r = restore_outbox(&store, &outbox, &MessagePlanner, &guard, now)
            .await
            .unwrap();

        let id = |i: usize| OperationId::derive(evs[i].event_id, SEND_EFFECT, 0);
        assert_eq!(r.discharged_in_history, 1);
        assert_eq!(r.enqueued, vec![id(5)]);
        assert_eq!(r.held, vec![id(4)]);
        let dropped: HashMap<_, _> = r.dropped.iter().copied().collect();
        assert_eq!(dropped[&id(1)], "already delivered");
        assert_eq!(dropped[&id(2)], "expired");
        assert_eq!(dropped[&id(3)], "recipient revoked");
        assert!(
            !dropped.contains_key(&id(0)),
            "discharged in history: not even considered"
        );

        // What the outbox will actually run now: only m5, and m4 later.
        let due = outbox
            .claim_due(Timestamp(1_000), 10_000, 100)
            .await
            .unwrap();
        assert_eq!(
            due.iter().map(|e| e.operation_id).collect::<Vec<_>>(),
            vec![id(5)]
        );
        assert_eq!(due[0].payload, vec![5]);
        let later = outbox
            .claim_due(Timestamp(6_000), 10_000, 100)
            .await
            .unwrap();
        assert_eq!(
            later.iter().map(|e| e.operation_id).collect::<Vec<_>>(),
            vec![id(4)]
        );
        for i in [1, 2, 3] {
            let row = outbox.get(id(i)).await.unwrap().unwrap();
            assert_eq!(row.state, OutboxState::DeadLettered);
            assert!(row.payload.is_empty(), "dropped content is not retained");
        }

        // The decision survives a later ordinary reconciliation: the
        // dropped operations are present (as dead letters), so they are
        // NOT rebuilt.
        let again = crate::outbox_reconcile::reconcile_outbox(
            &store,
            &outbox,
            &MessagePlanner,
            crate::outbox_reconcile::RepairMode::Repair,
            now,
        )
        .await
        .unwrap();
        assert!(again.missing.is_empty(), "{again:?}");
        // And restoring twice changes nothing.
        let second = restore_outbox(&store, &outbox, &MessagePlanner, &guard, now)
            .await
            .unwrap();
        assert!(second.enqueued.is_empty() && second.held.is_empty() && second.dropped.is_empty());
        assert_eq!(second.already_present, 5);
    }

    #[tokio::test]
    async fn trust_everything_is_the_explicit_blind_resend() {
        let (src, _, evs) = source_with_messages(3).await;
        let outbox = InMemoryOutbox::new();
        let r = restore_outbox(
            &src,
            &outbox,
            &MessagePlanner,
            &TrustEverything,
            Timestamp(5),
        )
        .await
        .unwrap();
        assert_eq!(r.enqueued.len(), evs.len());
    }
}
