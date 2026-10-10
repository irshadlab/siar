//! `04-offline-event-log-architecture.md` §38 "Snapshotting" and §39
//! "Snapshot Structure" — §92 Phase 7.
//!
//! ```text
//! events 1..10000 → snapshot @ 10000 → replay 10001+
//! ```
//!
//! "Snapshot is derived state, not authoritative history," and "if
//! invalid: discard, rebuild from log." Both sentences are enforced
//! structurally by [`load_state`]:
//!
//! - The log is the only source of truth. A snapshot is *only ever an
//!   optimization of replay*: [`load_state`] returns the same state with
//!   or without one (property-tested), so deleting every snapshot is
//!   always safe.
//! - A snapshot is used only if **all** of: its [`Snapshot::checksum`]
//!   verifies, its `projection_version` equals the fold's current
//!   [`StreamFold::PROJECTION_VERSION`] (the fold's logic may have
//!   changed since — same versioning discipline as §16), its state
//!   decodes, and its `stream_version` is not beyond the stream's real
//!   head (a store restored from an older backup). Otherwise it is
//!   discarded, a [`LoadSource::FullReplay`] with the reason is
//!   returned, and the bad row is removed so the next load doesn't
//!   re-pay for it.
//!
//! ## One snapshot per stream
//!
//! [`Snapshot`] has exactly §39's fields, so a stream carries one
//! snapshot, replaced by a newer one. Two different folds snapshotting
//! the *same* stream would evict each other; today every fold is
//! per-domain-stream so this doesn't arise, and the `projection_version`
//! check means an eviction is merely a rebuild, never a wrong answer.
//!
//! ## Deletion
//!
//! A snapshot can contain data derived from a payload that later gets
//! redacted. [`SnapshotStore::delete_stream`] exists for
//! [`crate::deletion::execute_deletion`], which must purge it.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use stoolap::Database;
use thiserror::Error;

use crate::ids::StreamId;
use crate::store::{EventStore, EventStoreError, StoredEvent};

/// §39, verbatim fields (`Bytes` → `Vec<u8>`, `Hash` → blake3 bytes, as
/// elsewhere in this crate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub stream_id: StreamId,
    pub stream_version: u64,
    pub projection_version: u16,
    pub state: Vec<u8>,
    pub checksum: [u8; 32],
}

impl Snapshot {
    pub fn new(
        stream_id: StreamId,
        stream_version: u64,
        projection_version: u16,
        state: Vec<u8>,
    ) -> Self {
        let checksum =
            Self::compute_checksum(stream_id, stream_version, projection_version, &state);
        Self {
            stream_id,
            stream_version,
            projection_version,
            state,
            checksum,
        }
    }

    fn compute_checksum(
        stream_id: StreamId,
        stream_version: u64,
        projection_version: u16,
        state: &[u8],
    ) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"siar/snapshot/v1");
        h.update(stream_id.as_bytes());
        h.update(&stream_version.to_le_bytes());
        h.update(&projection_version.to_le_bytes());
        h.update(&(state.len() as u64).to_le_bytes());
        h.update(state);
        *h.finalize().as_bytes()
    }

    pub fn verify(&self) -> bool {
        self.checksum
            == Self::compute_checksum(
                self.stream_id,
                self.stream_version,
                self.projection_version,
                &self.state,
            )
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("snapshot store backend error: {0}")]
    Backend(String),
    #[error(transparent)]
    Store(#[from] EventStoreError),
}

pub trait SnapshotStore: Send + Sync {
    /// Keeps the newer of the stored and the given snapshot.
    fn save(&self, snapshot: Snapshot) -> impl Future<Output = Result<(), SnapshotError>> + Send;

    fn load_latest(
        &self,
        stream: StreamId,
    ) -> impl Future<Output = Result<Option<Snapshot>, SnapshotError>> + Send;

    /// Returns how many were removed (0 or 1).
    fn delete_stream(
        &self,
        stream: StreamId,
    ) -> impl Future<Output = Result<u64, SnapshotError>> + Send;
}

