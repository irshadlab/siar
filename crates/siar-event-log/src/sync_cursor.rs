//! `04-offline-event-log-architecture.md` §53 "Sync Cursors" — §92
//! Phase 6's "stream heads / sync hooks" bookkeeping.
//!
//! ```text
//! SyncCursor { peer: DeviceId, stream: StreamId, acknowledged_version: u64 }
//! ```
//!
//! "Where stream ordering supports it": a cursor is only meaningful for
//! a stream with one writer, where version *n* means the same event on
//! both sides (see [`crate::replication`]'s §54 note). "For more complex
//! sparse state, later add Merkle/set reconciliation" — not here.
//!
//! This is **not** `siar_domain::SyncCursor` (`device` + `last_sequence`):
//! that is a plan.md-era per-device message-sequence marker with no
//! stream dimension, kept in `siar-domain` for its own callers. The two
//! share a name and nothing else.
//!
//! ## Semantics
//!
//! - **Monotonic.** [`CursorStore::acknowledge`] never moves a cursor
//!   backwards: a late or duplicated acknowledgement is harmless.
//! - **Acknowledged, not sent.** A cursor advances only on the peer's
//!   acknowledgement of durable receipt (§23's "ACK only after
//!   persistence"), never when a batch is merely transmitted. A crash
//!   between "sent" and "acked" re-sends, which the receiver absorbs
//!   through §24 idempotency.
//! - **Reset is explicit.** A peer restored from backup is *behind* its
//!   recorded cursor; [`CursorStore::reset`] is the one deliberate way to
//!   move backwards, so it can't happen by accident through an ack.
//!
//! Native `async fn`-in-trait (`-> impl Future + Send`), same as
//! [`crate::outbox::Outbox`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use siar_domain::DeviceId;
use stoolap::Database;
use thiserror::Error;

use crate::ids::StreamId;

/// §53, verbatim fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncCursor {
    pub peer: DeviceId,
    pub stream: StreamId,
    pub acknowledged_version: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CursorError {
    #[error("sync cursor store backend error: {0}")]
    Backend(String),
}

pub trait CursorStore: Send + Sync {
    /// 0 for a (peer, stream) never acknowledged.
    fn get(
        &self,
        peer: DeviceId,
        stream: StreamId,
    ) -> impl Future<Output = Result<SyncCursor, CursorError>> + Send;

    /// Monotonic: `max(stored, version)`. Returns the resulting cursor.
    fn acknowledge(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> impl Future<Output = Result<SyncCursor, CursorError>> + Send;

    /// Deliberate rewind (peer restored from backup, replication
    /// scope widened). The only way a cursor decreases.
    fn reset(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> impl Future<Output = Result<SyncCursor, CursorError>> + Send;

    /// Every stream cursor held for `peer`, ordered by stream id.
    fn list_for_peer(
        &self,
        peer: DeviceId,
    ) -> impl Future<Output = Result<Vec<SyncCursor>, CursorError>> + Send;
}

#[derive(Debug, Default)]
pub struct InMemoryCursorStore {
    cursors: Mutex<HashMap<(DeviceId, StreamId), u64>>,
}

impl InMemoryCursorStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(DeviceId, StreamId), u64>> {
        self.cursors.lock().expect("cursor store lock poisoned")
    }
}

impl CursorStore for InMemoryCursorStore {
    async fn get(&self, peer: DeviceId, stream: StreamId) -> Result<SyncCursor, CursorError> {
        let v = self.lock().get(&(peer, stream)).copied().unwrap_or(0);
        Ok(SyncCursor {
            peer,
            stream,
            acknowledged_version: v,
        })
    }

