//! §14's durable outbox: "if the process dies, the outbox still
//! exists." The `stoolap`-backed counterpart to
//! [`crate::outbox::InMemoryOutbox`], following the same pattern
//! [`crate::stoolap_checkpoint_store`] set for checkpoints — and, unlike
//! that module, built to share its [`crate::stoolap_store::
//! StoolapEventStore`]'s database, so §14's "one transaction may commit
//! the event *and* the outbox row" is real on disk, not only in memory.
//!
//! ## Schema (§57/§58)
//!
//! One `outbox` table, one row per [`crate::outbox::OperationId`], with
//! `operation_id` unique (idempotent enqueue) and a `(state, not_before)`
//! index — the access path §15 asks for: finding due work without
//! scanning history. Binary values use the same encodings as
//! `stoolap_store` (hex ids, base64 payload — no blob column type is
//! used anywhere in this crate).
//!
//! Unlike `stoolap_checkpoint_store`, this module uses `UPDATE`: the
//! outbox is mutated far more often than any other table here (every
//! claim, every completion), so `DELETE`+`INSERT` would double the write
//! amplification for no benefit. `UPDATE ... WHERE`, `ORDER BY ...
//! LIMIT`, `GROUP BY`, nullable parameters and `ROLLBACK` are exercised
//! against `stoolap` 0.4.2 by this module's tests, including a
//! behavioural parity test against [`crate::outbox::OutboxTable`].
//!
//! ## Concurrency
//!
//! `stoolap`'s `Database::clone` shares storage but gives each handle
//! its *own* transaction state. The outbox therefore holds a clone of
//! the event store's database **and the same process-local write lock**
//! (`Arc<Mutex<()>>`) the event store already serializes appends with:
//! an outbox write and an event append never overlap, so neither can
//! observe or interleave with the other's open transaction. This is the
//! same conservative single-writer posture `stoolap_store`'s own module
//! doc explains and accepts for this deployment model.

use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use stoolap::Database;

use crate::ids::{EventId, StreamId, Timestamp};
use crate::outbox::{
    truncate_error, EffectKind, EnqueueOutcome, OperationId, Outbox, OutboxCounts, OutboxEntry,
    OutboxError, OutboxState,
};

const COLUMNS: &str = "operation_id, kind, source_event_id, stream_id, payload, state, attempts, \
                       not_before, lease_until, created_at, updated_at, last_error";

fn be(e: impl std::fmt::Display) -> OutboxError {
    OutboxError::Backend(e.to_string())
}

/// Idempotent; also called by the event store's own schema setup so the
/// table always exists wherever an event store does.
pub(crate) fn apply_outbox_schema(db: &Database) -> Result<(), OutboxError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS outbox (
            operation_id    TEXT NOT NULL,
            kind            INTEGER NOT NULL,
            source_event_id TEXT NOT NULL,
            stream_id       TEXT NOT NULL,
            payload         TEXT NOT NULL,
            state           INTEGER NOT NULL,
            attempts        INTEGER NOT NULL,
            not_before      INTEGER NOT NULL,
            lease_until     INTEGER,
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL,
            last_error      TEXT
        )",
        (),
    )
    .map_err(be)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_outbox_operation ON outbox (operation_id)",
        (),
    )
    .map_err(be)?;
    db.execute(
        "CREATE INDEX IF NOT EXISTS idx_outbox_due ON outbox (state, not_before)",
        (),
    )
    .map_err(be)?;
    Ok(())
}