#[derive(Debug, Default)]
pub struct InMemorySnapshotStore {
    snapshots: Mutex<HashMap<StreamId, Snapshot>>,
}

impl InMemorySnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: overwrite a stored snapshot without re-checksumming,
    /// to simulate on-disk corruption.
    #[cfg(test)]
    pub(crate) fn corrupt_state(&self, stream: StreamId) {
        if let Some(s) = self.snapshots.lock().unwrap().get_mut(&stream) {
            s.state.push(0xff);
        }
    }
}

impl SnapshotStore for InMemorySnapshotStore {
    async fn save(&self, snapshot: Snapshot) -> Result<(), SnapshotError> {
        let mut map = self.snapshots.lock().expect("snapshot lock poisoned");
        match map.get(&snapshot.stream_id) {
            Some(old) if old.stream_version > snapshot.stream_version => {}
            _ => {
                map.insert(snapshot.stream_id, snapshot);
            }
        }
        Ok(())
    }

    async fn load_latest(&self, stream: StreamId) -> Result<Option<Snapshot>, SnapshotError> {
        Ok(self
            .snapshots
            .lock()
            .expect("snapshot lock poisoned")
            .get(&stream)
            .cloned())
    }

    async fn delete_stream(&self, stream: StreamId) -> Result<u64, SnapshotError> {
        Ok(u64::from(
            self.snapshots
                .lock()
                .expect("snapshot lock poisoned")
                .remove(&stream)
                .is_some(),
        ))
    }
}

/// Durable snapshots — one row per stream.
pub struct StoolapSnapshotStore {
    db: Database,
    lock: Arc<Mutex<()>>,
}

fn be(e: impl std::fmt::Display) -> SnapshotError {
    SnapshotError::Backend(e.to_string())
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

fn unhex32(s: &str) -> Result<[u8; 32], SnapshotError> {
    if s.len() != 64 {
        return Err(be("stored value has wrong length"));
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(be)?;
    }
    Ok(out)
}

impl StoolapSnapshotStore {
    pub fn open(path: &str) -> Result<Self, SnapshotError> {
        Self::from_database(Database::open(&format!("file://{path}")).map_err(be)?)
    }

    pub fn open_in_memory() -> Result<Self, SnapshotError> {
        Self::from_database(Database::open_in_memory().map_err(be)?)
    }

    fn from_database(db: Database) -> Result<Self, SnapshotError> {
        db.execute(
            "CREATE TABLE IF NOT EXISTS snapshots (
                stream_id          TEXT NOT NULL,
                stream_version     INTEGER NOT NULL,
                projection_version INTEGER NOT NULL,
                state              TEXT NOT NULL,
                checksum           TEXT NOT NULL
            )",
            (),
        )
        .map_err(be)?;
        db.execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_snapshots_stream ON snapshots (stream_id)",
            (),
        )
        .map_err(be)?;
        Ok(Self {
            db,
            lock: Arc::new(Mutex::new(())),
        })
    }

    /// Test hook mirroring the in-memory one: change the stored state
    /// text without touching the checksum column.
    #[cfg(test)]
    pub(crate) fn corrupt_state(&self, stream: StreamId) {
        self.db
            .execute(
                "UPDATE snapshots SET state = 'AAAA' WHERE stream_id = $1",
                (hex(stream.as_bytes()),),
            )
            .unwrap();
    }
}

