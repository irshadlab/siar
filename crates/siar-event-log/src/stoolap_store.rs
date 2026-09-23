//! §92 Phase 2: a real, durable [`EventStore`] backed by `stoolap` — §19's
//! recommended "SQLite-class" backend, using this workspace's own
//! pure-Rust embedded SQL engine rather than `rusqlite`/SQLite's C
//! library, per the root `Cargo.toml`'s explicit "storage: pure-Rust
//! embedded SQL ... replaces SQLite/rusqlite per explicit instruction".
//! `siar-storage` already depends on `stoolap` for an unrelated schema
//! (messages/outbox/contacts/groups, `plan.md`-era); this module depends
//! on `stoolap` directly instead of on `siar-storage`, on purpose — §1's
//! own requirement ("must remain reusable and independent of ... any one
//! database") reads the same way about being independent of any one
//! *application's* storage crate, and `siar-storage`'s schema has
//! nothing to do with `EventEnvelope`.
//!
//! ## Schema
//!
//! Follows §57 conceptually, adapted to what `stoolap` 0.4.0 actually
//! offers (confirmed by reading its own source, not assumed from
//! `siar-storage`'s doc comments alone — same finding either way):
//! `stoolap::Value` has no blob variant, so every binary/fixed-size id
//! column here is TEXT, not BLOB:
//!
//! - `event_id` / `correlation_id` / `causation_id`: the wrapped
//!   `Uuid`'s own string form (`Uuid::to_string`/`Uuid::parse_str`).
//! - `stream_id`: lower-hex of the `[u8; 32]` (not a `Uuid` — no
//!   existing string form to reuse, and hex is a plain enough encoding
//!   that it doesn't need a dependency of its own).
//! - `payload`: base64 (`base64`, already a real workspace dependency
//!   for exactly this in `siar-storage`).
//! - `origin`: a short tagged string (`"local:<uuid>"`,
//!   `"remote:<uuid>"`, `"imported"`, `"recovery"`, `"system"`) — the
//!   same denormalize-an-enum-into-text convention
//!   `siar-storage::message_repo`'s `delivery_state_to_str`/
//!   `str_to_delivery_state` already uses for `DeliveryState`.
//!
//! `stream_heads` is a materialized "current version per stream" table
//! — §11's own append transaction ends with "update stream head" as a
//! named step, and having it as a real row means §12's optimistic-
//! concurrency check is a single indexed point lookup, not a scan/count
//! over a stream's events on every append.
//!
//! ## §22 Integrity
//!
//! Every stored row also carries a `checksum`: a blake3 hash (`blake3`,
//! already a dependency for [`crate::ids::StreamId::from_name`]) over
//! the row's own identifying/content fields, verified on every read.
//! §22 calls this optional ("do not sign every trivial local event
//! unless there is a reason") — but it's cheap enough here, with the
//! hasher already a dependency for another reason, that skipping it
//! would be leaving §66's "local corruption" threat unhandled for free.
//! A mismatch surfaces as [`EventStoreError::Corrupt`], not a panic —
//! §69's "read-only recovery mode" is exactly the kind of caller
//! response a `Result` (not a panic) makes possible; that mode itself
//! is not implemented here.
//!
//! ## §11 Atomic append, and a deliberate extra guard
//!
//! `stoolap`'s own `BEGIN`/`COMMIT`/`ROLLBACK` (the same literal-SQL
//! transaction form `siar-storage::message_repo::insert_if_new` already
//! uses — confirmed against `stoolap` 0.4.0's own transaction API,
//! which also offers a `Transaction` object via `Database::begin()`;
//! this module sticks to the literal-SQL form already proven working
//! elsewhere in this workspace rather than introducing the second,
//! untested-here form) covers the check-version/insert-rows/update-head
//! sequence. On top of that, the whole critical section is additionally
//! serialized by a process-local [`std::sync::Mutex`], matching
//! [`crate::memory_store::InMemoryEventStore`]'s own single-lock
//! strategy: `siar-storage`'s own module doc flags `stoolap` as "days
//! old as a public release" with unproven concurrent-writer isolation,
//! so this module does not lean on `stoolap` alone for the read-then-
//! write race §12's guarantee depends on. A single global append lock
//! (not per-stream) is the conservative, explicitly-named trade-off —
//! harmless for this deployment model (one process, one `Database`
//! handle, matching `siar-storage::open`'s own `Arc<Database>` shared
//! across a process — appends aren't meant to be a high-throughput job
//! queue anyway, §15) and removable later once `stoolap`'s own
//! concurrency story has more mileage.
//!
//! ## What's still NOT here
//!
//! §92 Phase 2 itself, made real (SQLite/stoolap backend, stream
//! versioning, global local offset, unique event ids, batch append) —
//! not the phases after it. In particular: no snapshots (§38-39, Phase
//! 7), no outbox table (§14, Phase 5 — this module is a plain
//! [`EventStore`], not the transactional-outbox hybrid that section
//! describes), no projection/checkpoint machinery (§16-18, Phase 4), no
//! retention/compaction (§40), no migrations beyond `CREATE TABLE IF
//! NOT EXISTS` (§57's own schema has no version column yet — a real gap
//! for whoever adds the first breaking schema change), and no crash-
//! injection harness (§84) — only a same-process open/close/reopen
//! durability test (see this module's own tests), which exercises real
//! disk persistence but not an actual killed process.