    async fn acknowledge(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> Result<SyncCursor, CursorError> {
        let mut map = self.lock();
        let slot = map.entry((peer, stream)).or_insert(0);
        *slot = (*slot).max(version);
        Ok(SyncCursor {
            peer,
            stream,
            acknowledged_version: *slot,
        })
    }

    async fn reset(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> Result<SyncCursor, CursorError> {
        self.lock().insert((peer, stream), version);
        Ok(SyncCursor {
            peer,
            stream,
            acknowledged_version: version,
        })
    }

    async fn list_for_peer(&self, peer: DeviceId) -> Result<Vec<SyncCursor>, CursorError> {
        let mut out: Vec<SyncCursor> = self
            .lock()
            .iter()
            .filter(|((p, _), _)| *p == peer)
            .map(|((p, s), v)| SyncCursor {
                peer: *p,
                stream: *s,
                acknowledged_version: *v,
            })
            .collect();
        out.sort_unstable_by_key(|c| *c.stream.as_bytes());
        Ok(out)
    }
}

/// The durable counterpart — same pattern as
/// [`crate::stoolap_checkpoint_store::StoolapCheckpointStore`], one row
/// per `(peer, stream)`. A lost cursor is not data loss (the peer simply
/// re-receives and §24 dedups), but a durable one avoids re-sending a
/// whole stream after every restart.
pub struct StoolapCursorStore {
    db: Database,
    lock: Arc<Mutex<()>>,
}

fn be(e: impl std::fmt::Display) -> CursorError {
    CursorError::Backend(e.to_string())
}

fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex32(s: &str) -> Result<[u8; 32], CursorError> {
    if s.len() != 64 {
        return Err(be("stored stream_id has wrong length"));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(be)?;
    }
    Ok(out)
}

impl StoolapCursorStore {
    pub fn open(path: &str) -> Result<Self, CursorError> {
        Self::from_database(Database::open(&format!("file://{path}")).map_err(be)?)
    }

    pub fn open_in_memory() -> Result<Self, CursorError> {
        Self::from_database(Database::open_in_memory().map_err(be)?)
    }

    fn from_database(db: Database) -> Result<Self, CursorError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS sync_cursors (
                peer                 TEXT NOT NULL,
                stream_id            TEXT NOT NULL,
                acknowledged_version INTEGER NOT NULL
            )",
            (),
        )
        .map_err(be)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_sync_cursors_key
             ON sync_cursors (peer, stream_id)",
            (),
        )
        .map_err(be)?;
        Ok(Self {
            db,
            lock: Arc::new(Mutex::new(())),
        })
    }

    fn read(&self, peer: &str, stream: &str) -> Result<Option<u64>, CursorError> {
        let mut rows = self
            .db
            .query(
                "SELECT acknowledged_version FROM sync_cursors
                 WHERE peer = $1 AND stream_id = $2",
                (peer.to_string(), stream.to_string()),
            )
            .map_err(be)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(be)?.get::<i64>(0).map_err(be)? as u64)),
            None => Ok(None),
        }
    }

    /// `monotonic`: keep the larger of stored and `version`.
    fn write(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
        monotonic: bool,
    ) -> Result<SyncCursor, CursorError> {
        let _guard = self.lock.lock().expect("cursor store lock poisoned");
        let (p, s) = (peer.as_uuid().to_string(), hex32(stream.as_bytes()));
        let existing = self.read(&p, &s)?;
        let value = match (existing, monotonic) {
            (Some(old), true) => old.max(version),
            _ => version,
        };
        match existing {
            Some(_) => {
                self.db
                    .execute(
                        "UPDATE sync_cursors SET acknowledged_version = $1
                         WHERE peer = $2 AND stream_id = $3",
                        (value as i64, p, s),
                    )
                    .map_err(be)?;
            }
            None => {
                self.db
                    .execute(
                        "INSERT INTO sync_cursors (peer, stream_id, acknowledged_version)
                         VALUES ($1, $2, $3)",
                        (p, s, value as i64),
                    )
                    .map_err(be)?;
            }
        }
        Ok(SyncCursor {
            peer,
            stream,
            acknowledged_version: value,
        })
    }
}

impl CursorStore for StoolapCursorStore {
    async fn get(&self, peer: DeviceId, stream: StreamId) -> Result<SyncCursor, CursorError> {
        let _guard = self.lock.lock().expect("cursor store lock poisoned");
        let v = self
            .read(&peer.as_uuid().to_string(), &hex32(stream.as_bytes()))?
            .unwrap_or(0);
        Ok(SyncCursor {
            peer,
            stream,
            acknowledged_version: v,
        })
    }

