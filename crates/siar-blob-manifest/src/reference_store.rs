//! §148 "Blob Reference Model", §149 "Reference Integrity", §150
//! "Reference Count Recovery", §155 "Message Deletion", §156
//! "Disappearing Messages", §157 "User Save".
//!
//! ```rust,ignore
//! BlobReference { owner_namespace, object_id, blob_id, role }
//! ```
//!
//! ## The reference *is* the truth; the count is a cache (§150)
//!
//! [`crate::schema_rows::BlobRow::reference_count`] was always going to
//! drift: it is updated by whoever remembers to. Here the authoritative
//! record is the **set of [`BlobReference`] tuples**. A reference is one
//! `(namespace, object, blob, role)` fact — "message M in conversation C
//! uses blob B as an attachment" — and the set semantics give the right
//! answer to the question that broke a naive counter: two *different*
//! messages referencing one blob are two different tuples; the *same*
//! message re-asserting its reference is the same tuple, an idempotent
//! no-op, not a count of two. [`ReferenceStore::count_for`] is derived
//! from the tuples every time, and [`crate::gc`] repairs any `BlobRow`
//! whose cached count disagrees.
//!
//! ## Transactional with domain state (§149)
//!
//! "Do not update reference count loosely outside transactions." Two
//! guarantees, matching what each situation can actually offer:
//!
//! - **Within the reference store**, [`ReferenceStore::apply`] takes a
//!   *batch* and applies it atomically: deleting a message that has
//!   three attachments is one call, never "removed two of three, then
//!   crashed".
//! - **Across the reference store and a domain's own tables**, the
//!   guarantee needs one database. [`StoolapReferenceStore::
//!   apply_in_transaction`] runs the same operations on a `Database`
//!   handle **the caller already has a transaction open on** — the same
//!   technique `siar-event-log` uses to commit an event and its outbox
//!   row together. The domain `BEGIN`s, writes its own row, calls this,
//!   and `COMMIT`s (or `ROLLBACK`s): the message row and its reference
//!   exist together or not at all. (Two *separate* databases cannot be
//!   made atomic by anything in this crate; for that case the batch
//!   `apply` plus [`crate::gc`]'s reconciliation is the safety net — a
//!   reference that was never added only ever costs a delayed GC, and a
//!   missing one is found by the recount.)
//!
//! ## What deletion does and doesn't do (§155)
//!
//! [`release_object_references`] removes an object's references and
//! *reports* which blobs became unreferenced. It deletes nothing: "if
//! same blob referenced elsewhere: do not delete; GC handles it" — a
//! blob still referenced by another message simply doesn't appear in the
//! result, and one that does appear is a *candidate* whose grace period
//! ([`crate::storage_lifecycle::GcPolicy`]) starts counting in
//! [`crate::gc::sweep`], not an immediate delete.
//!
//! ## Disappearing messages and "Save" (§156, §157)
//!
//! A blob linked only to a disappearing message becomes eligible after
//! *message expiry + retention grace* — [`disappearing_release_at`] — by
//! the message's reference being released then. If the user explicitly
//! saved it, a [`BlobRole::UserSave`] reference exists, so releasing the
//! message's own reference leaves the blob referenced and GC never
//! touches it. [`SaveSemantics`] names the two *different* things "save"
//! can mean, because §157 insists the choice be explicit:
//! [`SaveSemantics::DurableReference`] (this module — a user-owned
//! reference that outlives the message) versus
//! [`SaveSemantics::ExportPlaintext`] (decrypt and write somewhere the
//! user chose, which creates no reference at all and is outside this
//! crate).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use stoolap::Database;

use crate::ids::BlobId;
use crate::storage_lifecycle::GcPolicy;
use crate::store_error::BlobError;
use siar_event_log::namespace::NamespaceId;

/// §148's `ObjectId`: the domain object holding the reference (a
/// message id, a profile id, a transfer id…). Opaque to this crate;
/// validated so it can never be empty or unbounded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(String);