/// Insert unless the id exists. Runs inside whatever transaction the
/// caller has open — this is what the event store calls mid-append.
pub(crate) fn insert_if_absent(
    db: &Database,
    entry: &OutboxEntry,
) -> Result<EnqueueOutcome, OutboxError> {
    let id = entry.operation_id.to_hex();
    let exists = {
        let mut rows = db
            .query(
                "SELECT 1 FROM outbox WHERE operation_id = $1",
                (id.clone(),),
            )
            .map_err(be)?;
        rows.next().is_some()
    };
    if exists {
        return Ok(EnqueueOutcome::AlreadyPresent);
    }
    db.execute(
        "INSERT INTO outbox
            (operation_id, kind, source_event_id, stream_id, payload, state, attempts,
             not_before, lease_until, created_at, updated_at, last_error)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        (
            id,
            i64::from(entry.kind.0),
            entry.source_event_id.as_uuid().to_string(),
            hex32(entry.stream_id.as_bytes()),
            BASE64.encode(&entry.payload),
            entry.state.code(),
            i64::from(entry.attempts),
            entry.not_before.0 as i64,
            entry.lease_until.map(|t| t.0 as i64),
            entry.created_at.0 as i64,
            entry.updated_at.0 as i64,
            entry.last_error.clone(),
        ),
    )
    .map_err(be)?;
    Ok(EnqueueOutcome::Inserted)
}

fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn unhex32(s: &str) -> Result<[u8; 32], OutboxError> {
    if s.len() != 64 {
        return Err(be("stored stream_id has wrong length"));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(be)?;
    }
    Ok(out)
}

fn row_to_entry(row: &stoolap::ResultRow) -> Result<OutboxEntry, OutboxError> {
    let operation_id: String = row.get(0).map_err(be)?;
    let kind: i64 = row.get(1).map_err(be)?;
    let source: String = row.get(2).map_err(be)?;
    let stream: String = row.get(3).map_err(be)?;
    let payload: String = row.get(4).map_err(be)?;
    let state: i64 = row.get(5).map_err(be)?;
    let attempts: i64 = row.get(6).map_err(be)?;
    let not_before: i64 = row.get(7).map_err(be)?;
    let lease_until: Option<i64> = row.get(8).map_err(be)?;
    let created_at: i64 = row.get(9).map_err(be)?;
    let updated_at: i64 = row.get(10).map_err(be)?;
    let last_error: Option<String> = row.get(11).map_err(be)?;

    Ok(OutboxEntry {
        operation_id: OperationId::from_hex(&operation_id)
            .ok_or_else(|| be("stored operation_id is not valid hex"))?,
        kind: EffectKind(kind as u32),
        source_event_id: EventId::from_uuid(source.parse().map_err(be)?),
        stream_id: StreamId::from_bytes(unhex32(&stream)?),
        payload: BASE64.decode(payload).map_err(be)?,
        state: OutboxState::from_code(state)
            .ok_or_else(|| be(format!("stored outbox state {state} is unknown")))?,
        attempts: attempts as u32,
        not_before: Timestamp(not_before as u64),
        lease_until: lease_until.map(|t| Timestamp(t as u64)),
        created_at: Timestamp(created_at as u64),
        updated_at: Timestamp(updated_at as u64),
        last_error,
    })
}

fn clamp_limit(limit: usize) -> i64 {
    limit.min(i64::MAX as usize) as i64
}

/// See the module doc. Obtain one sharing an event store's database via
/// [`crate::stoolap_store::StoolapEventStore::outbox`], or stand one up
/// on its own with [`Self::open`]/[`Self::open_in_memory`].
pub struct StoolapOutbox {
    db: Database,
    lock: Arc<Mutex<()>>,
}

impl StoolapOutbox {
    pub fn open(path: &str) -> Result<Self, OutboxError> {
        let db = Database::open(&format!("file://{path}")).map_err(be)?;
        Self::from_database(db, Arc::new(Mutex::new(())))
    }

    pub fn open_in_memory() -> Result<Self, OutboxError> {
        let db = Database::open_in_memory().map_err(be)?;
        Self::from_database(db, Arc::new(Mutex::new(())))
    }

    pub(crate) fn from_database(db: Database, lock: Arc<Mutex<()>>) -> Result<Self, OutboxError> {
        apply_outbox_schema(&db)?;
        Ok(Self { db, lock })
    }

