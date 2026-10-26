//! Device addresses learned from group invites and membership changes
//! (`peer_directory` table).
//!
//! Group members tell each other how to reach every member. That is only
//! useful if it is still known after a restart — otherwise a member who is
//! not also a saved contact falls out of reach every time the app is
//! closed. Contacts keep their own table; this one holds only what was
//! *learned*.
//!
//! Strings only: this crate has no dependency on the messaging types. The
//! ticket is stored as its own text form and re-validated by whoever loads
//! it, so a damaged row costs one entry, never the load.

use std::sync::Arc;

use stoolap::Database;

use crate::StorageError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPeerDevice {
    pub account_id: String,
    pub device_id: String,
    pub ticket_text: String,
}

pub trait PeerDirectoryRepository {
    /// Insert, or replace the ticket for an (account, device) already known.
    fn upsert(&self, peer: &StoredPeerDevice) -> Result<(), StorageError>;
    fn list(&self) -> Result<Vec<StoredPeerDevice>, StorageError>;
}

pub struct StoolapPeerDirectoryRepository {
    db: Arc<Database>,
}

impl StoolapPeerDirectoryRepository {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl PeerDirectoryRepository for StoolapPeerDirectoryRepository {
    fn upsert(&self, p: &StoredPeerDevice) -> Result<(), StorageError> {
        self.db
            .execute(
                "DELETE FROM peer_directory WHERE account_id = $1 AND device_id = $2",
                (p.account_id.clone(), p.device_id.clone()),
            )
            .map_err(StorageError::from_stoolap)?;
        self.db
            .execute(
                "INSERT INTO peer_directory (account_id, device_id, ticket_text) VALUES ($1, $2, $3)",
                (p.account_id.clone(), p.device_id.clone(), p.ticket_text.clone()),
            )
            .map_err(StorageError::from_stoolap)?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<StoredPeerDevice>, StorageError> {
        let rows = self
            .db
            .query(
                "SELECT account_id, device_id, ticket_text FROM peer_directory ORDER BY account_id, device_id",
                (),
            )
            .map_err(StorageError::from_stoolap)?;
        let mut out = Vec::new();
        for row in rows {
            let row = row.map_err(StorageError::from_stoolap)?;
            out.push(StoredPeerDevice {
                account_id: row.get(0).map_err(StorageError::from_stoolap)?,
                device_id: row.get(1).map_err(StorageError::from_stoolap)?,
                ticket_text: row.get(2).map_err(StorageError::from_stoolap)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(a: &str, d: &str, t: &str) -> StoredPeerDevice {
        StoredPeerDevice {
            account_id: a.into(),
            device_id: d.into(),
            ticket_text: t.into(),
        }
    }

    #[test]
    fn upsert_replaces_the_same_device_and_keeps_others() {
        let repo = StoolapPeerDirectoryRepository::new(crate::open_in_memory().unwrap());
        repo.upsert(&peer("a1", "d1", "old")).unwrap();
        repo.upsert(&peer("a1", "d2", "other device")).unwrap();
        repo.upsert(&peer("a2", "d1", "same device id, other account"))
            .unwrap();
        repo.upsert(&peer("a1", "d1", "new")).unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 3);
        assert!(listed.contains(&peer("a1", "d1", "new")));
        assert!(!listed.iter().any(|p| p.ticket_text == "old"));
    }

    #[test]
    fn entries_survive_reopening_the_database_file() {
        let dir = std::env::temp_dir().join(format!("siar-peerdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("siar.db");
        StoolapPeerDirectoryRepository::new(crate::open(path.to_str().unwrap()).unwrap())
            .upsert(&peer("a", "d", "t"))
            .unwrap();
        let again =
            StoolapPeerDirectoryRepository::new(crate::open(path.to_str().unwrap()).unwrap());
        assert_eq!(again.list().unwrap(), vec![peer("a", "d", "t")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
