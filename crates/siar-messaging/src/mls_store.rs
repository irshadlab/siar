//! Where a device's MLS group state lives on disk, and how a restarting
//! `GroupService` finds it again.
//!
//! openmls can reload a group *given its id*, but it cannot list the
//! groups in a database, and `siar-crypto-mls` deliberately left "where
//! does the file live / how do we rediscover conversations" to the
//! application layer (see `siar-crypto-mls::persistent`). This is that
//! layer:
//!
//! * one SQLite file per group (`<conversation>.sqlite3`) or per
//!   not-yet-used key package (`kp-<uuid>.sqlite3`), so one corrupt file
//!   costs one group, not all of them;
//! * `index.txt` beside them, mapping each conversation to its file, MLS
//!   group id and signing *public* key — the two public values
//!   `MlsGroupSession::load` needs. No secret is ever written to the
//!   index; key material lives only inside the SQLite files.
//!
//! The index is plain text, one record per line, rewritten atomically
//! (write temp, rename). A damaged line is skipped, so a bad index loses
//! the groups it named, never the whole device.

use std::path::{Path, PathBuf};

use siar_domain::ConversationId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRecord {
    pub conversation: ConversationId,
    pub file: String,
    pub group_id: Vec<u8>,
    pub signature_public_key: Vec<u8>,
}

/// A key package this device published but nobody has used yet. Kept so
/// the SAME package stays valid across restarts: a key package already
/// handed to someone out of band is useless if its private half died with
/// the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRecord {
    pub file: String,
    pub signature_public_key: Vec<u8>,
    pub key_package: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Index {
    pub groups: Vec<GroupRecord>,
    pub pending: Option<PendingRecord>,
}

#[derive(Debug, Clone)]
pub struct MlsStore {
    dir: PathBuf,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// File names come from our own records but are read back from disk, so
/// refuse anything that could point outside the directory.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".sqlite3")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.contains("..")
}

impl MlsStore {
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// A private scratch directory, for callers that do not want
    /// persistence (tests, the CLI). Never reused between processes.
    pub fn temporary() -> std::io::Result<Self> {
        Self::new(std::env::temp_dir().join(format!("siar-mls-{}", ConversationId::new())))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_of(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.txt")
    }

    pub fn load(&self) -> Index {
        let mut index = Index::default();
        let Ok(text) = std::fs::read_to_string(self.index_path()) else {
            return index;
        };
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                ["group", conv, file, gid, public] => {
                    let (Ok(conversation), Some(group_id), Some(signature_public_key)) =
                        (conv.parse::<ConversationId>(), unhex(gid), unhex(public))
                    else {
                        continue;
                    };
                    if safe_file_name(file) {
                        index.groups.retain(|g| g.conversation != conversation);
                        index.groups.push(GroupRecord {
                            conversation,
                            file: (*file).to_string(),
                            group_id,
                            signature_public_key,
                        });
                    }
                }
                ["pending", file, public, kp] => {
                    let (Some(signature_public_key), Some(key_package)) =
                        (unhex(public), unhex(kp))
                    else {
                        continue;
                    };
                    if safe_file_name(file) {
                        index.pending = Some(PendingRecord {
                            file: (*file).to_string(),
                            signature_public_key,
                            key_package,
                        });
                    }
                }
                _ => {}
            }
        }
        index
    }

    fn save(&self, index: &Index) -> std::io::Result<()> {
        let mut lines: Vec<String> = index
            .groups
            .iter()
            .map(|g| {
                format!(
                    "group {} {} {} {}",
                    g.conversation,
                    g.file,
                    hex(&g.group_id),
                    hex(&g.signature_public_key)
                )
            })
            .collect();
        lines.sort();
        if let Some(p) = &index.pending {
            lines.push(format!(
                "pending {} {} {}",
                p.file,
                hex(&p.signature_public_key),
                hex(&p.key_package)
            ));
        }
        let tmp = self.dir.join("index.tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(&tmp, self.index_path())
    }

    pub fn record_group(&self, record: GroupRecord) -> std::io::Result<()> {
        let mut index = self.load();
        index
            .groups
            .retain(|g| g.conversation != record.conversation);
        index.groups.push(record);
        self.save(&index)
    }

    pub fn record_pending(&self, record: PendingRecord) -> std::io::Result<()> {
        let mut index = self.load();
        index.pending = Some(record);
        self.save(&index)
    }

    pub fn clear_pending(&self) -> std::io::Result<()> {
        let mut index = self.load();
        index.pending = None;
        self.save(&index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u8) -> GroupRecord {
        GroupRecord {
            conversation: ConversationId::new(),
            file: format!("g{n}.sqlite3"),
            group_id: vec![n, 1, 2, 3],
            signature_public_key: vec![n, 9, 9],
        }
    }

    #[test]
    fn groups_and_pending_round_trip_and_replace_by_key() {
        let store = MlsStore::temporary().unwrap();
        let (a, b) = (group(1), group(2));
        store.record_group(a.clone()).unwrap();
        store.record_group(b.clone()).unwrap();
        let pending = PendingRecord {
            file: "kp-1.sqlite3".into(),
            signature_public_key: vec![7, 7],
            key_package: vec![0xde, 0xad],
        };
        store.record_pending(pending.clone()).unwrap();

        let loaded = MlsStore::new(store.dir()).unwrap().load();
        assert_eq!(loaded.groups.len(), 2);
        assert!(loaded.groups.contains(&a) && loaded.groups.contains(&b));
        assert_eq!(loaded.pending, Some(pending));

        // re-recording the same conversation replaces, never duplicates
        let a2 = GroupRecord {
            file: "other.sqlite3".into(),
            ..a
        };
        store.record_group(a2.clone()).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.groups.len(), 2);
        assert!(loaded.groups.contains(&a2));

        store.clear_pending().unwrap();
        assert_eq!(store.load().pending, None);
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_damaged_index_loses_only_the_damaged_lines() {
        let store = MlsStore::temporary().unwrap();
        let good = group(3);
        store.record_group(good.clone()).unwrap();
        let mut text = std::fs::read_to_string(store.dir().join("index.txt")).unwrap();
        text.push_str("garbage line\ngroup not-a-uuid f.sqlite3 00 00\ngroup\n");
        text.push_str(&format!(
            "group {} ../../etc/passwd 00 00\n",
            ConversationId::new()
        ));
        text.push_str(&format!(
            "group {} g.sqlite3 zz 00\n",
            ConversationId::new()
        ));
        std::fs::write(store.dir().join("index.txt"), text).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.groups, vec![good], "only the valid line survives");
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_missing_index_is_an_empty_one() {
        let store = MlsStore::temporary().unwrap();
        assert_eq!(store.load(), Index::default());
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[test]
    fn file_names_cannot_escape_the_directory() {
        assert!(safe_file_name("kp-1a2b.sqlite3"));
        assert!(!safe_file_name("../x.sqlite3"));
        assert!(!safe_file_name("a/b.sqlite3"));
        assert!(!safe_file_name("x.txt"));
        assert!(!safe_file_name(""));
    }
}
