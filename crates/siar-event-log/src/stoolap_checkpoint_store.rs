//! §17's durable counterpart to [`crate::projection::
//! InMemoryCheckpointStore`] — same `stoolap`-backed pattern
//! [`crate::stoolap_store::StoolapEventStore`] already established for
//! [`crate::store::EventStore`] itself, applied here to
//! [`crate::projection::ProjectionCheckpointStore`]: a real SQL table,
//! not a `HashMap` that evaporates on process exit.
//!
//! ## Why this exists
//!
//! Every `Projection` this workspace has so far
//! (`siar_messaging::projections::ConversationSummaryProjection`) is
//! itself in-memory, so pairing it with an in-memory checkpoint store
//! was a consistent, honest choice — losing the checkpoint and losing
//! the materialized state happen together, and a restart just replays
//! the whole (durable) event log from offset 0 to rebuild both, which
//! is correct, if wasteful for a long history. This module exists for
//! the day a `Projection` implementation IS itself durable (a real
//! `stoolap` table it materializes into, following `stoolap_store`'s
//! own schema pattern): pairing durable materialized state with an
//! in-memory checkpoint would be actively wrong — restarting would
//! resume "from where we left off" against state that's actually still
//! at whatever it was several thousand events ago, silently skipping
//! everything in between rather than either replaying or genuinely
//! resuming. No such durable projection exists in this workspace yet;
//! this module is the piece that would need to exist before one could
//! be built correctly, built ahead of that caller on the reasoning
//! that getting checkpoint durability right is more foundational than
//! any one projection's own schema.
//!
//! ## Schema
//!
//! One row per [`crate::projection::ProjectionId`] — `projection_id`
//! TEXT (a [`crate::projection::ProjectionId`] wraps a `&'static str`,
//! always a compile-time constant a projection author writes once, so
//! storing it as plain TEXT needs no encoding scheme the way
//! `stoolap_store`'s own `stream_id`/`event_id` columns do), plus
//! `last_log_offset`/`projection_version` as plain integers — no
//! checksum column: unlike an event (§22, immutable once written, so a
//! bit-rot check is meaningful forever), a checkpoint row is
//! overwritten on every single catch-up batch, and integrity here is
//! "is this the value I just wrote," which `stoolap`'s own storage
//! already has to get right for ANY column to work at all.
//!
//! ## Why `DELETE` + `INSERT`, not `UPDATE`
//!
//! Same reasoning `stoolap_store::append`'s own stream-head upsert
//! already documents: this module doesn't lean on `stoolap` supporting
//! any particular `UPDATE`/upsert SQL form correctly, since (per
//! `stoolap_store`'s own doc comment) it's new enough that this
//! workspace doesn't assume more of it than the plain
//! `CREATE TABLE`/`INSERT`/`SELECT`/`DELETE` forms already proven
//! working elsewhere in this crate.

use stoolap::Database;

use crate::projection::{
    ProjectionCheckpoint, ProjectionCheckpointStore, ProjectionError, ProjectionId,
};

/// See this module's own doc comment.
pub struct StoolapCheckpointStore {
    db: Database,
}

impl StoolapCheckpointStore {
    /// Opens (or creates) a file-backed checkpoint store at `path` and
    /// applies the schema. Same `"file://"`-prefixed DSN handling
    /// `StoolapEventStore::open`'s own doc comment explains.
    pub fn open(path: &str) -> Result<Self, ProjectionError> {
        let dsn = format!("file://{path}");
        let db = Database::open(&dsn).map_err(backend_err)?;
        Self::from_database(db)
    }

    /// In-memory — for tests, and for any caller that wants this
    /// trait's real schema/query behavior without a file on disk
    /// (mirrors `StoolapEventStore::open_in_memory`). Not the same
    /// thing as [`crate::projection::InMemoryCheckpointStore`]: that
    /// type is a bare `HashMap`; this one runs the same SQL this
    /// module would run against a real file, just against `stoolap`'s
    /// own in-memory backend — useful for testing this module ITSELF,
    /// not a lighter-weight alternative to it.
    pub fn open_in_memory() -> Result<Self, ProjectionError> {
        let db = Database::open_in_memory().map_err(backend_err)?;
        Self::from_database(db)
    }

    fn from_database(db: Database) -> Result<Self, ProjectionError> {
        apply_schema(&db)?;
        Ok(Self { db })
    }
}

fn apply_schema(db: &Database) -> Result<(), ProjectionError> {
    db.execute(
        "CREATE TABLE IF NOT EXISTS projection_checkpoints (
            projection_id      TEXT NOT NULL,
            last_log_offset    INTEGER NOT NULL,
            projection_version INTEGER NOT NULL
        )",
        (),
    )
    .map_err(backend_err)?;
    db.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_projection_checkpoints_id
         ON projection_checkpoints (projection_id)",
        (),
    )
    .map_err(backend_err)?;
    Ok(())
}

fn backend_err(e: impl std::fmt::Display) -> ProjectionError {
    ProjectionError::Checkpoint(e.to_string())
}