    async fn acknowledge(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> Result<SyncCursor, CursorError> {
        self.write(peer, stream, version, true)
    }

    async fn reset(
        &self,
        peer: DeviceId,
        stream: StreamId,
        version: u64,
    ) -> Result<SyncCursor, CursorError> {
        self.write(peer, stream, version, false)
    }

    async fn list_for_peer(&self, peer: DeviceId) -> Result<Vec<SyncCursor>, CursorError> {
        let _guard = self.lock.lock().expect("cursor store lock poisoned");
        let mut out = Vec::new();
        for row in self
            .db
            .query(
                "SELECT stream_id, acknowledged_version FROM sync_cursors
                 WHERE peer = $1 ORDER BY stream_id ASC",
                (peer.as_uuid().to_string(),),
            )
            .map_err(be)?
        {
            let row = row.map_err(be)?;
            let stream: String = row.get(0).map_err(be)?;
            let v: i64 = row.get(1).map_err(be)?;
            out.push(SyncCursor {
                peer,
                stream: StreamId::from_bytes(unhex32(&stream)?),
                acknowledged_version: v as u64,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (DeviceId, DeviceId, StreamId, StreamId) {
        (
            DeviceId::new(),
            DeviceId::new(),
            StreamId::from_name("a"),
            StreamId::from_name("b"),
        )
    }

    async fn contract<C: CursorStore>(c: &C) {
        let (p1, p2, s1, s2) = ids();
        assert_eq!(c.get(p1, s1).await.unwrap().acknowledged_version, 0);

        assert_eq!(
            c.acknowledge(p1, s1, 5).await.unwrap().acknowledged_version,
            5
        );
        // Late / duplicate ack: harmless.
        assert_eq!(
            c.acknowledge(p1, s1, 3).await.unwrap().acknowledged_version,
            5
        );
        assert_eq!(
            c.acknowledge(p1, s1, 5).await.unwrap().acknowledged_version,
            5
        );
        assert_eq!(
            c.acknowledge(p1, s1, 9).await.unwrap().acknowledged_version,
            9
        );

        // Independent per peer and per stream.
        assert_eq!(c.get(p2, s1).await.unwrap().acknowledged_version, 0);
        assert_eq!(c.get(p1, s2).await.unwrap().acknowledged_version, 0);
        c.acknowledge(p1, s2, 2).await.unwrap();
        assert_eq!(c.list_for_peer(p1).await.unwrap().len(), 2);
        assert!(c.list_for_peer(p2).await.unwrap().is_empty());

        // Only reset rewinds.
        assert_eq!(c.reset(p1, s1, 4).await.unwrap().acknowledged_version, 4);
        assert_eq!(c.get(p1, s1).await.unwrap().acknowledged_version, 4);
        assert_eq!(c.reset(p1, s1, 0).await.unwrap().acknowledged_version, 0);
    }

    #[tokio::test]
    async fn in_memory_cursor_contract() {
        contract(&InMemoryCursorStore::new()).await;
    }

    #[tokio::test]
    async fn stoolap_cursor_contract() {
        contract(&StoolapCursorStore::open_in_memory().unwrap()).await;
    }

    #[tokio::test]
    async fn stoolap_cursors_survive_reopen() {
        let path = std::env::temp_dir()
            .join(format!("siar-cursor-{}", uuid::Uuid::new_v4()))
            .to_str()
            .unwrap()
            .to_string();
        let (p, _, s, _) = ids();
        {
            let c = StoolapCursorStore::open(&path).unwrap();
            c.acknowledge(p, s, 42).await.unwrap();
        }
        {
            let c = StoolapCursorStore::open(&path).unwrap();
            assert_eq!(c.get(p, s).await.unwrap().acknowledged_version, 42);
            assert_eq!(c.list_for_peer(p).await.unwrap()[0].stream, s);
        }
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_file(&path);
    }
}