pub const MAX_OBJECT_ID_LEN: usize = 128;

impl ObjectId {
    pub fn new(id: &str) -> Result<Self, BlobError> {
        let ok = !id.is_empty()
            && id.len() <= MAX_OBJECT_ID_LEN
            && id.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.' | b'/')
            });
        if ok {
            Ok(Self(id.to_string()))
        } else {
            Err(BlobError::InvalidArgument(
                "object id must be 1-128 chars of [A-Za-z0-9-_:./]",
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// §148's examples, plus [`Self::UserSave`] for §157.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlobRole {
    MessageAttachment,
    Avatar,
    Thumbnail,
    Transfer,
    Backup,
    /// §157: a user-owned durable reference that outlives the message.
    UserSave,
}

impl BlobRole {
    pub(crate) const fn code(self) -> i64 {
        match self {
            Self::MessageAttachment => 0,
            Self::Avatar => 1,
            Self::Thumbnail => 2,
            Self::Transfer => 3,
            Self::Backup => 4,
            Self::UserSave => 5,
        }
    }

    pub(crate) const fn from_code(c: i64) -> Option<Self> {
        match c {
            0 => Some(Self::MessageAttachment),
            1 => Some(Self::Avatar),
            2 => Some(Self::Thumbnail),
            3 => Some(Self::Transfer),
            4 => Some(Self::Backup),
            5 => Some(Self::UserSave),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobReference {
    pub owner_namespace: NamespaceId,
    pub object_id: ObjectId,
    pub blob_id: BlobId,
    pub role: BlobRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefOp {
    Add(BlobReference),
    Remove(BlobReference),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApplyReport {
    pub added: u32,
    pub removed: u32,
    /// `Add` of a tuple that already existed.
    pub already_present: u32,
    /// `Remove` of a tuple that did not exist.
    pub absent: u32,
}

pub trait ReferenceStore: Send + Sync {
    /// Applies the whole batch atomically. Idempotent per tuple.
    fn apply(&self, ops: &[RefOp]) -> Result<ApplyReport, BlobError>;
    /// Derived from the tuples, every time (§150).
    fn count_for(&self, blob: BlobId) -> Result<u64, BlobError>;
    fn references_for(&self, blob: BlobId, limit: usize) -> Result<Vec<BlobReference>, BlobError>;
    fn references_of_object(
        &self,
        namespace: &NamespaceId,
        object: &ObjectId,
    ) -> Result<Vec<BlobReference>, BlobError>;
    /// Distinct referenced blob ids strictly after `after`, in order.
    fn referenced_blob_ids(
        &self,
        after: Option<BlobId>,
        limit: usize,
    ) -> Result<Vec<BlobId>, BlobError>;
    /// When GC first saw `blob` unreferenced (its grace period's start).
    fn unreferenced_since(&self, blob: BlobId) -> Result<Option<u64>, BlobError>;
    fn set_unreferenced_since(&self, blob: BlobId, since: Option<u64>) -> Result<(), BlobError>;
}

#[derive(Debug, Default)]
struct MemState {
    refs: BTreeSet<BlobReference>,
    marks: HashMap<BlobId, u64>,
}

#[derive(Debug, Default)]
pub struct InMemoryReferenceStore {
    state: Mutex<MemState>,
}

impl InMemoryReferenceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ReferenceStore for InMemoryReferenceStore {
    fn apply(&self, ops: &[RefOp]) -> Result<ApplyReport, BlobError> {
        let mut s = self.state.lock().expect("reference store poisoned");
        let mut r = ApplyReport::default();
        for op in ops {
            match op {
                RefOp::Add(x) => {
                    if s.refs.insert(x.clone()) {
                        r.added += 1;
                    } else {
                        r.already_present += 1;
                    }
                }
                RefOp::Remove(x) => {
                    if s.refs.remove(x) {
                        r.removed += 1;
                    } else {
                        r.absent += 1;
                    }
                }
            }
        }
        Ok(r)
    }

    fn count_for(&self, blob: BlobId) -> Result<u64, BlobError> {
        let s = self.state.lock().expect("reference store poisoned");
        Ok(s.refs.iter().filter(|r| r.blob_id == blob).count() as u64)
    }

    fn references_for(&self, blob: BlobId, limit: usize) -> Result<Vec<BlobReference>, BlobError> {
        let s = self.state.lock().expect("reference store poisoned");
        Ok(s.refs
            .iter()
            .filter(|r| r.blob_id == blob)
            .take(limit)
            .cloned()
            .collect())
    }

    fn references_of_object(
        &self,
        namespace: &NamespaceId,
        object: &ObjectId,
    ) -> Result<Vec<BlobReference>, BlobError> {
        let s = self.state.lock().expect("reference store poisoned");
        Ok(s.refs
            .iter()
            .filter(|r| &r.owner_namespace == namespace && &r.object_id == object)
            .cloned()
            .collect())
    }

    fn referenced_blob_ids(
        &self,
        after: Option<BlobId>,
        limit: usize,
    ) -> Result<Vec<BlobId>, BlobError> {
        let s = self.state.lock().expect("reference store poisoned");
        let ids: BTreeSet<[u8; 32]> = s
            .refs
            .iter()
            .map(|r| *r.blob_id.as_bytes())
            .filter(|b| after.is_none_or(|a| b > a.as_bytes()))
            .collect();
        Ok(ids
            .into_iter()
            .take(limit)
            .map(BlobId::from_raw_digest)
            .collect())
    }

    fn unreferenced_since(&self, blob: BlobId) -> Result<Option<u64>, BlobError> {
        Ok(self
            .state
            .lock()
            .expect("reference store poisoned")
            .marks
            .get(&blob)
            .copied())
    }

    fn set_unreferenced_since(&self, blob: BlobId, since: Option<u64>) -> Result<(), BlobError> {
        let mut s = self.state.lock().expect("reference store poisoned");
        match since {
            Some(t) => {
                s.marks.insert(blob, t);
            }
            None => {
                s.marks.remove(&blob);
            }
        }
        Ok(())
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

fn parse_hex32(s: &str) -> Result<[u8; 32], BlobError> {
    if s.len() != 64 {
        return Err(BlobError::Metadata(
            "stored blob id has wrong length".into(),
        ));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|e| BlobError::Metadata(e.to_string()))?;
    }
    Ok(out)
}

fn me(e: impl std::fmt::Display) -> BlobError {
    BlobError::Metadata(e.to_string())
}

/// Durable references on `stoolap`. See the module doc for how a domain
/// composes [`Self::apply_in_transaction`] with its own writes.
pub struct StoolapReferenceStore {
    db: Database,
    lock: Arc<Mutex<()>>,
}

impl StoolapReferenceStore {
    pub fn open(path: &str) -> Result<Self, BlobError> {
        Self::from_database(Database::open(&format!("file://{path}")).map_err(me)?)
    }

    pub fn open_in_memory() -> Result<Self, BlobError> {
        Self::from_database(Database::open_in_memory().map_err(me)?)
    }

    /// Adopts a database a domain already owns, so the reference tables
    /// live beside its own and share its transactions.
    pub fn from_database(db: Database) -> Result<Self, BlobError> {
        Self::apply_schema(&db)?;
        Ok(Self {
            db,
            lock: Arc::new(Mutex::new(())),
        })
    }

    /// Idempotent DDL, public so a domain can create the tables inside
    /// its own schema setup.
    pub fn apply_schema(db: &Database) -> Result<(), BlobError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS blob_refs (
                namespace TEXT NOT NULL, object_id TEXT NOT NULL,
                blob_id TEXT NOT NULL, role INTEGER NOT NULL)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_blob_refs_tuple
             ON blob_refs (namespace, object_id, blob_id, role)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_blob_refs_blob ON blob_refs (blob_id)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE INDEX IF NOT EXISTS idx_blob_refs_object ON blob_refs (namespace, object_id)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS blob_gc_marks (blob_id TEXT NOT NULL, since INTEGER NOT NULL)",
            (),
        )
        .map_err(me)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_blob_gc_marks ON blob_gc_marks (blob_id)",
            (),
        )
        .map_err(me)?;
        Ok(())
    }

    /// Runs `ops` on `db` **inside whatever transaction the caller has
    /// open on that handle** — no `BEGIN`/`COMMIT` here. See the module
    /// doc (§149).
    pub fn apply_in_transaction(db: &Database, ops: &[RefOp]) -> Result<ApplyReport, BlobError> {
        let mut r = ApplyReport::default();
        for op in ops {
            let (x, adding) = match op {
                RefOp::Add(x) => (x, true),
                RefOp::Remove(x) => (x, false),
            };
            let params = (
                x.owner_namespace.as_str().to_string(),
                x.object_id.as_str().to_string(),
                hex(x.blob_id.as_bytes()),
                x.role.code(),
            );
            let exists = db
                .query(
                    "SELECT 1 FROM blob_refs WHERE namespace = $1 AND object_id = $2
                     AND blob_id = $3 AND role = $4",
                    params.clone(),
                )
                .map_err(me)?
                .next()
                .is_some();
            match (adding, exists) {
                (true, false) => {
                    db.execute(
                        "INSERT INTO blob_refs (namespace, object_id, blob_id, role)
                         VALUES ($1, $2, $3, $4)",
                        params,
                    )
                    .map_err(me)?;
                    r.added += 1;
                }
                (true, true) => r.already_present += 1,
                (false, true) => {
                    db.execute(
                        "DELETE FROM blob_refs WHERE namespace = $1 AND object_id = $2
                         AND blob_id = $3 AND role = $4",
                        params,
                    )
                    .map_err(me)?;
                    r.removed += 1;
                }
                (false, false) => r.absent += 1,
            }
        }
        Ok(r)
    }

    fn rows_to_refs(
        rows: impl Iterator<Item = Result<stoolap::ResultRow, stoolap::Error>>,
    ) -> Result<Vec<BlobReference>, BlobError> {
        let mut out = Vec::new();
        for r in rows {
            let r = r.map_err(me)?;
            let ns: String = r.get(0).map_err(me)?;
            let obj: String = r.get(1).map_err(me)?;
            let blob: String = r.get(2).map_err(me)?;
            let role: i64 = r.get(3).map_err(me)?;
            out.push(BlobReference {
                owner_namespace: NamespaceId::new(&ns).map_err(me)?,
                object_id: ObjectId::new(&obj)?,
                blob_id: BlobId::from_raw_digest(parse_hex32(&blob)?),
                role: BlobRole::from_code(role).ok_or_else(|| me("unknown stored role"))?,
            });
        }
        Ok(out)
    }
}

impl ReferenceStore for StoolapReferenceStore {
    fn apply(&self, ops: &[RefOp]) -> Result<ApplyReport, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        self.db.execute("BEGIN", ()).map_err(me)?;
        match Self::apply_in_transaction(&self.db, ops) {
            Ok(r) => {
                self.db.execute("COMMIT", ()).map_err(me)?;
                Ok(r)
            }
            Err(e) => {
                let _ = self.db.execute("ROLLBACK", ());
                Err(e)
            }
        }
    }

    fn count_for(&self, blob: BlobId) -> Result<u64, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let mut rows = self
            .db
            .query(
                "SELECT COUNT(*) FROM blob_refs WHERE blob_id = $1",
                (hex(blob.as_bytes()),),
            )
            .map_err(me)?;
        match rows.next() {
            Some(r) => Ok(r.map_err(me)?.get::<i64>(0).map_err(me)? as u64),
            None => Ok(0),
        }
    }

    fn references_for(&self, blob: BlobId, limit: usize) -> Result<Vec<BlobReference>, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let rows = self
            .db
            .query(
                "SELECT namespace, object_id, blob_id, role FROM blob_refs
                 WHERE blob_id = $1 ORDER BY namespace ASC, object_id ASC, role ASC LIMIT $2",
                (hex(blob.as_bytes()), limit.min(i64::MAX as usize) as i64),
            )
            .map_err(me)?;
        Self::rows_to_refs(rows)
    }

    fn references_of_object(
        &self,
        namespace: &NamespaceId,
        object: &ObjectId,
    ) -> Result<Vec<BlobReference>, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let rows = self
            .db
            .query(
                "SELECT namespace, object_id, blob_id, role FROM blob_refs
                 WHERE namespace = $1 AND object_id = $2 ORDER BY blob_id ASC, role ASC",
                (namespace.as_str().to_string(), object.as_str().to_string()),
            )
            .map_err(me)?;
        Self::rows_to_refs(rows)
    }

    fn referenced_blob_ids(
        &self,
        after: Option<BlobId>,
        limit: usize,
    ) -> Result<Vec<BlobId>, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let lim = limit.min(i64::MAX as usize) as i64;
        let rows = match after {
            Some(a) => self
                .db
                .query(
                    "SELECT blob_id FROM blob_refs WHERE blob_id > $1
                     GROUP BY blob_id ORDER BY blob_id ASC LIMIT $2",
                    (hex(a.as_bytes()), lim),
                )
                .map_err(me)?,
            None => self
                .db
                .query(
                    "SELECT blob_id FROM blob_refs GROUP BY blob_id ORDER BY blob_id ASC LIMIT $1",
                    (lim,),
                )
                .map_err(me)?,
        };
        let mut out = Vec::new();
        for r in rows {
            let s: String = r.map_err(me)?.get(0).map_err(me)?;
            out.push(BlobId::from_raw_digest(parse_hex32(&s)?));
        }
        Ok(out)
    }

    fn unreferenced_since(&self, blob: BlobId) -> Result<Option<u64>, BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let mut rows = self
            .db
            .query(
                "SELECT since FROM blob_gc_marks WHERE blob_id = $1",
                (hex(blob.as_bytes()),),
            )
            .map_err(me)?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(me)?.get::<i64>(0).map_err(me)? as u64)),
            None => Ok(None),
        }
    }

    fn set_unreferenced_since(&self, blob: BlobId, since: Option<u64>) -> Result<(), BlobError> {
        let _g = self.lock.lock().expect("reference store poisoned");
        let id = hex(blob.as_bytes());
        match since {
            Some(t) => {
                let exists = self
                    .db
                    .query(
                        "SELECT 1 FROM blob_gc_marks WHERE blob_id = $1",
                        (id.clone(),),
                    )
                    .map_err(me)?
                    .next()
                    .is_some();
                if exists {
                    self.db
                        .execute(
                            "UPDATE blob_gc_marks SET since = $1 WHERE blob_id = $2",
                            (t as i64, id),
                        )
                        .map_err(me)?;
                } else {
                    self.db
                        .execute(
                            "INSERT INTO blob_gc_marks (blob_id, since) VALUES ($1, $2)",
                            (id, t as i64),
                        )
                        .map_err(me)?;
                }
            }
            None => {
                self.db
                    .execute("DELETE FROM blob_gc_marks WHERE blob_id = $1", (id,))
                    .map_err(me)?;
            }
        }
        Ok(())
    }
}