use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use stoolap::Database;

use crate::envelope::{EventEnvelope, EventOrigin};
use crate::ids::{CorrelationId, EventId, EventTypeId, LocalLogOffset, StreamId, Timestamp};
use crate::store::{
    validate_payload_size, AppendRequest, AppendResult, EventStore, EventStoreError, StoredEvent,
};
use async_trait::async_trait;
use siar_domain::DeviceId;
use uuid::Uuid;

/// A real, durable [`EventStore`]. See this module's own doc comment
/// for the schema and the concurrency strategy.
pub struct StoolapEventStore {
    db: Database,
    // §11's extra process-local guard — see the module doc's "Atomic
    // append" section. Guards nothing by itself (`stoolap::Database`'s
    // own methods take `&self`); it exists purely to serialize the
    // multi-statement check-then-write critical section in
    // [`Self::append`].
    append_lock: Mutex<()>,
}

impl StoolapEventStore {
    /// Opens (or creates) a file-backed store at `path` and applies the
    /// schema. Mirrors `siar-storage::open`'s own DSN handling — a real
    /// bug that crate's own doc comment names (`Database::open` needs a
    /// `"file://"`-prefixed DSN, not a bare path) is avoided here by
    /// applying the same prefix from the start.
    pub fn open(path: &str) -> Result<Self, EventStoreError> {
        let dsn = format!("file://{path}");
        let db = Database::open(&dsn).map_err(|e| EventStoreError::Backend(e.to_string()))?;
        Self::from_database(db)
    }

    /// In-memory store — for tests, and for any caller that wants this
    /// trait's real transactional/idempotency behavior without a file
    /// on disk (mirrors `siar-storage::open_in_memory`).
    pub fn open_in_memory() -> Result<Self, EventStoreError> {
        let db = Database::open_in_memory().map_err(|e| EventStoreError::Backend(e.to_string()))?;
        Self::from_database(db)
    }

    fn from_database(db: Database) -> Result<Self, EventStoreError> {
        apply_schema(&db)?;
        Ok(Self {
            db,
            append_lock: Mutex::new(()),
        })
    }
}

fn apply_schema(db: &Database) -> Result<(), EventStoreError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS events (
            local_offset    INTEGER NOT NULL,
            event_id        TEXT NOT NULL,
            stream_id       TEXT NOT NULL,
            stream_version  INTEGER NOT NULL,
            event_type      INTEGER NOT NULL,
            schema_version  INTEGER NOT NULL,
            created_at      INTEGER NOT NULL,
            origin          TEXT NOT NULL,
            correlation_id  TEXT,
            causation_id    TEXT,
            payload         TEXT NOT NULL,
            checksum        TEXT NOT NULL
        )",
        (),
    )
    .map_err(backend_err)?;

    // §58 "Indexes": event_id, (stream_id, stream_version), local_offset
    // — the spec's own stated minimum, no more.
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_events_local_offset ON events (local_offset)",
        (),
    )
    .map_err(backend_err)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_events_event_id ON events (event_id)",
        (),
    )
    .map_err(backend_err)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_events_stream_version
         ON events (stream_id, stream_version)",
        (),
    )
    .map_err(backend_err)?;

    db.execute(
        "CREATE TABLE IF NOT EXISTS stream_heads (
            stream_id       TEXT NOT NULL,
            current_version INTEGER NOT NULL
        )",
        (),
    )
    .map_err(backend_err)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_stream_heads_id ON stream_heads (stream_id)",
        (),
    )
    .map_err(backend_err)?;

    Ok(())
}

