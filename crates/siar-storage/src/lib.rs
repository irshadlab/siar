//! siar-storage: local persistence, backed by stoolap (plan.md §18–20).
//!
//! ============================ NOT YET FULLY VERIFIED =======================
//! First compiled against real `stoolap 0.4.0` source (previously this
//! module doc described stoolap's API from documentation/inference
//! alone, and got two things wrong that a compiler caught): `db.query`
//! returns `Rows`, an iterator of `Result<ResultRow>` — not the raw
//! `Row` type, which is what `ResultRow` wraps internally and has a
//! different (`Option`-returning, not `Result`-returning) `get` method.
//! And `stoolap::Value` has no blob variant at all — `ToParam`/
//! `FromValue` aren't implemented for `Vec<u8>`/`&[u8]`, so every binary
//! payload this crate stores goes in as base64 text via
//! `blob_codec::{encode_blob, decode_blob}` instead. Still not run —
//! `cargo test -p siar-storage` on your machine is the actual check.
//! ============================================================================
//!
//! Also flagged separately: stoolap is days old as a public release. The
//! durability guarantees this crate leans on (plan.md §16–17: "persist
//! before sending", crash-safe outbox) are only as good as stoolap's own
//! WAL/commit correctness, which hasn't had time to be battle-tested.
//! Treat this crate as prototype-grade until stoolap has more mileage.

mod blob_codec;
mod blob_repo;
mod contact_repo;
mod error;
mod group_repo;
mod message_repo;
mod outbox_repo;
mod peer_directory_repo;
mod schema;
mod security_event_repo;

pub use blob_repo::{BlobRepository, StoolapBlobRepository};
pub use contact_repo::{ContactRepository, StoolapContactRepository, StoredContact};
pub use error::StorageError;
pub use group_repo::{GroupRepository, StoolapGroupRepository};
pub use message_repo::{MessageRepository, StoolapMessageRepository, StoredMessage};
pub use outbox_repo::{OutboxOperation, OutboxRepository, StoolapOutboxRepository};
pub use peer_directory_repo::{
    PeerDirectoryRepository, StoolapPeerDirectoryRepository, StoredPeerDevice,
};
pub use security_event_repo::{
    SecurityEventRepository, StoolapSecurityEventRepository, StoredSecurityEvent,
};

use std::sync::Arc;
use stoolap::Database;

/// Opens (or creates) the local database and applies the schema.
/// One `Database` handle is meant to be shared (via `Arc`) across every
/// repository in the process — stoolap manages its own internal
/// concurrency (plan.md §18's "proven durability, transactions, indexes").
///
/// Takes a plain filesystem path (e.g. from `std::path::Path::display()`),
/// not a raw stoolap DSN — a real, previously-latent bug found while
/// wiring `apps/desktop`'s persistent contact book: `Database::open`
/// itself requires a `"file://<path>"`-prefixed DSN string (confirmed
/// against stoolap 0.4.0's own `parse_dsn`/`FILE_SCHEME` handling; a
/// bare path fails with "Unsupported scheme"), but this function was
/// passing `path` straight through unprefixed. Never caught before
/// because nothing in this workspace had called `open()` with a real
/// path until now — every prior caller used `open_in_memory()` instead,
/// which bypasses DSN parsing entirely via its own constructor.
pub fn open(path: &str) -> Result<Arc<Database>, StorageError> {
    let dsn = format!("file://{path}");
    let db = Database::open(&dsn).map_err(StorageError::from_stoolap)?;
    schema::apply(&db)?;
    Ok(Arc::new(db))
}

/// In-memory database for tests / the Phase-1 CLI's ephemeral runs.
pub fn open_in_memory() -> Result<Arc<Database>, StorageError> {
    let db = Database::open_in_memory().map_err(StorageError::from_stoolap)?;
    schema::apply(&db)?;
    Ok(Arc::new(db))
}
