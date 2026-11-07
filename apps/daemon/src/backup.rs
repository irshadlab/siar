//! §113-§115 for the daemon: consistent backup, staged restore.
//!
//! **What "consistent" means here, precisely.** The database is *never
//! copied live*. stoolap's own `create_snapshot()` writes an immutable,
//! timestamped set of per-table `.bin` files under `siar.db/snapshots/`
//! (plus a DDL file and a manifest); the archive contains **only those**
//! plus the small non-database files (identity, ids, MLS state). The live
//! write-ahead log and volumes — which stoolap's background checkpoint
//! thread rewrites at any moment — are excluded and never even opened, so a
//! backup cannot capture a half-checkpointed store.
//!
//! What is *not* guaranteed: the app core has no write barrier (nothing
//! outside the runtime tests reads `network_open()`), so messages that
//! arrive while the snapshot is being taken land either in it or after it.
//! Whether stoolap's snapshot is atomic *across tables* under concurrent
//! writes is not documented upstream and is not tested here; the restore
//! test below runs with writers idle.
//!
//! **Restore** cannot swap files under an open database, so it is two-phase:
//! [`DaemonBackup::restore`] decrypts and fully validates the archive into a
//! sibling staging directory and drops a marker; the swap happens at the
//! **next start** ([`apply_pending_restore`]), before the database is
//! opened. The swap never deletes anything: the previous contents move to a
//! `<data>.pre-restore-<ms>` sibling, and any failure rolls back.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use siar_backup::{InstanceLock, KdfParams};
use siar_runtime::backup::{BackupError, BackupId, BackupService, BackupTarget};
use siar_runtime::config::SecretRef;
use siar_runtime::runtime::BoxFuture;

const DB_DIR: &str = "siar.db";
const ARCHIVE_PREFIX: &str = "siar-backup-";
const ARCHIVE_EXT: &str = ".siarbk";
const STAGING_SUFFIX: &str = ".restore-staging";
const MARKER_SUFFIX: &str = ".restore-pending";
const LOCK_FILE: &str = "siar.lock";
/// Entries a restore must leave exactly where they are: the lock we hold, and
/// the audit log — restoring an older backup must never rewind or erase the
/// record of what administrators did (including the restore itself).
const KEEP_IN_PLACE: [&str; 2] = [LOCK_FILE, "audit"];

fn keep_in_place(name: &std::ffi::OsStr) -> bool {
    KEEP_IN_PLACE.iter().any(|k| name == *k)
}

/// Which files of a **live** data folder go in the archive.
#[must_use]
pub fn include_in_live_backup(rel: &str) -> bool {
    if rel == LOCK_FILE || rel.starts_with("crash/") || rel.starts_with("audit/") {
        return false;
    }
    if let Some(inside) = rel.strip_prefix("siar.db/") {
        return inside.starts_with("snapshots/");
    }
    true
}

fn svc<E: std::fmt::Display>(e: E) -> BackupError {
    BackupError::Service(e.to_string())
}

