//! §89 "Metadata Store".
//!
//! ```text
//! filesystem: ciphertext blobs
//! SQLite:     descriptors, references, transfer state, quotas
//! ```
//!
//! "Blob bytes and metadata can be separated … a strong practical
//! architecture." Bytes live in files ([`crate::fs_store`]); the facts
//! *about* them — [`crate::schema_rows::BlobRow`] — live behind this
//! trait. The reasons are the usual ones: listing, counting and
//! "least-recently-used cache blob" must not mean walking a directory
//! tree, and a reference count or classification must change
//! transactionally without rewriting a multi-gigabyte file.
//!
//! [`InMemoryBlobMetadata`] is the reference implementation;
//! [`StoolapBlobMetadata`] is the durable one (the same `stoolap`
//! backend `siar-event-log` and `siar-storage` already use — the spec's
//! "SQLite" role, filled by the workspace's chosen engine).
//!
//! ## The metadata is *advisory about bytes*, never authoritative
//!
//! A row saying "blob X is `Verified`" does not make the file at X's path
//! trustworthy; the file's own hash does ([`crate::fs_store`] verifies
//! before it ever renames into place). Conversely, if the metadata store
//! is lost, every blob is still on disk under its content address and
//! [`crate::fs_store::FsBlobStore::reconcile_metadata`] reconstructs the
//! rows from the files. Losing metadata loses *classification and
//! reference counts*, not data — which is why the one-way dependency
//! (metadata derived from bytes) is the safe direction.
//!
//! The trait is synchronous: the durable backend is an embedded
//! database called in-process, and the store calls it from its own
//! (already blocking) file-I/O paths, so an async trait would add
//! allocation for no concurrency.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use stoolap::Database;

use crate::ids::BlobId;
use crate::schema_rows::BlobRow;
use crate::store_error::BlobError;

pub trait BlobMetadataStore: Send + Sync {
    fn put(&self, row: BlobRow) -> Result<(), BlobError>;
    fn get(&self, id: BlobId) -> Result<Option<BlobRow>, BlobError>;
    /// `true` if a row existed.
    fn remove(&self, id: BlobId) -> Result<bool, BlobError>;
    /// Updates `last_accessed_millis`; absent row is a no-op.
    fn touch(&self, id: BlobId, now_millis: u64) -> Result<(), BlobError>;
    /// Up to `limit` rows, ordered by `blob_id` (stable paging order).
    fn list(&self, limit: usize) -> Result<Vec<BlobRow>, BlobError>;
    fn count(&self) -> Result<u64, BlobError>;
    /// Up to `limit` rows with `blob_id` strictly greater than `after`
    /// (`None` = from the start), in `blob_id` order — the cursor form
    /// of [`Self::list`], so a sweep over millions of blobs pages
    /// instead of loading them all (§59 memory discipline).
    fn list_after(&self, after: Option<BlobId>, limit: usize) -> Result<Vec<BlobRow>, BlobError>;
}

#[derive(Debug, Default)]
pub struct InMemoryBlobMetadata {
    rows: Mutex<HashMap<BlobId, BlobRow>>,
}

