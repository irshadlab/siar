//! §111-§115: backup coordination and restore mode.
//!
//! §113: the daemon exposes *create consistent backup* so nobody copies a
//! live database with `cp`. "Consistent" is enforced here, not hoped for:
//! a backup runs with the runtime **quiesced** (`Maintenance`: mutating
//! *commands* are refused and `network_open()` reports false), and a
//! [`QuiesceGuard`] holds it there so an admin's `maintenance off` cannot
//! reopen the door mid-copy.
//!
//! **Limit, stated plainly:** the runtime only *publishes* `network_open()`;
//! it cannot stop the hosting app's own writers. As of this writing nothing in
//! the app core reads it, so incoming messages keep being processed in
//! `Maintenance`. A [`BackupService`] therefore must not rely on the runtime
//! having silenced the app — the daemon's implementation snapshots through
//! the database's own `create_snapshot` and archives only those immutable
//! files for exactly this reason.
//!
//! §115: a restore is only legal in `Maintenance` (the daemon-stopped case is
//! the operator's, outside this process). After the data is replaced the
//! Part 09 *reconciliation* runs, and **if it fails the runtime stays in
//! Maintenance** — a restored-but-unreconciled store never serves traffic.
//!
//! What this module does not know: how to copy or restore the data. That is
//! the hosting app's [`BackupService`] (it owns the database); this module
//! owns the *when it is safe* decision.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::RuntimeError;
use crate::lifecycle::RuntimeState;
use crate::runtime::{BoxFuture, Runtime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BackupId(pub u128);

impl std::fmt::Display for BackupId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

pub const MAX_TARGET_LEN: usize = 4096;

/// A directory to write a backup into / read one from. Validated on
/// construction; a path with `..` is rejected rather than normalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupTarget(PathBuf);

impl BackupTarget {
    pub fn new(p: impl Into<PathBuf>) -> Result<Self, BackupError> {
        let p = p.into();
        let s = p.as_os_str();
        let bad = s.is_empty()
            || s.len() > MAX_TARGET_LEN
            || !p.is_absolute()
            || p.components().any(|c| matches!(c, Component::ParentDir))
            || s.to_string_lossy().contains('\0');
        if bad {
            Err(BackupError::InvalidTarget)
        } else {
            Ok(Self(p))
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BackupError {
    #[error("backup target must be an absolute path without `..`")]
    InvalidTarget,
    #[error("another backup/restore is already running")]
    Busy,
    #[error("runtime is {0:?}; a backup needs Ready, Degraded, ReadOnly or Maintenance")]
    NotQuiescent(RuntimeState),
    #[error("restore needs the runtime in Maintenance (it is {0:?})")]
    RestoreNeedsMaintenance(RuntimeState),
    #[error("no backup service is configured on this runtime")]
    NoService,
    #[error("backup service failed: {0}")]
    Service(String),
    #[error("restored, but reconciliation failed ({0}); runtime left in Maintenance")]
    ReconcileFailed(String),
}

impl From<BackupError> for RuntimeError {
    fn from(e: BackupError) -> Self {
        match e {
            BackupError::InvalidTarget => RuntimeError::InvalidCommand("backup target".into()),
            BackupError::Busy => RuntimeError::Busy,
            BackupError::NotQuiescent(_) | BackupError::RestoreNeedsMaintenance(_) => {
                RuntimeError::NotReady
            }
            BackupError::NoService => RuntimeError::Unsupported,
            BackupError::Service(_) | BackupError::ReconcileFailed(_) => RuntimeError::Storage,
        }
    }
}

/// §114. Implemented by the embedding app, which owns the database.
pub trait BackupService: Send + Sync + 'static {
    /// Write a consistent copy of the durable state into `target`. Called
    /// only while the runtime is quiesced.
    fn create_backup<'a>(
        &'a self,
        target: &'a BackupTarget,
    ) -> BoxFuture<'a, Result<BackupId, BackupError>>;

    /// Replace the durable state with the backup in `source`. Called only in
    /// Maintenance.
    fn restore<'a>(&'a self, source: &'a BackupTarget) -> BoxFuture<'a, Result<(), BackupError>>;

    /// Part 09 reconciliation of a freshly restored store (stale cursors,
    /// outbox entries for already-delivered messages, high-water marks…).
    fn reconcile<'a>(&'a self) -> BoxFuture<'a, Result<(), BackupError>>;
}

/// Holds the runtime in a state that refuses mutations for as long as it
/// lives. `Drop` returns it to `Ready`/`Degraded` **only** if this guard is
/// the one that entered Maintenance.
pub struct QuiesceGuard {
    rt: Runtime,
    entered_maintenance: bool,
}

impl QuiesceGuard {
    pub fn enter(rt: &Runtime) -> Result<Self, BackupError> {
        let state = rt.lifecycle.state();
        let entered_maintenance = match state {
            RuntimeState::Ready | RuntimeState::Degraded => {
                rt.lifecycle
                    .transition(RuntimeState::Maintenance)
                    .map_err(|_| BackupError::Busy)?;
                rt.announce_state();
                true
            }
            // Already quiet (operator-set, or recovery left it read-only):
            // keep that state, just stop anyone from leaving it meanwhile.
            RuntimeState::Maintenance | RuntimeState::ReadOnly => false,
            other => return Err(BackupError::NotQuiescent(other)),
        };
        rt.holds.fetch_add(1, SeqCst);
        Ok(Self {
            rt: rt.clone(),
            entered_maintenance,
        })
    }
}

impl Drop for QuiesceGuard {
    fn drop(&mut self) {
        self.rt.holds.fetch_sub(1, SeqCst);
        if self.entered_maintenance && self.rt.lifecycle.state() == RuntimeState::Maintenance {
            let _ = self.rt.lifecycle.transition(RuntimeState::Ready);
            crate::supervisor::sync_lifecycle(&self.rt.lifecycle, &self.rt.health);
            self.rt.announce_state();
        }
    }
}

/// Only one backup/restore at a time per runtime (released on drop, so a
/// cancelled future cannot leave it stuck).
struct ExclusiveOp(Arc<std::sync::atomic::AtomicBool>);

impl ExclusiveOp {
    fn take(rt: &Runtime) -> Result<Self, BackupError> {
        rt.backup_busy
            .compare_exchange(false, true, SeqCst, SeqCst)
            .map_err(|_| BackupError::Busy)?;
        Ok(Self(rt.backup_busy.clone()))
    }
}

impl Drop for ExclusiveOp {
    fn drop(&mut self) {
        self.0.store(false, SeqCst);
    }
}

impl Runtime {
    /// §113: create a consistent backup of the durable state.
    pub async fn create_backup(&self, target: &str) -> Result<BackupId, BackupError> {
        let svc = self.backup.clone().ok_or(BackupError::NoService)?;
        let target = BackupTarget::new(target)?;
        let _op = ExclusiveOp::take(self)?;
        let _quiet = QuiesceGuard::enter(self)?;
        let id = svc.create_backup(&target).await?;
        tracing::info!(backup = %id, "backup created");
        Ok(id)
    }

    /// §115: restore from `source`, then reconcile. Requires Maintenance.
    pub async fn restore_backup(&self, source: &str) -> Result<(), BackupError> {
        let svc = self.backup.clone().ok_or(BackupError::NoService)?;
        let source = BackupTarget::new(source)?;
        let state = self.lifecycle.state();
        if state != RuntimeState::Maintenance {
            return Err(BackupError::RestoreNeedsMaintenance(state));
        }
        let _op = ExclusiveOp::take(self)?;
        let _quiet = QuiesceGuard::enter(self)?;
        svc.restore(&source).await?;
        svc.reconcile()
            .await
            .map_err(|e| BackupError::ReconcileFailed(e.to_string()))?;
        tracing::info!("restore complete and reconciled; runtime still in Maintenance");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Command;
    use crate::runtime::AppService;
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);
    impl Log {
        fn push(&self, s: &str) {
            self.0.lock().unwrap().push(s.to_owned());
        }
        fn get(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Records the runtime state it observed while "copying", and can be
    /// told to stall (to test mid-backup interference) or to fail.
    struct Svc {
        rt: Mutex<Option<Runtime>>,
        log: Arc<Log>,
        seen_state: Mutex<Vec<RuntimeState>>,
        stall: Option<Arc<Notify>>,
        entered: Arc<Notify>,
        fail_backup: bool,
        fail_reconcile: bool,
        next: AtomicU32,
    }

    impl Svc {
        fn new(log: Arc<Log>) -> Self {
            Self {
                rt: Mutex::new(None),
                log,
                seen_state: Mutex::default(),
                stall: None,
                entered: Arc::new(Notify::new()),
                fail_backup: false,
                fail_reconcile: false,
                next: AtomicU32::new(1),
            }
        }
        fn note_state(&self) {
            let st = self.rt.lock().unwrap().as_ref().unwrap().state();
            self.seen_state.lock().unwrap().push(st);
        }
    }

    impl BackupService for Arc<Svc> {
        fn create_backup<'a>(
            &'a self,
            t: &'a BackupTarget,
        ) -> BoxFuture<'a, Result<BackupId, BackupError>> {
            Box::pin(async move {
                self.note_state();
                self.log.push(&format!("backup {}", t.path().display()));
                self.entered.notify_one();
                if let Some(s) = &self.stall {
                    s.notified().await;
                }
                if self.fail_backup {
                    return Err(BackupError::Service("disk full".into()));
                }
                Ok(BackupId(u128::from(self.next.fetch_add(1, SeqCst))))
            })
        }
        fn restore<'a>(&'a self, t: &'a BackupTarget) -> BoxFuture<'a, Result<(), BackupError>> {
            Box::pin(async move {
                self.note_state();
                self.log.push(&format!("restore {}", t.path().display()));
                Ok(())
            })
        }
        fn reconcile<'a>(&'a self) -> BoxFuture<'a, Result<(), BackupError>> {
            Box::pin(async move {
                self.log.push("reconcile");
                if self.fail_reconcile {
                    Err(BackupError::Service("stale cursor".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    struct NoopApp;
    impl AppService for NoopApp {
        fn command<'a>(
            &'a self,
            _: &'a str,
            _: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn query<'a>(
            &'a self,
            _: &'a str,
            _: Vec<u8>,
        ) -> BoxFuture<'a, Result<Vec<u8>, RuntimeError>> {
            Box::pin(async { Ok(vec![]) })
        }
    }

    async fn rt_with(svc: Arc<Svc>) -> Runtime {
        let rt = Runtime::builder().app(NoopApp).backup(svc.clone()).build();
        *svc.rt.lock().unwrap() = Some(rt.clone());
        rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
            .await
            .unwrap();
        rt
    }

    const DIR: &str = "/var/backups/siar";

    #[test]
    fn targets_must_be_absolute_and_free_of_dotdot() {
        assert!(BackupTarget::new("/var/backups/x").is_ok());
        for bad in ["", "relative/x", "../x", "/a/../etc", "/a/b/..", "/x\0y"] {
            assert_eq!(
                BackupTarget::new(bad),
                Err(BackupError::InvalidTarget),
                "{bad:?}"
            );
        }
        assert!(BackupTarget::new(format!("/{}", "a".repeat(MAX_TARGET_LEN))).is_err());
    }

    #[tokio::test]
    async fn backup_runs_quiesced_and_returns_to_ready() {
        let log = Arc::new(Log::default());
        let svc = Arc::new(Svc::new(log.clone()));
        let rt = rt_with(svc.clone()).await;
        assert_eq!(rt.state(), RuntimeState::Ready);

        let id = rt.create_backup(DIR).await.unwrap();
        assert_eq!(id, BackupId(1));
        assert_eq!(
            svc.seen_state.lock().unwrap().as_slice(),
            [RuntimeState::Maintenance],
            "§113: the copy happens with mutating commands refused"
        );
        assert_eq!(rt.state(), RuntimeState::Ready, "and the quiesce is undone");
        assert_eq!(log.get(), [format!("backup {DIR}")]);
        assert_eq!(rt.holds.load(SeqCst), 0);
        assert!(!rt.backup_busy.load(SeqCst));
    }

    #[tokio::test]
    async fn a_failed_backup_still_returns_the_runtime_to_service() {
        let log = Arc::new(Log::default());
        let mut s = Svc::new(log);
        s.fail_backup = true;
        let svc = Arc::new(s);
        let rt = rt_with(svc).await;
        assert_eq!(
            rt.create_backup(DIR).await,
            Err(BackupError::Service("disk full".into()))
        );
        assert_eq!(rt.state(), RuntimeState::Ready);
        assert_eq!(rt.holds.load(SeqCst), 0);
        assert!(!rt.backup_busy.load(SeqCst), "the busy flag is released");
    }

    #[tokio::test]
    async fn admin_cannot_leave_maintenance_while_a_backup_is_copying() {
        let log = Arc::new(Log::default());
        let mut s = Svc::new(log);
        let gate = Arc::new(Notify::new());
        s.stall = Some(gate.clone());
        let svc = Arc::new(s);
        let entered = svc.entered.clone();
        let rt = rt_with(svc).await;

        let rt2 = rt.clone();
        let job = tokio::spawn(async move { rt2.create_backup(DIR).await });
        entered.notified().await;
        assert_eq!(rt.state(), RuntimeState::Maintenance);

        // `set-maintenance off` mid-copy is refused…
        let r = rt
            .command(
                1,
                crate::dedup::CommandId(1),
                Command::SetMaintenance(false),
            )
            .await;
        assert_eq!(r, Err(RuntimeError::Busy));
        assert_eq!(rt.state(), RuntimeState::Maintenance);
        // …and so is a second backup.
        assert_eq!(rt.create_backup(DIR).await, Err(BackupError::Busy));

        gate.notify_one();
        assert!(job.await.unwrap().is_ok());
        assert_eq!(rt.state(), RuntimeState::Ready);
        // After the backup the admin can drive the state again.
        rt.command(1, crate::dedup::CommandId(2), Command::SetMaintenance(true))
            .await
            .unwrap();
        rt.command(
            1,
            crate::dedup::CommandId(3),
            Command::SetMaintenance(false),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_operator_set_maintenance_is_left_as_the_operator_set_it() {
        let svc = Arc::new(Svc::new(Arc::default()));
        let rt = rt_with(svc).await;
        rt.command(1, crate::dedup::CommandId(1), Command::SetMaintenance(true))
            .await
            .unwrap();
        rt.create_backup(DIR).await.unwrap();
        assert_eq!(
            rt.state(),
            RuntimeState::Maintenance,
            "a backup must not end someone else's maintenance window"
        );
    }

    #[tokio::test]
    async fn backup_works_from_read_only_and_refuses_while_starting_or_stopping() {
        let svc = Arc::new(Svc::new(Arc::default()));
        let rt = rt_with(svc.clone()).await;
        rt.command(1, crate::dedup::CommandId(1), Command::SetReadOnly(true))
            .await
            .unwrap();
        rt.create_backup(DIR).await.unwrap();
        assert_eq!(rt.state(), RuntimeState::ReadOnly);

        let fresh = Runtime::builder().app(NoopApp).backup(svc).build();
        assert_eq!(
            fresh.create_backup(DIR).await,
            Err(BackupError::NotQuiescent(RuntimeState::Starting))
        );
        rt.lifecycle.begin_shutdown();
        assert_eq!(
            rt.create_backup(DIR).await,
            Err(BackupError::NotQuiescent(RuntimeState::ShuttingDown))
        );
    }

    #[tokio::test]
    async fn restore_requires_maintenance_then_reconciles_and_stays_there() {
        let log = Arc::new(Log::default());
        let svc = Arc::new(Svc::new(log.clone()));
        let rt = rt_with(svc).await;

        assert_eq!(
            rt.restore_backup(DIR).await,
            Err(BackupError::RestoreNeedsMaintenance(RuntimeState::Ready))
        );
        assert!(log.get().is_empty(), "nothing was touched");

        rt.command(1, crate::dedup::CommandId(1), Command::SetMaintenance(true))
            .await
            .unwrap();
        rt.restore_backup(DIR).await.unwrap();
        assert_eq!(
            log.get(),
            [format!("restore {DIR}"), "reconcile".to_owned()]
        );
        assert_eq!(
            rt.state(),
            RuntimeState::Maintenance,
            "the operator decides when to reopen"
        );
    }

    #[tokio::test]
    async fn failed_reconciliation_is_reported_and_never_reopens_the_runtime() {
        let mut s = Svc::new(Arc::default());
        s.fail_reconcile = true;
        let rt = rt_with(Arc::new(s)).await;
        rt.command(1, crate::dedup::CommandId(1), Command::SetMaintenance(true))
            .await
            .unwrap();
        let e = rt.restore_backup(DIR).await.unwrap_err();
        assert!(matches!(e, BackupError::ReconcileFailed(_)), "{e:?}");
        assert_eq!(rt.state(), RuntimeState::Maintenance);
    }

    #[tokio::test]
    async fn no_service_and_bad_targets_are_clean_errors() {
        let rt = Runtime::builder().app(NoopApp).build();
        rt.start(async { Ok::<_, String>(()) }, async { Ok::<_, String>(()) })
            .await
            .unwrap();
        assert_eq!(rt.create_backup(DIR).await, Err(BackupError::NoService));

        let svc = Arc::new(Svc::new(Arc::default()));
        let rt = rt_with(svc).await;
        assert_eq!(
            rt.create_backup("relative").await,
            Err(BackupError::InvalidTarget)
        );
        assert_eq!(
            rt.state(),
            RuntimeState::Ready,
            "a rejected request changes nothing"
        );
    }

    #[tokio::test]
    async fn the_command_path_is_admin_only_idempotent_and_audited_without_the_path() {
        let log = Arc::new(Log::default());
        let svc = Arc::new(Svc::new(log.clone()));
        let rt = rt_with(svc).await;
        let cmd = Command::CreateBackup { target: DIR.into() };
        assert_eq!(cmd.required_role(), crate::protocol::Role::Admin);
        assert!(
            !cmd.is_mutation(),
            "it must be allowed to run in maintenance"
        );

        let id = crate::dedup::CommandId(77);
        let first = rt.command(1, id, cmd.clone()).await.unwrap();
        let again = rt.command(1, id, cmd.clone()).await.unwrap();
        assert_eq!(first, again);
        assert_eq!(log.get().len(), 1, "a retried CommandId backs up once");
        assert_eq!(String::from_utf8(first).unwrap(), BackupId(1).to_string());

        let label = crate::audit::action_label(&cmd);
        assert_eq!(label, "create-backup");
        assert!(!label.contains("backups"));
    }
}