fn sibling(data_dir: &Path, suffix: &str) -> Result<PathBuf> {
    let name = data_dir
        .file_name()
        .ok_or_else(|| anyhow!("data dir has no name"))?
        .to_string_lossy()
        .into_owned();
    Ok(data_dir.with_file_name(format!("{name}{suffix}")))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn private_dir(p: &Path) -> std::io::Result<()> {
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The daemon's [`BackupService`].
#[derive(Clone)]
pub struct DaemonBackup {
    data_dir: PathBuf,
    /// The lock this process holds on the data folder. Without it we cannot
    /// prove nobody else is writing, so we refuse (`None` = couldn't lock).
    lock: Option<Arc<InstanceLock>>,
    /// Filled once the core has opened the database.
    db: Arc<OnceLock<Arc<siar_storage::Database>>>,
    passphrase: Option<SecretRef>,
}

impl DaemonBackup {
    #[must_use]
    pub fn new(
        data_dir: PathBuf,
        lock: Option<Arc<InstanceLock>>,
        passphrase: Option<SecretRef>,
    ) -> Self {
        Self {
            data_dir,
            lock,
            db: Arc::default(),
            passphrase,
        }
    }

    /// Hand this to the startup barrier to fill with `boot.db`.
    #[must_use]
    pub fn db_slot(&self) -> Arc<OnceLock<Arc<siar_storage::Database>>> {
        self.db.clone()
    }

    fn passphrase(&self) -> Result<String, BackupError> {
        let r = self.passphrase.as_ref().ok_or_else(|| {
            svc("no backup passphrase configured (secrets.backup_passphrase in daemon.toml)")
        })?;
        let secret = r.resolve().map_err(svc)?;
        let text = String::from_utf8(secret.expose().to_vec())
            .map_err(|_| svc("backup passphrase is not UTF-8"))?;
        Ok(text.trim_end_matches(['\n', '\r']).to_owned())
    }

    fn create_blocking(&self, target: &BackupTarget) -> Result<BackupId, BackupError> {
        let lock = self
            .lock
            .as_ref()
            .ok_or_else(|| svc("this daemon does not hold the data-folder lock"))?;
        let db = self
            .db
            .get()
            .ok_or_else(|| svc("the database is not open yet"))?;
        let pass = self.passphrase()?;
        private_dir(target.path()).map_err(svc)?;

        // Fresh, immutable snapshot set; the archive reads only those files.
        db.create_snapshot().map_err(svc)?;

        let created = now_ms();
        let out = target
            .path()
            .join(format!("{ARCHIVE_PREFIX}{created:020}{ARCHIVE_EXT}"));
        let summary = siar_backup::create_backup_while_locked(
            &self.data_dir,
            &pass,
            &out,
            KdfParams::DEFAULT,
            lock,
            &include_in_live_backup,
        )
        .map_err(svc)?;
        Ok(BackupId(u128::from(summary.created_at_millis)))
    }

    fn restore_blocking(&self, source: &BackupTarget) -> Result<(), BackupError> {
        let pass = self.passphrase()?;
        let archive = locate_archive(source.path()).map_err(svc)?;
        let staging = sibling(&self.data_dir, STAGING_SUFFIX).map_err(svc)?;
        let marker = sibling(&self.data_dir, MARKER_SUFFIX).map_err(svc)?;
        if staging.exists() {
            fs::remove_dir_all(&staging).map_err(svc)?; // a stale, never-applied stage
        }
        // Decrypts and validates everything (paths, sizes, required files)
        // before writing; on failure the staging dir is left untouched/absent.
        siar_backup::restore_backup(&archive, &pass, &staging).map_err(svc)?;
        write_marker(&marker).map_err(svc)?;
        Ok(())
    }
}

/// `source` may be one archive file, or a directory (the newest archive in
/// it — names embed a zero-padded timestamp, so lexical order is time order).
fn locate_archive(source: &Path) -> Result<PathBuf> {
    if source.is_file() {
        return Ok(source.to_owned());
    }
    let mut found: Vec<String> = fs::read_dir(source)
        .with_context(|| "backup source is neither a file nor a readable directory")?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(ARCHIVE_PREFIX) && n.ends_with(ARCHIVE_EXT))
        .collect();
    found.sort();
    let newest = found
        .pop()
        .ok_or_else(|| anyhow!("no backup archive found there"))?;
    Ok(source.join(newest))
}

fn write_marker(path: &Path) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(b"siar-restore-v1\n")?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)
}

impl BackupService for DaemonBackup {
    fn create_backup<'a>(
        &'a self,
        target: &'a BackupTarget,
    ) -> BoxFuture<'a, Result<BackupId, BackupError>> {
        let (this, target) = (self.clone(), target.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || this.create_blocking(&target))
                .await
                .map_err(svc)?
        })
    }

    fn restore<'a>(&'a self, source: &'a BackupTarget) -> BoxFuture<'a, Result<(), BackupError>> {
        let (this, source) = (self.clone(), source.clone());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || this.restore_blocking(&source))
                .await
                .map_err(svc)?
        })
    }

    /// Part 09 reconciliation of a restored store happens when the restore
    /// is *applied* (next start, before the core opens the database) — there
    /// is nothing to reconcile in this process, whose database still holds
    /// the pre-restore data.
    fn reconcile<'a>(&'a self) -> BoxFuture<'a, Result<(), BackupError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Apply a staged restore, if one is pending. **Call at startup, holding the