impl InMemoryBlobMetadata {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlobMetadataStore for InMemoryBlobMetadata {
    fn put(&self, row: BlobRow) -> Result<(), BlobError> {
        self.rows
            .lock()
            .expect("metadata poisoned")
            .insert(row.blob_id, row);
        Ok(())
    }
    fn get(&self, id: BlobId) -> Result<Option<BlobRow>, BlobError> {
        Ok(self
            .rows
            .lock()
            .expect("metadata poisoned")
            .get(&id)
            .copied())
    }
    fn remove(&self, id: BlobId) -> Result<bool, BlobError> {
        Ok(self
            .rows
            .lock()
            .expect("metadata poisoned")
            .remove(&id)
            .is_some())
    }
    fn touch(&self, id: BlobId, now_millis: u64) -> Result<(), BlobError> {
        if let Some(r) = self.rows.lock().expect("metadata poisoned").get_mut(&id) {
            r.last_accessed_millis = now_millis;
        }
        Ok(())
    }
    fn list(&self, limit: usize) -> Result<Vec<BlobRow>, BlobError> {
        let mut rows: Vec<BlobRow> = self
            .rows
            .lock()
            .expect("metadata poisoned")
            .values()
            .copied()
            .collect();
        rows.sort_by_key(|r| *r.blob_id.as_bytes());
        rows.truncate(limit);
        Ok(rows)
    }
    fn count(&self) -> Result<u64, BlobError> {
        Ok(self.rows.lock().expect("metadata poisoned").len() as u64)
    }
    fn list_after(&self, after: Option<BlobId>, limit: usize) -> Result<Vec<BlobRow>, BlobError> {
        let mut rows: Vec<BlobRow> = self
            .rows
            .lock()
            .expect("metadata poisoned")
            .values()
            .filter(|r| after.is_none_or(|a| r.blob_id.as_bytes() > a.as_bytes()))
            .copied()
            .collect();
        rows.sort_by_key(|r| *r.blob_id.as_bytes());
        rows.truncate(limit);
        Ok(rows)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn me(e: impl std::fmt::Display) -> BlobError {
    BlobError::Metadata(e.to_string())
}

/// Durable metadata on `stoolap`. One row per blob: the hex id (unique
/// index) and the postcard-encoded [`BlobRow`] — so adding a field to
/// `BlobRow` is a postcard versioning question, not a column migration.
pub struct StoolapBlobMetadata {
    db: Database,
    lock: Arc<Mutex<()>>,
}

impl StoolapBlobMetadata {
    pub fn open(path: &str) -> Result<Self, BlobError> {
        Self::from_db(Database::open(&format!("file://{path}")).map_err(me)?)
    }

    pub fn open_in_memory() -> Result<Self, BlobError> {
        Self::from_db(Database::open_in_memory().map_err(me)?)
    }

    fn from_db(db: Database) -> Result<Self, BlobError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS blob_rows (blob_id TEXT NOT NULL, row TEXT NOT NULL)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_blob_rows_id ON blob_rows (blob_id)",
            (),
        )
        .map_err(me)?;
        Ok(Self {
            db,
            lock: Arc::new(Mutex::new(())),
        })
    }

    fn encode(row: &BlobRow) -> Result<String, BlobError> {
        Ok(BASE64.encode(postcard::to_allocvec(row).map_err(me)?))
    }

    fn decode(text: &str) -> Result<BlobRow, BlobError> {
        let bytes = BASE64.decode(text).map_err(me)?;
        postcard::from_bytes(&bytes).map_err(me)
    }

    fn fetch(&self, id: BlobId) -> Result<Option<BlobRow>, BlobError> {
        let mut rows = self
            .db
            .query(
                "SELECT row FROM blob_rows WHERE blob_id = $1",
                (hex(id.as_bytes()),),
            )
            .map_err(me)?;
        match rows.next() {
            Some(r) => Ok(Some(Self::decode(
                &r.map_err(me)?.get::<String>(0).map_err(me)?,
            )?)),
            None => Ok(None),
        }
    }
}

impl BlobMetadataStore for StoolapBlobMetadata {
    fn put(&self, row: BlobRow) -> Result<(), BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        let id = hex(row.blob_id.as_bytes());
        let enc = Self::encode(&row)?;
        if self.fetch(row.blob_id)?.is_some() {
            self.db
                .execute(
                    "UPDATE blob_rows SET row = $1 WHERE blob_id = $2",
                    (enc, id),
                )
                .map_err(me)?;
        } else {
            self.db
                .execute(
                    "INSERT INTO blob_rows (blob_id, row) VALUES ($1, $2)",
                    (id, enc),
                )
                .map_err(me)?;
        }
        Ok(())
    }