    /// Runs `f` as one transaction under the shared write lock.
    fn txn<T>(
        &self,
        f: impl FnOnce(&Database) -> Result<T, OutboxError>,
    ) -> Result<T, OutboxError> {
        let _guard = self.lock.lock().expect("outbox write lock poisoned");
        self.db.execute("BEGIN", ()).map_err(be)?;
        match f(&self.db) {
            Ok(v) => {
                self.db.execute("COMMIT", ()).map_err(be)?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.db.execute("ROLLBACK", ());
                Err(e)
            }
        }
    }

    fn fetch(db: &Database, id: OperationId) -> Result<Option<OutboxEntry>, OutboxError> {
        let mut rows = db
            .query(
                &format!("SELECT {COLUMNS} FROM outbox WHERE operation_id = $1"),
                (id.to_hex(),),
            )
            .map_err(be)?;
        match rows.next() {
            Some(row) => Ok(Some(row_to_entry(&row.map_err(be)?)?)),
            None => Ok(None),
        }
    }

    fn collect(
        db: &Database,
        sql: &str,
        p: impl stoolap::Params,
    ) -> Result<Vec<OutboxEntry>, OutboxError> {
        let mut out = Vec::new();
        for row in db.query(sql, p).map_err(be)? {
            out.push(row_to_entry(&row.map_err(be)?)?);
        }
        Ok(out)
    }
}

impl Outbox for StoolapOutbox {
    async fn enqueue(&self, entry: OutboxEntry) -> Result<EnqueueOutcome, OutboxError> {
        crate::outbox::validate_entry(&entry)?;
        self.txn(|db| insert_if_absent(db, &entry))
    }