impl SnapshotStore for StoolapSnapshotStore {
    async fn save(&self, s: Snapshot) -> Result<(), SnapshotError> {
        let _guard = self.lock.lock().expect("snapshot lock poisoned");
        let id = hex(s.stream_id.as_bytes());
        self.db.execute("BEGIN", ()).map_err(be)?;
        let run = || -> Result<(), SnapshotError> {
            let existing: Option<i64> = {
                let mut rows = self
                    .db
                    .query(
                        "SELECT stream_version FROM snapshots WHERE stream_id = $1",
                        (id.clone(),),
                    )
                    .map_err(be)?;
                match rows.next() {
                    Some(r) => Some(r.map_err(be)?.get(0).map_err(be)?),
                    None => None,
                }
            };
            match existing {
                Some(v) if v as u64 > s.stream_version => Ok(()),
                Some(_) => {
                    self.db
                        .execute(
                            "UPDATE snapshots SET stream_version = $1, projection_version = $2,
                                    state = $3, checksum = $4 WHERE stream_id = $5",
                            (
                                s.stream_version as i64,
                                i64::from(s.projection_version),
                                BASE64.encode(&s.state),
                                hex(&s.checksum),
                                id,
                            ),
                        )
                        .map_err(be)?;
                    Ok(())
                }
                None => {
                    self.db
                        .execute(
                            "INSERT INTO snapshots
                                (stream_id, stream_version, projection_version, state, checksum)
                             VALUES ($1, $2, $3, $4, $5)",
                            (
                                id,
                                s.stream_version as i64,
                                i64::from(s.projection_version),
                                BASE64.encode(&s.state),
                                hex(&s.checksum),
                            ),
                        )
                        .map_err(be)?;
                    Ok(())
                }
            }
        };
        match run() {
            Ok(()) => self.db.execute("COMMIT", ()).map(|_| ()).map_err(be),
            Err(e) => {
                let _ = self.db.execute("ROLLBACK", ());
                Err(e)
            }
        }
    }

    async fn load_latest(&self, stream: StreamId) -> Result<Option<Snapshot>, SnapshotError> {
        let _guard = self.lock.lock().expect("snapshot lock poisoned");
        let mut rows = self
            .db
            .query(
                "SELECT stream_version, projection_version, state, checksum
                 FROM snapshots WHERE stream_id = $1",
                (hex(stream.as_bytes()),),
            )
            .map_err(be)?;
        let Some(row) = rows.next() else {
            return Ok(None);
        };
        let row = row.map_err(be)?;
        let version: i64 = row.get(0).map_err(be)?;
        let proj: i64 = row.get(1).map_err(be)?;
        let state: String = row.get(2).map_err(be)?;
        let checksum: String = row.get(3).map_err(be)?;
        // A row that cannot even be decoded is returned as a snapshot
        // that fails `verify`, so the caller's one discard path handles
        // every kind of damage.
        Ok(Some(Snapshot {
            stream_id: stream,
            stream_version: version as u64,
            projection_version: proj as u16,
            state: BASE64.decode(state).unwrap_or_default(),
            checksum: unhex32(&checksum).unwrap_or([0; 32]),
        }))
    }

    async fn delete_stream(&self, stream: StreamId) -> Result<u64, SnapshotError> {
        let _guard = self.lock.lock().expect("snapshot lock poisoned");
        let n = self
            .db
            .execute(
                "DELETE FROM snapshots WHERE stream_id = $1",
                (hex(stream.as_bytes()),),
            )
            .map_err(be)?;
        Ok(n as u64)
    }
}

/// A fold of a stream's events into some state. The state is
/// whatever a domain needs (a conversation summary, a device
/// directory); this crate defines none.
pub trait StreamFold: Send + Sync {
    type State: Send;
    /// Bump when `apply`/`encode`'s meaning changes — see the module
    /// doc on why a mismatch discards the snapshot.
    const PROJECTION_VERSION: u16;