    fn get(&self, id: BlobId) -> Result<Option<BlobRow>, BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        self.fetch(id)
    }

    fn remove(&self, id: BlobId) -> Result<bool, BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        let n = self
            .db
            .execute(
                "DELETE FROM blob_rows WHERE blob_id = $1",
                (hex(id.as_bytes()),),
            )
            .map_err(me)?;
        Ok(n > 0)
    }

    fn touch(&self, id: BlobId, now_millis: u64) -> Result<(), BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        if let Some(mut row) = self.fetch(id)? {
            row.last_accessed_millis = now_millis;
            self.db
                .execute(
                    "UPDATE blob_rows SET row = $1 WHERE blob_id = $2",
                    (Self::encode(&row)?, hex(id.as_bytes())),
                )
                .map_err(me)?;
        }
        Ok(())
    }

    fn list(&self, limit: usize) -> Result<Vec<BlobRow>, BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        let mut out = Vec::new();
        for r in self
            .db
            .query(
                "SELECT row FROM blob_rows ORDER BY blob_id ASC LIMIT $1",
                (limit.min(i64::MAX as usize) as i64,),
            )
            .map_err(me)?
        {
            out.push(Self::decode(&r.map_err(me)?.get::<String>(0).map_err(me)?)?);
        }
        Ok(out)
    }

    fn list_after(&self, after: Option<BlobId>, limit: usize) -> Result<Vec<BlobRow>, BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        let mut out = Vec::new();
        let lim = limit.min(i64::MAX as usize) as i64;
        let rows = match after {
            Some(a) => self
                .db
                .query(
                    "SELECT row FROM blob_rows WHERE blob_id > $1 ORDER BY blob_id ASC LIMIT $2",
                    (hex(a.as_bytes()), lim),
                )
                .map_err(me)?,
            None => self
                .db
                .query(
                    "SELECT row FROM blob_rows ORDER BY blob_id ASC LIMIT $1",
                    (lim,),
                )
                .map_err(me)?,
        };
        for r in rows {
            out.push(Self::decode(&r.map_err(me)?.get::<String>(0).map_err(me)?)?);
        }
        Ok(out)
    }

    fn count(&self) -> Result<u64, BlobError> {
        let _g = self.lock.lock().expect("metadata poisoned");
        let mut rows = self
            .db
            .query("SELECT COUNT(*) FROM blob_rows", ())
            .map_err(me)?;
        match rows.next() {
            Some(r) => Ok(r.map_err(me)?.get::<i64>(0).map_err(me)? as u64),
            None => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_rows::BlobLifecycleState;
    use crate::storage_lifecycle::BlobClassification;

    fn row(n: u8, accessed: u64) -> BlobRow {
        BlobRow {
            blob_id: BlobId::from_ciphertext(&[n]),
            size: n as u64 * 100,
            state: BlobLifecycleState::Verified,
            created_at_millis: 1,
            last_accessed_millis: accessed,
            storage_class: BlobClassification::Cache,
            reference_count: n as usize,
        }
    }

    fn contract<M: BlobMetadataStore>(m: &M) {
        assert_eq!(m.count().unwrap(), 0);
        assert_eq!(m.get(row(1, 0).blob_id).unwrap(), None);

        m.put(row(1, 5)).unwrap();
        m.put(row(2, 6)).unwrap();
        assert_eq!(m.get(row(1, 0).blob_id).unwrap(), Some(row(1, 5)));
        assert_eq!(m.count().unwrap(), 2);

        // put replaces.
        let mut changed = row(1, 5);
        changed.reference_count = 99;
        m.put(changed).unwrap();
        assert_eq!(m.get(changed.blob_id).unwrap().unwrap().reference_count, 99);
        assert_eq!(m.count().unwrap(), 2, "replace, not duplicate");

        m.touch(changed.blob_id, 1234).unwrap();
        assert_eq!(
            m.get(changed.blob_id)
                .unwrap()
                .unwrap()
                .last_accessed_millis,
            1234
        );
        m.touch(BlobId::from_ciphertext(b"absent"), 1).unwrap(); // no-op

        let all = m.list(10).unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.windows(2)
                .all(|w| w[0].blob_id.as_bytes() <= w[1].blob_id.as_bytes()),
            "stable order"
        );
        assert_eq!(m.list(1).unwrap().len(), 1);

        // Cursor paging visits every row exactly once, in order.
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = m.list_after(cursor, 1).unwrap();
            let Some(last) = page.last() else { break };
            cursor = Some(last.blob_id);
            seen.extend(page.iter().map(|r| r.blob_id));
        }
        assert_eq!(seen, all.iter().map(|r| r.blob_id).collect::<Vec<_>>());

        assert!(m.remove(changed.blob_id).unwrap());
        assert!(!m.remove(changed.blob_id).unwrap());
        assert_eq!(m.count().unwrap(), 1);
    }

    #[test]
    fn in_memory_contract() {
        contract(&InMemoryBlobMetadata::new());
    }

    #[test]
    fn stoolap_contract() {
        contract(&StoolapBlobMetadata::open_in_memory().unwrap());
    }

    #[test]
    fn stoolap_rows_survive_reopen() {
        let path = std::env::temp_dir().join(format!("siar-blobmeta-{}", uuid::Uuid::new_v4()));
        let p = path.to_str().unwrap();
        {
            let m = StoolapBlobMetadata::open(p).unwrap();
            m.put(row(3, 77)).unwrap();
        }
        {
            let m = StoolapBlobMetadata::open(p).unwrap();
            assert_eq!(m.get(row(3, 0).blob_id).unwrap(), Some(row(3, 77)));
        }
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_file(&path);
    }
}