    async fn claim_due(
        &self,
        now: Timestamp,
        lease_ms: u64,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, OutboxError> {
        let lease_until = Timestamp(now.0.saturating_add(lease_ms));
        self.txn(|db| {
            let mut due = Self::collect(
                db,
                &format!(
                    "SELECT {COLUMNS} FROM outbox
                     WHERE (state = 0 AND not_before <= $1)
                        OR (state = 1 AND (lease_until IS NULL OR lease_until <= $1))
                     ORDER BY not_before ASC, created_at ASC, operation_id ASC
                     LIMIT $2"
                ),
                (now.0 as i64, clamp_limit(limit)),
            )?;
            for e in &mut due {
                db.execute(
                    "UPDATE outbox SET state = 1, attempts = attempts + 1,
                            lease_until = $1, updated_at = $2
                     WHERE operation_id = $3",
                    (lease_until.0 as i64, now.0 as i64, e.operation_id.to_hex()),
                )
                .map_err(be)?;
                e.state = OutboxState::InFlight;
                e.attempts = e.attempts.saturating_add(1);
                e.lease_until = Some(lease_until);
                e.updated_at = now;
            }
            Ok(due)
        })
    }

    async fn complete(&self, id: OperationId, now: Timestamp) -> Result<(), OutboxError> {
        self.txn(|db| {
            let e = Self::fetch(db, id)?.ok_or(OutboxError::NotFound(id))?;
            match e.state {
                OutboxState::Done => Ok(()),
                OutboxState::DeadLettered => Err(OutboxError::InvalidTransition {
                    id,
                    from: e.state,
                    to: OutboxState::Done,
                }),
                OutboxState::Pending | OutboxState::InFlight => {
                    db.execute(
                        "UPDATE outbox SET state = 2, lease_until = NULL, updated_at = $1
                         WHERE operation_id = $2",
                        (now.0 as i64, id.to_hex()),
                    )
                    .map_err(be)?;
                    Ok(())
                }
            }
        })
    }

    async fn reschedule(
        &self,
        id: OperationId,
        not_before: Timestamp,
        error: Option<String>,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        self.txn(|db| {
            let e = Self::fetch(db, id)?.ok_or(OutboxError::NotFound(id))?;
            if e.state != OutboxState::InFlight {
                return Err(OutboxError::InvalidTransition {
                    id,
                    from: e.state,
                    to: OutboxState::Pending,
                });
            }
            db.execute(
                "UPDATE outbox SET state = 0, not_before = $1, lease_until = NULL,
                        last_error = $2, updated_at = $3
                 WHERE operation_id = $4",
                (
                    not_before.0 as i64,
                    error.map(truncate_error),
                    now.0 as i64,
                    id.to_hex(),
                ),
            )
            .map_err(be)?;
            Ok(())
        })
    }

    async fn dead_letter(
        &self,
        id: OperationId,
        reason: String,
        now: Timestamp,
    ) -> Result<(), OutboxError> {
        self.txn(|db| {
            let e = Self::fetch(db, id)?.ok_or(OutboxError::NotFound(id))?;
            match e.state {
                OutboxState::DeadLettered => Ok(()),
                OutboxState::Done => Err(OutboxError::InvalidTransition {
                    id,
                    from: e.state,
                    to: OutboxState::DeadLettered,
                }),
                OutboxState::Pending | OutboxState::InFlight => {
                    db.execute(
                        "UPDATE outbox SET state = 3, lease_until = NULL,
                                last_error = $1, updated_at = $2
                         WHERE operation_id = $3",
                        (truncate_error(reason), now.0 as i64, id.to_hex()),
                    )
                    .map_err(be)?;
                    Ok(())
                }
            }
        })
    }

    async fn get(&self, id: OperationId) -> Result<Option<OutboxEntry>, OutboxError> {
        let _guard = self.lock.lock().expect("outbox write lock poisoned");
        Self::fetch(&self.db, id)
    }

    async fn list_open(&self, limit: usize) -> Result<Vec<OutboxEntry>, OutboxError> {
        let _guard = self.lock.lock().expect("outbox write lock poisoned");
        Self::collect(
            &self.db,
            &format!(
                "SELECT {COLUMNS} FROM outbox WHERE state IN (0, 1)
                 ORDER BY created_at ASC, operation_id ASC LIMIT $1"
            ),
            (clamp_limit(limit),),
        )
    }

    async fn release_all_in_flight(&self, now: Timestamp) -> Result<u64, OutboxError> {
        self.txn(|db| {
            let n = db
                .execute(
                    "UPDATE outbox SET state = 0, lease_until = NULL, updated_at = $1
                     WHERE state = 1",
                    (now.0 as i64,),
                )
                .map_err(be)?;
            Ok(n as u64)
        })
    }

    async fn purge_done(&self, before: Timestamp) -> Result<u64, OutboxError> {
        self.txn(|db| {
            let n = db
                .execute(
                    "DELETE FROM outbox WHERE state = 2 AND updated_at < $1",
                    (before.0 as i64,),
                )
                .map_err(be)?;
            Ok(n as u64)
        })
    }

    async fn counts(&self) -> Result<OutboxCounts, OutboxError> {
        let _guard = self.lock.lock().expect("outbox write lock poisoned");
        let mut c = OutboxCounts::default();
        for row in self
            .db
            .query("SELECT state, COUNT(*) FROM outbox GROUP BY state", ())
            .map_err(be)?
        {
            let row = row.map_err(be)?;
            let state: i64 = row.get(0).map_err(be)?;
            let n: i64 = row.get(1).map_err(be)?;
            match OutboxState::from_code(state) {
                Some(OutboxState::Pending) => c.pending = n as u64,
                Some(OutboxState::InFlight) => c.in_flight = n as u64,
                Some(OutboxState::Done) => c.done = n as u64,
                Some(OutboxState::DeadLettered) => c.dead_lettered = n as u64,
                None => return Err(be(format!("stored outbox state {state} is unknown"))),
            }
        }
        Ok(c)
    }
}