    fn initial(&self) -> Self::State;
    fn apply(&self, state: &mut Self::State, event: &StoredEvent);
    fn encode(&self, state: &Self::State) -> Vec<u8>;
    /// `None` for bytes that don't decode — treated as an invalid
    /// snapshot, not an error.
    fn decode(&self, bytes: &[u8]) -> Option<Self::State>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscardReason {
    BadChecksum,
    ProjectionVersionChanged { snapshot: u16, current: u16 },
    UndecodableState,
    BeyondStreamHead { snapshot_version: u64, head: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    /// Started from a snapshot at this version.
    Snapshot { at: u64 },
    /// Replayed from version 0. `discarded` is why an existing
    /// snapshot was not used, if one existed.
    FullReplay { discarded: Option<DiscardReason> },
}

#[derive(Debug)]
pub struct Loaded<S> {
    pub state: S,
    /// The stream version `state` reflects.
    pub version: u64,
    pub source: LoadSource,
    /// Events replayed on top of the starting point.
    pub replayed: u64,
}

const REPLAY_PAGE: usize = 512;

/// §38's "snapshot @ N, replay N+1..". See the module doc for exactly
/// when a snapshot is trusted.
pub async fn load_state<E, N, F>(
    events: &E,
    snapshots: &N,
    fold: &F,
    stream: StreamId,
) -> Result<Loaded<F::State>, SnapshotError>
where
    E: EventStore + ?Sized,
    N: SnapshotStore + ?Sized,
    F: StreamFold,
{
    let head = events.stream_head(stream).await?;
    let mut discarded = None;
    let mut start: Option<(F::State, u64)> = None;

    if let Some(snap) = snapshots.load_latest(stream).await? {
        let verdict = if !snap.verify() {
            Some(DiscardReason::BadChecksum)
        } else if snap.projection_version != F::PROJECTION_VERSION {
            Some(DiscardReason::ProjectionVersionChanged {
                snapshot: snap.projection_version,
                current: F::PROJECTION_VERSION,
            })
        } else if snap.stream_version > head {
            Some(DiscardReason::BeyondStreamHead {
                snapshot_version: snap.stream_version,
                head,
            })
        } else {
            match fold.decode(&snap.state) {
                Some(state) => {
                    start = Some((state, snap.stream_version));
                    None
                }
                None => Some(DiscardReason::UndecodableState),
            }
        };
        if let Some(reason) = verdict {
            // §39: "discard, rebuild from log."
            snapshots.delete_stream(stream).await?;
            discarded = Some(reason);
        }
    }

    let source = match &start {
        Some((_, at)) => LoadSource::Snapshot { at: *at },
        None => LoadSource::FullReplay { discarded },
    };
    let (mut state, mut version) = start.unwrap_or_else(|| (fold.initial(), 0));
    let mut replayed = 0u64;
    loop {
        let page = events.read_stream(stream, version, REPLAY_PAGE).await?;
        let Some(last) = page.last() else { break };
        let last_version = last.envelope.stream_version;
        for e in &page {
            fold.apply(&mut state, e);
            replayed += 1;
        }
        version = last_version;
    }
    Ok(Loaded {
        state,
        version,
        source,
        replayed,
    })
}

/// Saves a snapshot of `loaded` if at least `every` events have been
/// folded since `last_snapshot_version`. Returns whether it did.
pub async fn snapshot_if_due<N, F>(
    snapshots: &N,
    fold: &F,
    stream: StreamId,
    loaded: &Loaded<F::State>,
    last_snapshot_version: u64,
    every: u64,
) -> Result<bool, SnapshotError>
where
    N: SnapshotStore + ?Sized,
    F: StreamFold,
{
    if every == 0 || loaded.version.saturating_sub(last_snapshot_version) < every {
        return Ok(false);
    }
    snapshots
        .save(Snapshot::new(
            stream,
            loaded.version,
            F::PROJECTION_VERSION,
            fold.encode(&loaded.state),
        ))
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::store::{AppendRequest, NewEvent};
    use proptest::prelude::*;
    use siar_domain::DeviceId;

    /// Sums payload bytes; encodes as 8 LE bytes.
    struct SumFold<const V: u16>;
    impl<const V: u16> StreamFold for SumFold<V> {
        type State = u64;
        const PROJECTION_VERSION: u16 = V;
        fn initial(&self) -> u64 {
            0
        }
        fn apply(&self, s: &mut u64, e: &StoredEvent) {
            *s += e
                .envelope
                .payload
                .iter()
                .map(|b| u64::from(*b))
                .sum::<u64>();
        }
        fn encode(&self, s: &u64) -> Vec<u8> {
            s.to_le_bytes().to_vec()
        }
        fn decode(&self, b: &[u8]) -> Option<u64> {
            Some(u64::from_le_bytes(b.try_into().ok()?))
        }
    }

    fn ev(n: u8) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: EventTypeId(1),
            schema_version: 1,
            created_at: Timestamp(0),
            origin: EventOrigin::LocalDevice(DeviceId::new()),
            correlation_id: None,
            causation_id: None,
            payload: vec![n],
        }
    }