/// What [`release_object_references`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReport {
    pub removed: u32,
    /// Blobs that had a reference from this object and now have **none
    /// at all**. Candidates for GC, never deleted here.
    pub now_unreferenced: Vec<BlobId>,
}

/// §155: drop every reference `object` holds (one atomic batch) and
/// report which blobs that left unreferenced. Blobs still referenced
/// by anything else are absent from the result — "do not delete".
pub fn release_object_references<R: ReferenceStore + ?Sized>(
    refs: &R,
    namespace: &NamespaceId,
    object: &ObjectId,
) -> Result<ReleaseReport, BlobError> {
    let held = refs.references_of_object(namespace, object)?;
    let ops: Vec<RefOp> = held.iter().cloned().map(RefOp::Remove).collect();
    let report = refs.apply(&ops)?;
    let mut now_unreferenced = BTreeSet::new();
    for r in &held {
        if refs.count_for(r.blob_id)? == 0 {
            now_unreferenced.insert(*r.blob_id.as_bytes());
        }
    }
    Ok(ReleaseReport {
        removed: report.removed,
        now_unreferenced: now_unreferenced
            .into_iter()
            .map(BlobId::from_raw_digest)
            .collect(),
    })
}

/// §156: the instant a disappearing message's reference should be
/// released — *message expiry + retention grace*. Saturating, so a
/// huge expiry cannot wrap into the past.
pub fn disappearing_release_at(message_expires_at_millis: u64, policy: &GcPolicy) -> u64 {
    message_expires_at_millis.saturating_add(policy.grace_period_millis)
}