fn backend_err(e: impl std::fmt::Display) -> EventStoreError {
    EventStoreError::Backend(e.to_string())
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Zero-dependency hex — 32 bytes, once per event, doesn't
        // justify pulling in a `hex` crate for what `write!` already
        // does.
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

fn hex_decode(s: &str) -> Result<[u8; 32], EventStoreError> {
    if s.len() != 64 {
        return Err(EventStoreError::Corrupt(format!(
            "stream_id hex had length {}, expected 64",
            s.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hex_pair = std::str::from_utf8(chunk)
            .map_err(|_| EventStoreError::Corrupt("stream_id hex was not UTF-8".to_string()))?;
        out[i] = u8::from_str_radix(hex_pair, 16)
            .map_err(|_| EventStoreError::Corrupt(format!("invalid hex byte '{hex_pair}'")))?;
    }
    Ok(out)
}

fn encode_origin(origin: &EventOrigin) -> String {
    match origin {
        EventOrigin::LocalDevice(id) => format!("local:{}", id.as_uuid()),
        EventOrigin::RemoteDevice(id) => format!("remote:{}", id.as_uuid()),
        EventOrigin::Imported => "imported".to_string(),
        EventOrigin::Recovery => "recovery".to_string(),
        EventOrigin::System => "system".to_string(),
    }
}

fn decode_origin(s: &str) -> Result<EventOrigin, EventStoreError> {
    if let Some(rest) = s.strip_prefix("local:") {
        return Ok(EventOrigin::LocalDevice(DeviceId::from_uuid(parse_uuid(
            rest,
        )?)));
    }
    if let Some(rest) = s.strip_prefix("remote:") {
        return Ok(EventOrigin::RemoteDevice(DeviceId::from_uuid(parse_uuid(
            rest,
        )?)));
    }
    match s {
        "imported" => Ok(EventOrigin::Imported),
        "recovery" => Ok(EventOrigin::Recovery),
        "system" => Ok(EventOrigin::System),
        other => Err(EventStoreError::Corrupt(format!(
            "unknown event origin '{other}'"
        ))),
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, EventStoreError> {
    Uuid::parse_str(s).map_err(|e| EventStoreError::Corrupt(format!("malformed uuid: {e}")))
}

/// §22: one hash over everything that identifies and gives meaning to a
/// row, computed the same way at append time and re-derived at read
/// time. Deliberately excludes `local_offset` (assigned by the store,
/// not semantic content) — everything else is exactly what §4's
/// `EventEnvelope` says an event *is*.
#[allow(clippy::too_many_arguments)]
fn compute_checksum(
    event_id: &str,
    stream_id: &str,
    stream_version: u64,
    event_type: u32,
    schema_version: u16,
    created_at: u64,
    origin: &str,
    correlation_id: Option<&str>,
    causation_id: Option<&str>,
    payload_b64: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(event_id.as_bytes());
    hasher.update(stream_id.as_bytes());
    hasher.update(&stream_version.to_le_bytes());
    hasher.update(&event_type.to_le_bytes());
    hasher.update(&schema_version.to_le_bytes());
    hasher.update(&created_at.to_le_bytes());
    hasher.update(origin.as_bytes());
    hasher.update(correlation_id.unwrap_or("").as_bytes());
    hasher.update(causation_id.unwrap_or("").as_bytes());
    hasher.update(payload_b64.as_bytes());
    hex_encode(hasher.finalize().as_bytes())
}

fn row_to_stored_event(row: &stoolap::ResultRow) -> Result<StoredEvent, EventStoreError> {
    let local_offset: i64 = row.get(0).map_err(backend_err)?;
    let event_id: String = row.get(1).map_err(backend_err)?;
    let stream_id: String = row.get(2).map_err(backend_err)?;
    let stream_version: i64 = row.get(3).map_err(backend_err)?;
    let event_type: i64 = row.get(4).map_err(backend_err)?;
    let schema_version: i64 = row.get(5).map_err(backend_err)?;
    let created_at: i64 = row.get(6).map_err(backend_err)?;
    let origin: String = row.get(7).map_err(backend_err)?;
    let correlation_id: Option<String> = row.get(8).map_err(backend_err)?;
    let causation_id: Option<String> = row.get(9).map_err(backend_err)?;
    let payload_b64: String = row.get(10).map_err(backend_err)?;
    let checksum: String = row.get(11).map_err(backend_err)?;

    let expected_checksum = compute_checksum(
        &event_id,
        &stream_id,
        stream_version as u64,
        event_type as u32,
        schema_version as u16,
        created_at as u64,
        &origin,
        correlation_id.as_deref(),
        causation_id.as_deref(),
        &payload_b64,
    );
    if expected_checksum != checksum {
        return Err(EventStoreError::Corrupt(format!(
            "checksum mismatch for event_id {event_id}"
        )));
    }

    let payload = BASE64
        .decode(&payload_b64)
        .map_err(|e| EventStoreError::Corrupt(format!("payload was not valid base64: {e}")))?;

    let envelope = EventEnvelope {
        event_id: EventId::from_uuid(parse_uuid(&event_id)?),
        stream_id: StreamId::from_bytes(hex_decode(&stream_id)?),
        stream_version: stream_version as u64,
        event_type: EventTypeId(event_type as u32),
        schema_version: schema_version as u16,
        created_at: Timestamp(created_at as u64),
        origin: decode_origin(&origin)?,
        correlation_id: correlation_id
            .map(|s| parse_uuid(&s).map(CorrelationId::from_uuid))
            .transpose()?,
        causation_id: causation_id
            .map(|s| parse_uuid(&s).map(EventId::from_uuid))
            .transpose()?,
        payload,
    };

    Ok(StoredEvent {
        envelope,
        local_offset: LocalLogOffset(local_offset as u64),
    })
}

#[async_trait]
impl EventStore for StoolapEventStore {
    /// §11/§12/§21/§24 — see the module doc's "Atomic append" section
    /// for why the `Mutex` is here alongside `stoolap`'s own
    /// transaction.
    async fn append(&self, request: AppendRequest) -> Result<AppendResult, EventStoreError> {
        // §55: checked before acquiring the lock or opening a
        // transaction at all — no reason to touch the database for a
        // batch that's going to be rejected regardless.
        for new_event in &request.events {
            validate_payload_size(new_event.event_type, new_event.payload.len())?;
        }

        let _guard = self
            .append_lock
            .lock()
            .expect("StoolapEventStore append lock poisoned");

        let stream_id_hex = hex_encode(request.stream_id.as_bytes());

        self.db.execute("BEGIN", ()).map_err(backend_err)?;

        let run = || -> Result<AppendResult, EventStoreError> {
            let current_version: Option<i64> = {
                let mut rows = self
                    .db
                    .query(
                        "SELECT current_version FROM stream_heads WHERE stream_id = $1",
                        (stream_id_hex.clone(),),
                    )
                    .map_err(backend_err)?;
                match rows.next() {
                    Some(row) => Some(row.map_err(backend_err)?.get(0).map_err(backend_err)?),
                    None => None,
                }
            };
            let current_version = current_version.unwrap_or(0) as u64;

            if request.expected_version != current_version {
                return Err(EventStoreError::ConcurrencyConflict {
                    stream_id: request.stream_id,
                    expected_version: request.expected_version,
                    actual_version: current_version,
                });
            }

            let mut next_offset: i64 = {
                let mut rows = self
                    .db
                    .query("SELECT COALESCE(MAX(local_offset), 0) FROM events", ())
                    .map_err(backend_err)?;
                match rows.next() {
                    Some(row) => row
                        .map_err(backend_err)?
                        .get::<i64>(0)
                        .map_err(backend_err)?,
                    None => 0,
                }
            };

            let mut new_version = current_version;
            let mut local_offsets = Vec::with_capacity(request.events.len());

            for new_event in request.events {
                let event_id_str = new_event.event_id.as_uuid().to_string();

                // §24: idempotent no-op on a previously seen event_id.
                let already_seen = {
                    let mut rows = self
                        .db
                        .query(
                            "SELECT 1 FROM events WHERE event_id = $1",
                            (event_id_str.clone(),),
                        )
                        .map_err(backend_err)?;
                    rows.next().is_some()
                };
                if already_seen {
                    local_offsets.push(None);
                    continue;
                }

                new_version += 1;
                next_offset += 1;

                let origin_str = encode_origin(&new_event.origin);
                let correlation_id_str = new_event.correlation_id.map(|c| c.as_uuid().to_string());
                let causation_id_str = new_event.causation_id.map(|c| c.as_uuid().to_string());
                let payload_b64 = BASE64.encode(&new_event.payload);
                let checksum = compute_checksum(
                    &event_id_str,
                    &stream_id_hex,
                    new_version,
                    new_event.event_type.0,
                    new_event.schema_version,
                    new_event.created_at.0,
                    &origin_str,
                    correlation_id_str.as_deref(),
                    causation_id_str.as_deref(),
                    &payload_b64,
                );

                self.db
                    .execute(
                        "INSERT INTO events
                            (local_offset, event_id, stream_id, stream_version, event_type,
                             schema_version, created_at, origin, correlation_id, causation_id,
                             payload, checksum)
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                        (
                            next_offset,
                            event_id_str,
                            stream_id_hex.clone(),
                            new_version as i64,
                            new_event.event_type.0 as i64,
                            new_event.schema_version as i64,
                            new_event.created_at.0 as i64,
                            origin_str,
                            correlation_id_str,
                            causation_id_str,
                            payload_b64,
                            checksum,
                        ),
                    )
                    .map_err(backend_err)?;

                local_offsets.push(Some(LocalLogOffset(next_offset as u64)));
            }

            if new_version != current_version {
                self.db
                    .execute(
                        "DELETE FROM stream_heads WHERE stream_id = $1",
                        (stream_id_hex.clone(),),
                    )
                    .map_err(backend_err)?;
                self.db
                    .execute(
                        "INSERT INTO stream_heads (stream_id, current_version) VALUES ($1, $2)",
                        (stream_id_hex.clone(), new_version as i64),
                    )
                    .map_err(backend_err)?;
            }

            Ok(AppendResult {
                stream_id: request.stream_id,
                new_version,
                local_offsets,
            })
        };

        match run() {
            Ok(result) => {
                self.db.execute("COMMIT", ()).map_err(backend_err)?;
                Ok(result)
            }
            Err(e) => {
                // Best-effort rollback: §11's "failure means no partial
                // logical event" — if COMMIT is never reached, none of
                // this transaction's writes should be visible.
                let _ = self.db.execute("ROLLBACK", ());
                Err(e)
            }
        }
    }

    async fn read_stream(
        &self,
        stream: StreamId,
        from_version: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let stream_id_hex = hex_encode(stream.as_bytes());
        let rows = self
            .db
            .query(
                "SELECT local_offset, event_id, stream_id, stream_version, event_type,
                        schema_version, created_at, origin, correlation_id, causation_id,
                        payload, checksum
                 FROM events
                 WHERE stream_id = $1 AND stream_version > $2
                 ORDER BY stream_version ASC
                 LIMIT $3",
                (stream_id_hex, from_version as i64, limit as i64),
            )
            .map_err(backend_err)?;

        let mut out = Vec::with_capacity(limit);
        for row in rows {
            out.push(row_to_stored_event(&row.map_err(backend_err)?)?);
        }
        Ok(out)
    }

    async fn read_log(
        &self,
        from_offset: LocalLogOffset,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let rows = self
            .db
            .query(
                "SELECT local_offset, event_id, stream_id, stream_version, event_type,
                        schema_version, created_at, origin, correlation_id, causation_id,
                        payload, checksum
                 FROM events
                 WHERE local_offset > $1
                 ORDER BY local_offset ASC
                 LIMIT $2",
                (from_offset.0 as i64, limit as i64),
            )
            .map_err(backend_err)?;

        let mut out = Vec::with_capacity(limit);
        for row in rows {
            out.push(row_to_stored_event(&row.map_err(backend_err)?)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::EventTypeId;
    use crate::store::NewEvent;

    fn new_event(event_id: EventId) -> NewEvent {
        NewEvent {
            event_id,
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp::now(),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: vec![1, 2, 3],
        }
    }

    #[tokio::test]
    async fn appending_to_a_new_stream_at_version_zero_succeeds() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();
        assert_eq!(result.new_version, 1);
        assert!(result.local_offsets[0].is_some());
    }

    #[tokio::test]
    async fn a_stale_expected_version_is_a_real_concurrency_conflict() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();

        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await;
        assert_eq!(
            result,
            Err(EventStoreError::ConcurrencyConflict {
                stream_id: stream,
                expected_version: 0,
                actual_version: 1,
            })
        );

        // §11: a rejected append must leave no partial trace.
        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn appending_a_previously_seen_event_id_is_an_idempotent_no_op() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        let event_id = EventId::new();

        let first = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(event_id)],
            })
            .await
            .unwrap();
        assert_eq!(first.new_version, 1);

        let second = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 1,
                events: vec![new_event(event_id)],
            })
            .await
            .unwrap();
        assert_eq!(second.new_version, 1);
        assert_eq!(second.local_offsets, vec![None]);

        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn a_batch_append_is_all_or_nothing_on_concurrency_conflict() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 5,
                events: vec![new_event(EventId::new()), new_event(EventId::new())],
            })
            .await;
        assert!(result.is_err());
        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn read_stream_only_returns_events_after_from_version() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![
                    new_event(EventId::new()),
                    new_event(EventId::new()),
                    new_event(EventId::new()),
                ],
            })
            .await
            .unwrap();

        let events = store.read_stream(stream, 1, 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].envelope.stream_version, 2);
    }

    #[tokio::test]
    async fn read_log_spans_multiple_streams_in_append_order() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream_a = StreamId::from_name("conversation/a");
        let stream_b = StreamId::from_name("conversation/b");
        store
            .append(AppendRequest {
                stream_id: stream_a,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();
        store
            .append(AppendRequest {
                stream_id: stream_b,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();

        let events = store.read_log(LocalLogOffset(0), 10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].envelope.stream_id, stream_a);
        assert_eq!(events[1].envelope.stream_id, stream_b);
    }

    /// §93's own Definition-of-Done line: "accepted local commands
    /// survive process death." Not a real killed process (§84's crash
    /// injection is explicitly out of scope — see the module doc), but
    /// a real file on disk, closed and reopened as a fresh `Database`
    /// handle — the actual persistence mechanism this test is checking
    /// is exercised for real, not simulated in memory.
    #[tokio::test]
    async fn events_survive_closing_and_reopening_the_same_file() {
        let path = std::env::temp_dir().join(format!(
            "siar-event-log-durability-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path_str = path.to_str().unwrap().to_string();

        let stream = StreamId::from_name("conversation/durable");
        let event_id = EventId::new();
        {
            let store = StoolapEventStore::open(&path_str).unwrap();
            store
                .append(AppendRequest {
                    stream_id: stream,
                    expected_version: 0,
                    events: vec![new_event(event_id)],
                })
                .await
                .unwrap();
            // `store` (and its `Database` handle) drops here — the
            // real "process death" boundary this test can exercise.
        }

        {
            let store = StoolapEventStore::open(&path_str).unwrap();
            let events = store.read_stream(stream, 0, 10).await.unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].envelope.event_id, event_id);
            assert_eq!(events[0].envelope.payload, vec![1, 2, 3]);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_tampered_checksum_is_detected_as_corruption() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![new_event(EventId::new())],
            })
            .await
            .unwrap();

        store
            .db
            .execute(
                "UPDATE events SET payload = $1",
                ("dGFtcGVyZWQ=".to_string(),),
            )
            .unwrap();

        let result = store.read_stream(stream, 0, 10).await;
        assert!(matches!(result, Err(EventStoreError::Corrupt(_))));
    }

    #[tokio::test]
    async fn an_oversized_payload_is_rejected_without_touching_the_database() {
        let store = StoolapEventStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("conversation/abc");
        let mut oversized = new_event(EventId::new());
        oversized.payload = vec![0u8; crate::store::DEFAULT_MAX_EVENT_PAYLOAD_BYTES + 1];

        let result = store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![oversized],
            })
            .await;
        assert!(matches!(
            result,
            Err(EventStoreError::PayloadTooLarge { .. })
        ));

        let events = store.read_stream(stream, 0, 10).await.unwrap();
        assert!(events.is_empty());
    }
}