/// data-folder lock, before the database is opened.**
///
/// Returns the directory the previous contents were moved to. Nothing is
/// deleted; on any failure the previous contents are put back.
pub fn apply_pending_restore(data_dir: &Path, held: &InstanceLock) -> Result<Option<PathBuf>> {
    apply_with(data_dir, held, &|_| Ok(()))
}

/// Points in the swap where a test can inject a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Every previous entry has been moved to the `.pre-restore-*` folder.
    MovedAway,
    /// Every restored entry has been placed in the data folder.
    Placed,
    /// The database was rebuilt from the restored snapshot set and closed.
    DbRestored,
}

fn apply_with(
    data_dir: &Path,
    _held: &InstanceLock,
    hook: &dyn Fn(Phase) -> Result<()>,
) -> Result<Option<PathBuf>> {
    let marker = sibling(data_dir, MARKER_SUFFIX)?;
    if !marker.exists() {
        return Ok(None);
    }
    let staging = sibling(data_dir, STAGING_SUFFIX)?;
    if !staging.is_dir() {
        let _ = fs::remove_file(&marker);
        bail!("a restore was pending but its staging directory is gone; nothing was changed");
    }
    let previous = sibling(data_dir, &format!(".pre-restore-{}", now_ms()))?;
    fs::create_dir_all(&previous)?;

    let mut moved_away: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut placed: Vec<PathBuf> = Vec::new();
    let result = (|| -> Result<()> {
        for entry in fs::read_dir(data_dir)? {
            let entry = entry?;
            if keep_in_place(&entry.file_name()) {
                continue; // the lock we hold, and the audit log
            }
            let from = entry.path();
            let to = previous.join(entry.file_name());
            fs::rename(&from, &to).with_context(|| format!("moving {from:?} aside"))?;
            moved_away.push((from, to));
        }
        hook(Phase::MovedAway)?;
        for entry in fs::read_dir(&staging)? {
            let entry = entry?;
            if keep_in_place(&entry.file_name()) {
                continue; // an older archive may carry one; never overwrite the live one
            }
            let to = data_dir.join(entry.file_name());
            fs::rename(entry.path(), &to)
                .with_context(|| format!("placing restored {:?}", entry.file_name()))?;
            placed.push(to);
        }
        hook(Phase::Placed)?;
        // Rebuild the database from the restored snapshot set, then close it
        // so the daemon's own open starts clean.
        let db_path = data_dir.join(DB_DIR);
        let db = siar_storage::open(&db_path.display().to_string())
            .map_err(|e| anyhow!("opening restored database: {e}"))?;
        db.restore_snapshot(None)
            .map_err(|e| anyhow!("restoring database snapshot: {e}"))?;
        db.close()
            .map_err(|e| anyhow!("closing restored database: {e}"))?;
        hook(Phase::DbRestored)?;
        Ok(())
    })();

    if let Err(e) = result {
        // Hand the restored entries BACK to the staging folder so the staged
        // restore stays valid for a retry (deleting them would leave the
        // marker pointing at an empty stage). The database folder is trimmed
        // to what was staged — opening it created a write-ahead log and lock
        // files that were never part of the archive.
        for p in placed.iter().rev() {
            if p.file_name().is_some_and(|n| n == DB_DIR) {
                if let Ok(rd) = fs::read_dir(p) {
                    for child in rd.flatten() {
                        if child.file_name() != "snapshots" {
                            let cp = child.path();
                            let _ = if cp.is_dir() {
                                fs::remove_dir_all(&cp)
                            } else {
                                fs::remove_file(&cp)
                            };
                        }
                    }
                }
            }
            if let Some(name) = p.file_name() {
                fs::rename(p, staging.join(name)).with_context(|| {
                    format!(
                        "returning {p:?} to the staging folder (previous data is in {previous:?})"
                    )
                })?;
            }
        }
        // Whatever the failed attempt created in the data dir (the database
        // folder it opened) goes too, then the originals come back.
        for entry in fs::read_dir(data_dir)?.flatten() {
            if !keep_in_place(&entry.file_name()) {
                let p = entry.path();
                let _ = if p.is_dir() {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
            }
        }
        for (orig, bak) in moved_away.iter().rev() {
            fs::rename(bak, orig).with_context(|| {
                format!("rolling back {orig:?} (previous data is in {previous:?})")
            })?;
        }
        let _ = fs::remove_dir(&previous);
        return Err(e.context("restore failed; the previous data was put back"));
    }
    let _ = fs::remove_file(&marker);
    let _ = fs::remove_dir_all(&staging);
    Ok(Some(previous))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-dbk-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn passfile(dir: &Path) -> SecretRef {
        let p = dir.join("pass");
        fs::write(&p, "correct horse battery staple\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        SecretRef::File { path: p }
    }

    fn seed_data(data: &Path, identity: u8) {
        fs::create_dir_all(data.join("mls")).unwrap();
        fs::write(data.join("identity.bin"), [identity; 64]).unwrap();
        fs::write(data.join("account_id.txt"), "acct-1").unwrap();
        fs::write(data.join("device_id.txt"), "dev-1").unwrap();
        fs::write(data.join("mls/index.txt"), "g1").unwrap();
    }

    fn values(db: &siar_storage::Database) -> Vec<String> {
        let rows = db.query("SELECT v FROM kv ORDER BY id", ()).unwrap();
        rows.map(|r| r.unwrap().get::<String>(0).unwrap()).collect()
    }

    fn values_of(db: &siar_storage::Database, table: &str) -> Vec<String> {
        let rows = db
            .query(&format!("SELECT v FROM {table} ORDER BY id"), ())
            .unwrap();
        rows.map(|r| r.unwrap().get::<String>(0).unwrap()).collect()
    }

    fn open_db(data: &Path) -> Arc<siar_storage::Database> {
        siar_storage::open(&data.join(DB_DIR).display().to_string()).unwrap()
    }

    #[test]
    fn the_live_filter_keeps_snapshots_and_drops_live_database_files() {
        for yes in [
            "identity.bin",
            "mls/index.txt",
            "siar.db/snapshots/t/snapshot-1.bin",
            "siar.db/snapshots/manifest-1.json",
        ] {
            assert!(include_in_live_backup(yes), "{yes}");
        }
        for no in [
            "siar.lock",
            "crash/crash-1.txt",
            "audit/audit.log",
            "siar.db/wal/wal-1.log",
            "siar.db/db.lock",
            "siar.db/volumes/x.vol",
        ] {
            assert!(!include_in_live_backup(no), "{no}");
        }
    }

    #[tokio::test]
    async fn backup_then_staged_restore_brings_back_the_database_and_files() {
        let root = tmp("e2e");
        let data = root.join("data");
        seed_data(&data, 7);
        fs::create_dir_all(data.join("audit")).unwrap();
        fs::write(
            data.join("audit/audit.log"),
            "history that a restore must not touch",
        )
        .unwrap();
        let lock = Arc::new(InstanceLock::acquire(&data).unwrap());
        let db = open_db(&data);
        db.execute("CREATE TABLE kv(id INTEGER PRIMARY KEY, v TEXT)", ())
            .unwrap();
        db.execute("INSERT INTO kv VALUES (1, 'before')", ())
            .unwrap();

        let svc = DaemonBackup::new(data.clone(), Some(lock.clone()), Some(passfile(&root)));
        // Not usable until the core has opened the database.
        let out_dir = root.join("backups");
        let target = BackupTarget::new(out_dir.clone()).unwrap();
        assert!(
            matches!(svc.create_backup(&target).await, Err(BackupError::Service(m)) if m.contains("not open"))
        );
        svc.db_slot().set(db.clone()).ok().unwrap();

        // ── back up while the daemon (lock + open DB) is running ────────────
        let id = svc.create_backup(&target).await.unwrap();
        assert!(id.0 > 0);
        let archives: Vec<_> = fs::read_dir(&out_dir).unwrap().flatten().collect();
        assert_eq!(archives.len(), 1);

        // The archive contains snapshots and no live WAL.
        let probe = root.join("probe");
        siar_backup::restore_backup(&archives[0].path(), "correct horse battery staple", &probe)
            .unwrap();
        assert!(probe.join("siar.db/snapshots").is_dir());
        assert!(
            !probe.join("siar.db/wal").exists(),
            "live WAL must not be archived"
        );
        assert!(
            !probe.join("audit").exists(),
            "the audit log is not archived"
        );
        assert!(probe.join("identity.bin").is_file());

        // ── later, things change… ───────────────────────────────────────────
        db.execute("INSERT INTO kv VALUES (2, 'after')", ())
            .unwrap();
        fs::write(data.join("identity.bin"), [9u8; 64]).unwrap();
        assert_eq!(values(&db), ["before", "after"]);

        // ── stage the restore (what `ctl restore` does) ─────────────────────
        // Not allowed to proceed without the right passphrase.
        let wrong = DaemonBackup::new(data.clone(), Some(lock.clone()), {
            let p = root.join("wrongpass");
            fs::write(&p, "a totally different passphrase").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
            }
            Some(SecretRef::File { path: p })
        });
        assert!(wrong.restore(&target).await.is_err());
        assert!(
            !sibling(&data, MARKER_SUFFIX).unwrap().exists(),
            "a failed stage leaves no marker"
        );

        svc.restore(&target).await.unwrap();
        svc.reconcile().await.unwrap();
        assert!(sibling(&data, MARKER_SUFFIX).unwrap().exists());
        // Nothing has changed yet: the running DB still has both rows.
        assert_eq!(values(&db), ["before", "after"]);

        // ── "restart": close the DB, then apply before reopening ────────────
        db.close().unwrap();
        drop(db);
        let previous = apply_pending_restore(&data, &lock)
            .unwrap()
            .expect("a restore was pending");

        let db = open_db(&data);
        assert_eq!(
            values(&db),
            ["before"],
            "the database is back to the backed-up state"
        );
        assert_eq!(fs::read(data.join("identity.bin")).unwrap(), vec![7u8; 64]);
        // The audit log was neither rewound nor moved aside.
        assert_eq!(
            fs::read_to_string(data.join("audit/audit.log")).unwrap(),
            "history that a restore must not touch"
        );
        assert!(!previous.join("audit").exists());
        // Nothing was destroyed: the pre-restore state is kept.
        assert_eq!(
            fs::read(previous.join("identity.bin")).unwrap(),
            vec![9u8; 64]
        );
        assert!(previous.join(DB_DIR).is_dir());
        assert!(!sibling(&data, MARKER_SUFFIX).unwrap().exists());
        assert!(!sibling(&data, STAGING_SUFFIX).unwrap().exists());
        assert!(
            data.join(LOCK_FILE).exists(),
            "the lock file is never moved"
        );
        // A second call is a no-op.
        db.close().unwrap();
        drop(db);
        assert!(apply_pending_restore(&data, &lock).unwrap().is_none());
    }

    #[test]
    fn refuses_without_the_prerequisites_and_with_bad_sources() {
        let root = tmp("prereq");
        let data = root.join("data");
        seed_data(&data, 1);
        let target = BackupTarget::new(root.join("out")).unwrap();

        // No lock held by this process.
        let no_lock = DaemonBackup::new(data.clone(), None, Some(passfile(&root)));
        assert!(
            matches!(no_lock.create_blocking(&target), Err(BackupError::Service(m)) if m.contains("lock"))
        );
        // No passphrase configured.
        let lock = Arc::new(InstanceLock::acquire(&data).unwrap());
        let no_pass = DaemonBackup::new(data.clone(), Some(lock), None);
        no_pass.db_slot().set(open_db(&data)).ok().unwrap();
        assert!(
            matches!(no_pass.create_blocking(&target), Err(BackupError::Service(m)) if m.contains("passphrase"))
        );
        // Restore from a directory with no archive / a missing path.
        let s = DaemonBackup::new(data.clone(), None, Some(passfile(&root)));
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(s
            .restore_blocking(&BackupTarget::new(empty).unwrap())
            .is_err());
        assert!(s
            .restore_blocking(&BackupTarget::new(root.join("nope")).unwrap())
            .is_err());
        assert!(!sibling(&data, MARKER_SUFFIX).unwrap().exists());
    }

    #[test]
    fn a_marker_without_staging_changes_nothing_and_clears_itself() {
        let root = tmp("nostage");
        let data = root.join("data");
        seed_data(&data, 1);
        let lock = InstanceLock::acquire(&data).unwrap();
        write_marker(&sibling(&data, MARKER_SUFFIX).unwrap()).unwrap();
        assert!(apply_pending_restore(&data, &lock).is_err());
        assert!(!sibling(&data, MARKER_SUFFIX).unwrap().exists());
        assert_eq!(fs::read(data.join("identity.bin")).unwrap(), vec![1u8; 64]);
    }

    /// Snapshot of a directory tree: relative path → bytes (dirs as None).
    fn tree(root: &Path) -> std::collections::BTreeMap<String, Option<Vec<u8>>> {
        type Tree = std::collections::BTreeMap<String, Option<Vec<u8>>>;
        fn walk(dir: &Path, root: &Path, out: &mut Tree) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                if p.is_dir() {
                    out.insert(rel, None);
                    walk(&p, root, out);
                } else {
                    out.insert(rel, Some(fs::read(&p).unwrap()));
                }
            }
        }
        let mut out = Tree::new();
        walk(root, root, &mut out);
        out
    }

    #[tokio::test]
    async fn a_failure_at_any_phase_rolls_back_to_byte_identical_original_data() {
        for fail_at in [Phase::MovedAway, Phase::Placed, Phase::DbRestored] {
            let root = tmp("rollback");
            let data = root.join("data");
            seed_data(&data, 5);
            let lock = Arc::new(InstanceLock::acquire(&data).unwrap());
            let db = open_db(&data);
            db.execute("CREATE TABLE kv(id INTEGER PRIMARY KEY, v TEXT)", ())
                .unwrap();
            db.execute("INSERT INTO kv VALUES (1, 'v1')", ()).unwrap();
            let svc = DaemonBackup::new(data.clone(), Some(lock.clone()), Some(passfile(&root)));
            svc.db_slot().set(db.clone()).ok().unwrap();
            let target = BackupTarget::new(root.join("out")).unwrap();
            svc.create_backup(&target).await.unwrap();
            // The live state diverges from the backup, then a restore is staged.
            fs::write(data.join("identity.bin"), [9u8; 64]).unwrap();
            svc.restore(&target).await.unwrap();
            db.close().unwrap();
            drop(db);
            let before = tree(&data);

            let r = apply_with(&data, &lock, &|p| {
                if p == fail_at {
                    Err(anyhow!("injected failure at {p:?}"))
                } else {
                    Ok(())
                }
            });
            let e = r.expect_err("the injected failure must surface");
            assert!(format!("{e:#}").contains("injected"), "{e:#}");
            assert_eq!(
                tree(&data),
                before,
                "data folder must be byte-identical after a failure at {fail_at:?}"
            );
            // The staged restore is still there to retry; nothing else lingers.
            assert!(sibling(&data, MARKER_SUFFIX).unwrap().exists());
            assert!(sibling(&data, STAGING_SUFFIX).unwrap().is_dir());
            let leftovers: Vec<_> = fs::read_dir(&root)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("pre-restore"))
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");

            // And the very same staged restore succeeds on retry.
            assert!(apply_pending_restore(&data, &lock).unwrap().is_some());
            let db = open_db(&data);
            assert_eq!(values_of(&db, "kv"), ["v1"]);
            db.close().unwrap();
        }
    }
}