/// §157's two meanings of "save".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveSemantics {
    /// A user-owned durable reference ([`user_save_reference`]).
    DurableReference,
    /// Decrypt and write to a user-chosen location. Creates **no**
    /// reference; the plaintext copy is the user's, outside this
    /// subsystem's lifecycle.
    ExportPlaintext,
}

/// §157's [`SaveSemantics::DurableReference`]: a [`BlobRole::UserSave`]
/// reference the *user* owns, which survives the message's own
/// reference being released.
pub fn user_save_reference(
    owner_namespace: NamespaceId,
    saved_object: ObjectId,
    blob_id: BlobId,
) -> BlobReference {
    BlobReference {
        owner_namespace,
        object_id: saved_object,
        blob_id,
        role: BlobRole::UserSave,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(s: &str) -> NamespaceId {
        NamespaceId::new(s).unwrap()
    }
    fn obj(s: &str) -> ObjectId {
        ObjectId::new(s).unwrap()
    }
    fn blob(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }
    fn r(n: &str, o: &str, b: u8, role: BlobRole) -> BlobReference {
        BlobReference {
            owner_namespace: ns(n),
            object_id: obj(o),
            blob_id: blob(b),
            role,
        }
    }

    #[test]
    fn object_ids_are_bounded_and_restricted() {
        assert!(ObjectId::new("msg:7f3a/att-1").is_ok());
        for bad in ["", "has space", "new\nline", "ünï", &"a".repeat(129)] {
            assert!(ObjectId::new(bad).is_err(), "{bad:?}");
        }
    }

    fn contract<R: ReferenceStore>(s: &R) {
        let a = r("personal", "msg-1", 1, BlobRole::MessageAttachment);
        let b = r("personal", "msg-2", 1, BlobRole::MessageAttachment); // same blob, other message
        let c = r("personal", "msg-1", 2, BlobRole::MessageAttachment);

        let rep = s
            .apply(&[
                RefOp::Add(a.clone()),
                RefOp::Add(b.clone()),
                RefOp::Add(c.clone()),
            ])
            .unwrap();
        assert_eq!(
            rep,
            ApplyReport {
                added: 3,
                ..Default::default()
            }
        );
        assert_eq!(
            s.count_for(blob(1)).unwrap(),
            2,
            "two messages = two references"
        );

        // The same message re-asserting its reference is NOT a second one.
        let rep = s.apply(&[RefOp::Add(a.clone())]).unwrap();
        assert_eq!((rep.added, rep.already_present), (0, 1));
        assert_eq!(s.count_for(blob(1)).unwrap(), 2);

        // Roles distinguish tuples.
        s.apply(&[RefOp::Add(r("personal", "msg-1", 1, BlobRole::UserSave))])
            .unwrap();
        assert_eq!(s.count_for(blob(1)).unwrap(), 3);

        assert_eq!(
            s.references_of_object(&ns("personal"), &obj("msg-1"))
                .unwrap()
                .len(),
            3
        );
        assert_eq!(s.references_for(blob(1), 10).unwrap().len(), 3);
        assert_eq!(s.references_for(blob(1), 1).unwrap().len(), 1);
        // Namespaces never mix.
        assert!(s
            .references_of_object(&ns("work"), &obj("msg-1"))
            .unwrap()
            .is_empty());

        // Remove is idempotent.
        let rep = s
            .apply(&[RefOp::Remove(b.clone()), RefOp::Remove(b.clone())])
            .unwrap();
        assert_eq!((rep.removed, rep.absent), (1, 1));

        // Distinct referenced ids, cursor-paged, ordered.
        let mut ids = Vec::new();
        let mut cur = None;
        loop {
            let page = s.referenced_blob_ids(cur, 1).unwrap();
            let Some(last) = page.last() else { break };
            cur = Some(*last);
            ids.extend(page);
        }
        let mut want = vec![blob(1), blob(2)];
        want.sort_by_key(|b| *b.as_bytes());
        assert_eq!(ids, want);

        // GC marks round-trip.
        assert_eq!(s.unreferenced_since(blob(9)).unwrap(), None);
        s.set_unreferenced_since(blob(9), Some(500)).unwrap();
        assert_eq!(s.unreferenced_since(blob(9)).unwrap(), Some(500));
        s.set_unreferenced_since(blob(9), Some(700)).unwrap();
        assert_eq!(s.unreferenced_since(blob(9)).unwrap(), Some(700));
        s.set_unreferenced_since(blob(9), None).unwrap();
        assert_eq!(s.unreferenced_since(blob(9)).unwrap(), None);
    }

    #[test]
    fn in_memory_contract() {
        contract(&InMemoryReferenceStore::new());
    }

    #[test]
    fn stoolap_contract() {
        contract(&StoolapReferenceStore::open_in_memory().unwrap());
    }

    #[test]
    fn stoolap_references_survive_reopen() {
        let path = std::env::temp_dir().join(format!("siar-refs-{}", uuid::Uuid::new_v4()));
        let p = path.to_str().unwrap();
        {
            let s = StoolapReferenceStore::open(p).unwrap();
            s.apply(&[RefOp::Add(r("personal", "m", 4, BlobRole::Avatar))])
                .unwrap();
        }
        {
            let s = StoolapReferenceStore::open(p).unwrap();
            assert_eq!(s.count_for(blob(4)).unwrap(), 1);
        }
        let _ = std::fs::remove_dir_all(&path);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reference_commits_or_rolls_back_with_the_domain_row_it_belongs_to() {
        // §149: a "message" table and the reference live in ONE
        // database; the domain opens the transaction.
        let db = Database::open_in_memory().unwrap();
        db.execute("CREATE TABLE messages (id TEXT NOT NULL)", ())
            .unwrap();
        StoolapReferenceStore::apply_schema(&db).unwrap();
        let count = |sql: &str| -> i64 {
            db.query(sql, ())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .get(0)
                .unwrap()
        };

        // Committed together.
        db.execute("BEGIN", ()).unwrap();
        db.execute("INSERT INTO messages (id) VALUES ('m1')", ())
            .unwrap();
        StoolapReferenceStore::apply_in_transaction(
            &db,
            &[RefOp::Add(r(
                "personal",
                "m1",
                1,
                BlobRole::MessageAttachment,
            ))],
        )
        .unwrap();
        db.execute("COMMIT", ()).unwrap();
        assert_eq!(
            (
                count("SELECT COUNT(*) FROM messages"),
                count("SELECT COUNT(*) FROM blob_refs")
            ),
            (1, 1)
        );

        // Rolled back together (the domain operation failed after
        // adding the reference).
        db.execute("BEGIN", ()).unwrap();
        db.execute("INSERT INTO messages (id) VALUES ('m2')", ())
            .unwrap();
        StoolapReferenceStore::apply_in_transaction(
            &db,
            &[RefOp::Add(r(
                "personal",
                "m2",
                2,
                BlobRole::MessageAttachment,
            ))],
        )
        .unwrap();
        db.execute("ROLLBACK", ()).unwrap();
        assert_eq!(
            (
                count("SELECT COUNT(*) FROM messages"),
                count("SELECT COUNT(*) FROM blob_refs")
            ),
            (1, 1),
            "neither the message nor its reference survived the rollback"
        );
    }

    fn release_contract<R: ReferenceStore>(s: &R) {
        // Two messages share blob 1; message 3 alone holds blob 2.
        s.apply(&[
            RefOp::Add(r("personal", "m1", 1, BlobRole::MessageAttachment)),
            RefOp::Add(r("personal", "m2", 1, BlobRole::MessageAttachment)),
            RefOp::Add(r("personal", "m3", 2, BlobRole::MessageAttachment)),
            RefOp::Add(r("personal", "m3", 3, BlobRole::Thumbnail)),
        ])
        .unwrap();

        // Deleting m1: blob 1 is still referenced by m2 -> NOT a candidate.
        let rep = release_object_references(s, &ns("personal"), &obj("m1")).unwrap();
        assert_eq!(rep.removed, 1);
        assert!(
            rep.now_unreferenced.is_empty(),
            "same blob referenced elsewhere: do not delete"
        );
        assert_eq!(s.count_for(blob(1)).unwrap(), 1);

        // Deleting m2 too: now blob 1 has nothing.
        let rep = release_object_references(s, &ns("personal"), &obj("m2")).unwrap();
        assert_eq!(rep.now_unreferenced, vec![blob(1)]);

        // m3 releases two blobs in one atomic batch.
        let rep = release_object_references(s, &ns("personal"), &obj("m3")).unwrap();
        assert_eq!(rep.removed, 2);
        let mut want = vec![blob(2), blob(3)];
        want.sort_by_key(|b| *b.as_bytes());
        assert_eq!(rep.now_unreferenced, want);

        // Releasing an object with no references is a clean no-op.
        let rep = release_object_references(s, &ns("personal"), &obj("never")).unwrap();
        assert_eq!((rep.removed, rep.now_unreferenced.len()), (0, 0));
    }

    #[test]
    fn deleting_a_message_releases_only_blobs_nothing_else_references() {
        release_contract(&InMemoryReferenceStore::new());
        release_contract(&StoolapReferenceStore::open_in_memory().unwrap());
    }

    #[test]
    fn a_user_saved_blob_survives_its_disappearing_message() {
        let s = InMemoryReferenceStore::new();
        let policy = GcPolicy {
            grace_period_millis: 3_600_000,
        };
        // The disappearing message references blob 7; the user also saved it.
        s.apply(&[
            RefOp::Add(r(
                "personal",
                "msg-vanishing",
                7,
                BlobRole::MessageAttachment,
            )),
            RefOp::Add(user_save_reference(ns("personal"), obj("saved-1"), blob(7))),
            RefOp::Add(r(
                "personal",
                "msg-vanishing",
                8,
                BlobRole::MessageAttachment,
            )),
        ])
        .unwrap();

        // Expiry + grace says WHEN to release.
        assert_eq!(disappearing_release_at(10_000, &policy), 3_610_000);
        assert_eq!(
            disappearing_release_at(u64::MAX, &policy),
            u64::MAX,
            "saturates"
        );

        let rep = release_object_references(&s, &ns("personal"), &obj("msg-vanishing")).unwrap();
        assert_eq!(
            rep.now_unreferenced,
            vec![blob(8)],
            "blob 7 is user-saved, so it stays referenced"
        );
        assert_eq!(s.count_for(blob(7)).unwrap(), 1);
    }

    #[test]
    fn the_two_meanings_of_save_are_distinct() {
        assert_ne!(
            SaveSemantics::DurableReference,
            SaveSemantics::ExportPlaintext
        );
        let sr = user_save_reference(ns("personal"), obj("saved-2"), blob(1));
        assert_eq!(sr.role, BlobRole::UserSave);
    }
}