#[async_trait::async_trait]
impl ProjectionCheckpointStore for StoolapCheckpointStore {
    async fn load(
        &self,
        projection_id: ProjectionId,
    ) -> Result<Option<ProjectionCheckpoint>, ProjectionError> {
        let mut rows = self
            .db
            .query(
                "SELECT last_log_offset, projection_version
                 FROM projection_checkpoints WHERE projection_id = $1",
                (projection_id.0.to_string(),),
            )
            .map_err(backend_err)?;

        match rows.next() {
            Some(row) => {
                let row = row.map_err(backend_err)?;
                let last_log_offset: i64 = row.get(0).map_err(backend_err)?;
                let projection_version: i64 = row.get(1).map_err(backend_err)?;
                Ok(Some(ProjectionCheckpoint {
                    // Constructed from the CALLER's own `projection_id`
                    // (already `'static`), never from the TEXT column —
                    // a `&'static str` cannot be manufactured from an
                    // arbitrary runtime `String` without leaking memory
                    // for it forever. The column exists so a different
                    // process (or a debugger/DBA looking at the raw
                    // table) can see which projection a row belongs
                    // to; this method never needs to turn that text
                    // back into a `ProjectionId` itself, since the only
                    // way to reach this row was already having one.
                    projection_id,
                    last_log_offset: crate::ids::LocalLogOffset(last_log_offset as u64),
                    projection_version: projection_version as u16,
                }))
            }
            None => Ok(None),
        }
    }

    async fn save(&self, checkpoint: ProjectionCheckpoint) -> Result<(), ProjectionError> {
        let projection_id = checkpoint.projection_id.0.to_string();
        self.db
            .execute(
                "DELETE FROM projection_checkpoints WHERE projection_id = $1",
                (projection_id.clone(),),
            )
            .map_err(backend_err)?;
        self.db
            .execute(
                "INSERT INTO projection_checkpoints
                    (projection_id, last_log_offset, projection_version)
                 VALUES ($1, $2, $3)",
                (
                    projection_id,
                    checkpoint.last_log_offset.0 as i64,
                    checkpoint.projection_version as i64,
                ),
            )
            .map_err(backend_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::LocalLogOffset;

    #[tokio::test]
    async fn loading_an_unknown_projection_id_returns_none() {
        let store = StoolapCheckpointStore::open_in_memory().unwrap();
        let result = store.load(ProjectionId("never_saved")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn a_saved_checkpoint_round_trips() {
        let store = StoolapCheckpointStore::open_in_memory().unwrap();
        let checkpoint = ProjectionCheckpoint {
            projection_id: ProjectionId("conversation_summary"),
            last_log_offset: LocalLogOffset(42),
            projection_version: 3,
        };
        store.save(checkpoint).await.unwrap();

        let loaded = store
            .load(ProjectionId("conversation_summary"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded, checkpoint);
    }

    #[tokio::test]
    async fn saving_again_overwrites_rather_than_duplicating() {
        let store = StoolapCheckpointStore::open_in_memory().unwrap();
        let id = ProjectionId("conversation_summary");
        store
            .save(ProjectionCheckpoint {
                projection_id: id,
                last_log_offset: LocalLogOffset(1),
                projection_version: 1,
            })
            .await
            .unwrap();
        store
            .save(ProjectionCheckpoint {
                projection_id: id,
                last_log_offset: LocalLogOffset(2),
                projection_version: 1,
            })
            .await
            .unwrap();

        let loaded = store.load(id).await.unwrap().unwrap();
        assert_eq!(loaded.last_log_offset, LocalLogOffset(2));

        // No leftover duplicate row from the first save — a raw count
        // against the table itself, not just trusting `load`'s own
        // (unique-indexed, so also proof of this) query.
        let mut rows = store
            .db
            .query(
                "SELECT COUNT(*) FROM projection_checkpoints WHERE projection_id = $1",
                (id.0.to_string(),),
            )
            .unwrap();
        let count: i64 = rows.next().unwrap().unwrap().get(0).unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn different_projection_ids_never_cross_over() {
        let store = StoolapCheckpointStore::open_in_memory().unwrap();
        store
            .save(ProjectionCheckpoint {
                projection_id: ProjectionId("a"),
                last_log_offset: LocalLogOffset(5),
                projection_version: 1,
            })
            .await
            .unwrap();

        assert!(store.load(ProjectionId("b")).await.unwrap().is_none());
        assert_eq!(
            store
                .load(ProjectionId("a"))
                .await
                .unwrap()
                .unwrap()
                .last_log_offset,
            LocalLogOffset(5)
        );
    }

    /// Same real durability proof `stoolap_store`'s own
    /// `events_survive_closing_and_reopening_the_same_file` test makes
    /// for events: an actual `std::fs` round trip through a real file,
    /// not simulated in memory.
    #[tokio::test]
    async fn checkpoints_survive_closing_and_reopening_the_same_file() {
        let path = std::env::temp_dir().join(format!(
            "siar-event-log-checkpoint-durability-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path_str = path.to_str().unwrap().to_string();
        let id = ProjectionId("conversation_summary");

        {
            let store = StoolapCheckpointStore::open(&path_str).unwrap();
            store
                .save(ProjectionCheckpoint {
                    projection_id: id,
                    last_log_offset: LocalLogOffset(7),
                    projection_version: 2,
                })
                .await
                .unwrap();
        }

        {
            let store = StoolapCheckpointStore::open(&path_str).unwrap();
            let loaded = store.load(id).await.unwrap().unwrap();
            assert_eq!(loaded.last_log_offset, LocalLogOffset(7));
            assert_eq!(loaded.projection_version, 2);
        }

        let _ = std::fs::remove_file(&path);
    }
}
