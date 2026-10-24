//! Real disk persistence for MLS group state — closes the gap this
//! crate's own `lib.rs` doc comment has flagged since the first MLS
//! delivery: `OpenMlsRustCrypto`'s bundled storage
//! (`openmls_memory_storage::MemoryStorage`) is in-memory only, so a
//! process restart lost every group's live crypto state.
//!
//! ## A genuinely separate database, not a shared connection
//!
//! An earlier version of this crate's docs speculated about "deciding
//! how this composes with `siar-storage`'s own SQLite connection" —
//! that was **wrong**, caught while actually building this rather than
//! left uncorrected: `siar-storage` is backed by `stoolap`
//! (`crates/siar-storage/Cargo.toml`), not SQLite/`rusqlite` at all.
//! There is no existing SQLite connection to share. `SqlitePersistentProvider`
//! opens its own, separate SQLite file dedicated to MLS state. Two
//! embedded databases in one application is a real trade-off, not
//! ideal — but it's the honest one, not a "unify them" plan that was
//! never actually checked against what `siar-storage` runs on.
//!
//! ## What "real persistence" means here, precisely
//!
//! `openmls_sqlite_storage::SqliteStorageProvider` implements
//! `openmls_traits::storage::StorageProvider` — every signing key,
//! epoch secret, proposal, and piece of group state `MlsGroupSession`
//! touches now round-trips through SQLite instead of an in-memory
//! `HashMap`-equivalent. What this does NOT give you:
//!
//! - **No wiring into `GroupService` yet.** `GroupService`'s
//!   `mls_sessions`/`pending_identity` fields still use the default
//!   `MlsGroupSession`/`OpenMlsRustCrypto` (in-memory) — nothing
//!   currently constructs an `MlsGroupSession<SqlitePersistentProvider>`
//!   anywhere in `siar-messaging`. That's a real follow-up: deciding
//!   where the `.sqlite3` file lives on disk, how/whether one file
//!   holds every conversation's state or each gets its own, and how a
//!   restarting `GroupService` rediscovers which conversations have a
//!   persisted session to reopen — none of that is a `siar-crypto-mls`
//!   decision, it's an application-layer one, so it isn't made here.
//! - **No migration path for already-in-memory sessions.** A group
//!   created with `MlsGroupSession<OpenMlsRustCrypto>` (the default)
//!   can't be converted to a persistent one after the fact — nothing
//!   here attempts that.
//! - **No compiler-verified tests**, same caveat as this crate's other
//!   modules and for the same reason (see `lib.rs`'s top doc comment):
//!   correctness here rests on the openmls/rusqlite source checked
//!   below, not on having actually run.

use openmls::prelude::OpenMlsProvider;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PersistentProviderError {
    #[error("failed to open the MLS state database: {0}")]
    Open(String),
    #[error("failed to run MLS state database migrations: {0}")]
    Migrate(String),
}

/// `openmls_sqlite_storage::Codec` implemented with `serde_json` — see
/// this crate's `Cargo.toml` for why not postcard (it cannot read back
/// some openmls types). `Codec::Error` just needs `std::error::Error +
/// Debug + Send + Sync + 'static`, which `serde_json::Error` satisfies.
#[derive(Debug, Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    type Error = serde_json::Error;

    fn to_vec<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn from_slice<T: serde::de::DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

/// An `OpenMlsProvider` backed by real SQLite storage instead of
/// `OpenMlsRustCrypto`'s in-memory default — composed from two
/// upstream pieces rather than reimplemented: `openmls_rust_crypto`'s
/// `RustCrypto` (confirmed from that crate's own `lib.rs` to be the
/// same type `OpenMlsRustCrypto` uses for both its `CryptoProvider` and
/// `RandProvider` — reused here unchanged, since swapping the storage
/// backend has nothing to do with the crypto/randomness backend) for
/// crypto and randomness, and `openmls_sqlite_storage::SqliteStorageProvider`
/// for storage.
pub struct SqlitePersistentProvider {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<JsonCodec, Connection>,
}

impl SqlitePersistentProvider {
    /// Opens (creating if needed — `rusqlite::Connection::open`'s own
    /// behavior, confirmed from `rusqlite`'s source) the SQLite file at
    /// `path` and runs `openmls_sqlite_storage`'s bundled schema
    /// migrations (`SqliteStorageProvider::run_migrations`, required
    /// before first use per that crate's own doc comment). Safe to call
    /// again on an already-migrated file — `refinery` migrations are
    /// idempotent by design (tracked via a migration-history table,
    /// same principle `siar-storage`'s own schema versioning follows,
    /// different mechanism).
    pub fn open(path: &Path) -> Result<Self, PersistentProviderError> {
        let connection =
            Connection::open(path).map_err(|e| PersistentProviderError::Open(format!("{e:?}")))?;
        let mut storage = SqliteStorageProvider::<JsonCodec, Connection>::new(connection);
        storage
            .run_migrations()
            .map_err(|e| PersistentProviderError::Migrate(format!("{e:?}")))?;

        Ok(Self {
            crypto: RustCrypto::default(),
            storage,
        })
    }
}

impl OpenMlsProvider for SqlitePersistentProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<JsonCodec, Connection>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}