    async fn fill(store: &InMemoryEventStore, stream: StreamId, from: u64, vals: &[u8]) {
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: from,
                events: vals.iter().map(|v| ev(*v)).collect(),
            })
            .await
            .unwrap();
    }

    #[test]
    fn checksum_covers_every_field() {
        let s = Snapshot::new(StreamId::from_name("a"), 5, 1, vec![1, 2, 3]);
        assert!(s.verify());
        for tamper in [
            |s: &mut Snapshot| s.state.push(9),
            |s: &mut Snapshot| s.stream_version += 1,
            |s: &mut Snapshot| s.projection_version += 1,
            |s: &mut Snapshot| s.stream_id = StreamId::from_name("b"),
        ] {
            let mut t = s.clone();
            tamper(&mut t);
            assert!(!t.verify());
        }
    }

    #[tokio::test]
    async fn a_valid_snapshot_means_only_the_tail_is_replayed() {
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[1, 2, 3, 4]).await;

        let first = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!((first.state, first.version, first.replayed), (10, 4, 4));
        assert!(snapshot_if_due(&snaps, &SumFold::<1>, stream, &first, 0, 4)
            .await
            .unwrap());
        assert!(
            !snapshot_if_due(&snaps, &SumFold::<1>, stream, &first, 4, 4)
                .await
                .unwrap()
        );

        fill(&store, stream, 4, &[5, 6]).await;
        let second = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!(second.state, 21);
        assert_eq!(second.source, LoadSource::Snapshot { at: 4 });
        assert_eq!(second.replayed, 2, "10000 old events would not be re-read");
    }

    #[tokio::test]
    async fn a_corrupt_snapshot_is_discarded_and_state_is_still_correct() {
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[1, 2, 3]).await;
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        snapshot_if_due(&snaps, &SumFold::<1>, stream, &l, 0, 1)
            .await
            .unwrap();
        snaps.corrupt_state(stream);

        let again = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!(again.state, 6);
        assert_eq!(
            again.source,
            LoadSource::FullReplay {
                discarded: Some(DiscardReason::BadChecksum)
            }
        );
        assert!(
            snaps.load_latest(stream).await.unwrap().is_none(),
            "bad row removed"
        );
    }

    #[tokio::test]
    async fn a_projection_version_bump_rebuilds_from_the_log() {
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[1, 2, 3]).await;
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        snapshot_if_due(&snaps, &SumFold::<1>, stream, &l, 0, 1)
            .await
            .unwrap();

        let v2 = load_state(&store, &snaps, &SumFold::<2>, stream)
            .await
            .unwrap();
        assert_eq!(v2.replayed, 3);
        assert_eq!(
            v2.source,
            LoadSource::FullReplay {
                discarded: Some(DiscardReason::ProjectionVersionChanged {
                    snapshot: 1,
                    current: 2
                })
            }
        );
    }

    #[tokio::test]
    async fn a_snapshot_ahead_of_the_log_is_not_trusted() {
        // E.g. the log was restored from an older backup.
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[1, 2]).await;
        snaps
            .save(Snapshot::new(stream, 99, 1, 1000u64.to_le_bytes().to_vec()))
            .await
            .unwrap();
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!(l.state, 3);
        assert!(matches!(
            l.source,
            LoadSource::FullReplay {
                discarded: Some(DiscardReason::BeyondStreamHead { .. })
            }
        ));
    }

    #[tokio::test]
    async fn an_undecodable_state_is_discarded_not_an_error() {
        let store = InMemoryEventStore::new();
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[4]).await;
        snaps
            .save(Snapshot::new(stream, 1, 1, vec![1, 2, 3]))
            .await
            .unwrap(); // not 8 bytes
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!(l.state, 4);
        assert!(matches!(
            l.source,
            LoadSource::FullReplay {
                discarded: Some(DiscardReason::UndecodableState)
            }
        ));
    }

    #[tokio::test]
    async fn older_snapshots_never_replace_newer_ones() {
        let snaps = InMemorySnapshotStore::new();
        let stream = StreamId::from_name("s");
        snaps
            .save(Snapshot::new(stream, 10, 1, vec![]))
            .await
            .unwrap();
        snaps
            .save(Snapshot::new(stream, 4, 1, vec![]))
            .await
            .unwrap();
        assert_eq!(
            snaps
                .load_latest(stream)
                .await
                .unwrap()
                .unwrap()
                .stream_version,
            10
        );
        assert_eq!(snaps.delete_stream(stream).await.unwrap(), 1);
        assert_eq!(snaps.delete_stream(stream).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn stoolap_snapshots_round_trip_supersede_and_detect_corruption() {
        let store = InMemoryEventStore::new();
        let snaps = StoolapSnapshotStore::open_in_memory().unwrap();
        let stream = StreamId::from_name("s");
        fill(&store, stream, 0, &[1, 2, 3]).await;

        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        snapshot_if_due(&snaps, &SumFold::<1>, stream, &l, 0, 1)
            .await
            .unwrap();
        let got = snaps.load_latest(stream).await.unwrap().unwrap();
        assert!(got.verify());
        assert_eq!(got.stream_version, 3);

        fill(&store, stream, 3, &[4]).await;
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!((l.state, l.replayed), (10, 1));
        snapshot_if_due(&snaps, &SumFold::<1>, stream, &l, 3, 1)
            .await
            .unwrap();
        assert_eq!(
            snaps
                .load_latest(stream)
                .await
                .unwrap()
                .unwrap()
                .stream_version,
            4
        );
        snaps
            .save(Snapshot::new(stream, 2, 1, vec![]))
            .await
            .unwrap();
        assert_eq!(
            snaps
                .load_latest(stream)
                .await
                .unwrap()
                .unwrap()
                .stream_version,
            4
        );

        snaps.corrupt_state(stream);
        let l = load_state(&store, &snaps, &SumFold::<1>, stream)
            .await
            .unwrap();
        assert_eq!(l.state, 10);
        assert!(matches!(
            l.source,
            LoadSource::FullReplay {
                discarded: Some(DiscardReason::BadChecksum)
            }
        ));
        assert!(snaps.load_latest(stream).await.unwrap().is_none());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// §38's central claim: a snapshot is only an optimization.
        /// With a snapshot taken at any point, state equals a full
        /// replay's.
        #[test]
        fn snapshot_plus_tail_equals_full_replay(
            vals in proptest::collection::vec(any::<u8>(), 1..40),
            cut in 0usize..40,
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            rt.block_on(async {
                let store = InMemoryEventStore::new();
                let stream = StreamId::from_name("p");
                let cut = cut.min(vals.len());
                fill(&store, stream, 0, &vals[..cut]).await;
                let snaps = InMemorySnapshotStore::new();
                let head = load_state(&store, &snaps, &SumFold::<1>, stream).await.unwrap();
                snapshot_if_due(&snaps, &SumFold::<1>, stream, &head, 0, 1).await.unwrap();
                fill(&store, stream, cut as u64, &vals[cut..]).await;

                let with = load_state(&store, &snaps, &SumFold::<1>, stream).await.unwrap();
                let without = load_state(&store, &InMemorySnapshotStore::new(), &SumFold::<1>, stream)
                    .await.unwrap();
                prop_assert_eq!(with.state, without.state);
                prop_assert_eq!(with.version, without.version);
                Ok(())
            })?;
        }
    }
}
