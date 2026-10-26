//! Persistent security events (`security_events` table).
//!
//! Before this, the security log lived only in memory: every restart
//! emptied it, and any check that re-ran at launch (e.g. "this contact's
//! key changed") raised the same warning again. The user could resolve an
//! event and see it come back the next day.
//!
//! Plain strings and integers only — this crate does not depend on the UI
//! types. The *stable text names* for a kind/action live next to those
//! types (`SecurityEventKind::as_str`); this table just stores them, and a
//! name the running build doesn't recognise is returned as-is so the caller
//! can skip that row instead of the whole log failing to load.

use std::sync::Arc;

use stoolap::Database;

use crate::StorageError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSecurityEvent {
    pub event_id: u64,
    pub kind: String,
    pub occurred_at_millis: u64,
    pub resolved: bool,
    pub related_device: Option<String>,
    pub related_account: Option<String>,
    /// Comma-separated stable action names (empty = none).
    pub actions: String,
}

pub trait SecurityEventRepository {
    /// Insert or fully replace the row for `event.event_id`.
    fn upsert(&self, event: &StoredSecurityEvent) -> Result<(), StorageError>;
    /// Marks an event resolved. Unknown ids are a no-op, not an error.
    fn set_resolved(&self, event_id: u64) -> Result<(), StorageError>;
    /// Oldest first.
    fn list(&self) -> Result<Vec<StoredSecurityEvent>, StorageError>;
}

pub struct StoolapSecurityEventRepository {
    db: Arc<Database>,
}

impl StoolapSecurityEventRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl SecurityEventRepository for StoolapSecurityEventRepository {
    fn upsert(&self, e: &StoredSecurityEvent) -> Result<(), StorageError> {
        self.db
            .execute(
                "DELETE FROM security_events WHERE event_id = $1",
                (e.event_id as i64,),
            )
            .map_err(StorageError::from_stoolap)?;
        self.db
            .execute(
                "INSERT INTO security_events
                    (event_id, kind, occurred_at_millis, resolved, related_device, related_account, actions)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
                (
                    e.event_id as i64,
                    e.kind.clone(),
                    e.occurred_at_millis as i64,
                    i64::from(e.resolved),
                    e.related_device.clone(),
                    e.related_account.clone(),
                    e.actions.clone(),
                ),
            )
            .map_err(StorageError::from_stoolap)?;
        Ok(())
    }

    fn set_resolved(&self, event_id: u64) -> Result<(), StorageError> {
        self.db
            .execute(
                "UPDATE security_events SET resolved = 1 WHERE event_id = $1",
                (event_id as i64,),
            )
            .map_err(StorageError::from_stoolap)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<StoredSecurityEvent>, StorageError> {
        let rows = self
            .db
            .query(
                "SELECT event_id, kind, occurred_at_millis, resolved, related_device, related_account, actions
                 FROM security_events ORDER BY event_id",
                (),
            )
            .map_err(StorageError::from_stoolap)?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(StorageError::from_stoolap)?;
            let event_id: i64 = row.get(0).map_err(StorageError::from_stoolap)?;
            let resolved: i64 = row.get(3).map_err(StorageError::from_stoolap)?;
            let occurred: i64 = row.get(2).map_err(StorageError::from_stoolap)?;
            out.push(StoredSecurityEvent {
                event_id: event_id.max(0) as u64,
                kind: row.get(1).map_err(StorageError::from_stoolap)?,
                occurred_at_millis: occurred.max(0) as u64,
                resolved: resolved != 0,
                related_device: row.get(4).map_err(StorageError::from_stoolap)?,
                related_account: row.get(5).map_err(StorageError::from_stoolap)?,
                actions: row.get(6).map_err(StorageError::from_stoolap)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: u64) -> StoredSecurityEvent {
        StoredSecurityEvent {
            event_id: id,
            kind: "identity_changed".into(),
            occurred_at_millis: 1_000 + id,
            resolved: false,
            related_device: None,
            related_account: Some("11111111-1111-1111-1111-111111111111".into()),
            actions: "review,dismiss".into(),
        }
    }

    #[test]
    fn upsert_list_round_trips_in_id_order() {
        let repo = StoolapSecurityEventRepository::new(crate::open_in_memory().unwrap());
        repo.upsert(&event(2)).unwrap();
        repo.upsert(&event(1)).unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed, vec![event(1), event(2)]);
    }

    #[test]
    fn upsert_replaces_instead_of_duplicating() {
        let repo = StoolapSecurityEventRepository::new(crate::open_in_memory().unwrap());
        repo.upsert(&event(1)).unwrap();
        repo.upsert(&StoredSecurityEvent {
            resolved: true,
            ..event(1)
        })
        .unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].resolved);
    }

    #[test]
    fn set_resolved_flips_only_that_event_and_ignores_unknown_ids() {
        let repo = StoolapSecurityEventRepository::new(crate::open_in_memory().unwrap());
        repo.upsert(&event(1)).unwrap();
        repo.upsert(&event(2)).unwrap();
        repo.set_resolved(2).unwrap();
        repo.set_resolved(99).unwrap();
        let listed = repo.list().unwrap();
        assert!(!listed[0].resolved && listed[1].resolved);
    }

    #[test]
    fn nullable_columns_and_empty_actions_survive() {
        let repo = StoolapSecurityEventRepository::new(crate::open_in_memory().unwrap());
        let bare = StoredSecurityEvent {
            related_account: None,
            related_device: Some("dev".into()),
            actions: String::new(),
            ..event(5)
        };
        repo.upsert(&bare).unwrap();
        assert_eq!(repo.list().unwrap(), vec![bare]);
    }

    #[test]
    fn events_survive_closing_and_reopening_the_database_file() {
        let dir = std::env::temp_dir().join(format!("siar-secev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("siar.db");
        {
            let repo =
                StoolapSecurityEventRepository::new(crate::open(path.to_str().unwrap()).unwrap());
            repo.upsert(&event(7)).unwrap();
            repo.set_resolved(7).unwrap();
        }
        let repo =
            StoolapSecurityEventRepository::new(crate::open(path.to_str().unwrap()).unwrap());
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].resolved);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
