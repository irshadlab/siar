//! §67 "Android Process Death", §68 "Android Foreground Service
//! Recovery", §69 "iOS Recovery", §70 "Desktop Daemon Recovery", §71
//! "IPC Client Recovery", §72 "Headless Node Recovery".
//!
//! Platform-neutral rules the Kotlin/Swift/GUI layers consume through
//! the FFI/IPC boundary. The common principle: durable state lives in
//! Rust storage; a platform callback is only an *execution
//! opportunity*, and a client never keeps parallel hidden state.

use crate::progressive::RecoveryTier;
use crate::recovery_mode::RecoverySeverity;
use crate::shutdown_marker::RuntimeGeneration;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §67
// ---------------------------------------------------------------------

/// Proof that a state change reached durable storage. Only
/// [`DurableReceipt::from_commit`] makes one, so UI-facing success can
/// be typed to require it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableReceipt {
    committed_seq: u64,
}

impl DurableReceipt {
    /// Call with the sequence number returned by a *completed*
    /// storage commit.
    pub const fn from_commit(committed_seq: u64) -> Self {
        Self { committed_seq }
    }

    pub const fn committed_seq(&self) -> u64 {
        self.committed_seq
    }
}

/// §67: "persist before returning success to UI." The success value
/// handed across the FFI cannot be built without a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSuccess {
    receipt: DurableReceipt,
}

impl UiSuccess {
    pub const fn new(receipt: DurableReceipt) -> Self {
        Self { receipt }
    }

    pub const fn receipt(&self) -> DurableReceipt {
        self.receipt
    }
}

// ---------------------------------------------------------------------
// §68 / §69
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkItem {
    pub tier: RecoveryTier,
    /// Rough cost, used only to fit a short background window.
    pub estimated_millis: u64,
    pub finished: bool,
}

/// §68: what a restarted Kotlin service learns when it asks the
/// runtime "what work remains?". Pure function of durable state.
pub fn remaining_work(items: &[WorkItem]) -> Vec<usize> {
    let mut idx: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, w)| !w.finished)
        .map(|(i, _)| i)
        .collect();
    idx.sort_by_key(|&i| (items[i].tier, i));
    idx
}

/// §69: a platform wake-up granting `budget_millis` of execution.
/// Selects work in priority order that fits. The first item is always
/// admitted even if it exceeds the budget, so a long item cannot be
/// starved forever by a series of short windows; it must itself be
/// resumable (every durable operation here is).
pub fn select_for_opportunity(items: &[WorkItem], budget_millis: u64) -> Vec<usize> {
    let mut remaining = budget_millis;
    let mut out = Vec::new();
    for i in remaining_work(items) {
        let c = items[i].estimated_millis;
        if out.is_empty() || c <= remaining {
            remaining = remaining.saturating_sub(c);
            out.push(i);
        }
    }
    out
}

// ---------------------------------------------------------------------
// §70 / §71
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessRole {
    Daemon,
    Gui,
    Cli,
}

/// §70: "GUI should not attempt to repair DB independently."
pub const fn may_repair_storage(r: ProcessRole) -> bool {
    matches!(r, ProcessRole::Daemon)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpcClientState {
    Live,
    Disconnected,
    Reconnecting,
    AwaitingSnapshot,
    Resubscribing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IpcError {
    #[error("invalid IPC client transition {from:?} -> {to:?}")]
    InvalidTransition {
        from: IpcClientState,
        to: IpcClientState,
    },
    #[error("event from daemon epoch {event:?} but snapshot is epoch {snapshot:?}")]
    StaleEpoch {
        event: RuntimeGeneration,
        snapshot: RuntimeGeneration,
    },
    #[error("incremental event received while not live")]
    NotLive,
}

/// §71's reconnect → fresh snapshot → resubscribe sequence. Incremental
/// events are only accepted when `Live` *and* from the same daemon
/// epoch the snapshot came from — event-stream continuity across a
/// daemon restart is never assumed.
#[derive(Debug, Clone, Copy)]
pub struct IpcClient {
    state: IpcClientState,
    snapshot_epoch: Option<RuntimeGeneration>,
}

impl IpcClient {
    pub const fn new() -> Self {
        Self {
            state: IpcClientState::Disconnected,
            snapshot_epoch: None,
        }
    }

    pub const fn state(&self) -> IpcClientState {
        self.state
    }

    fn go(&mut self, to: IpcClientState) -> Result<(), IpcError> {
        use IpcClientState::*;
        let ok = matches!(
            (self.state, to),
            (Live, Disconnected)
                | (Disconnected, Reconnecting)
                | (Reconnecting, AwaitingSnapshot)
                | (Reconnecting, Disconnected)
                | (AwaitingSnapshot, Resubscribing)
                | (AwaitingSnapshot, Disconnected)
                | (Resubscribing, Live)
                | (Resubscribing, Disconnected)
        );
        if ok {
            self.state = to;
            Ok(())
        } else {
            Err(IpcError::InvalidTransition {
                from: self.state,
                to,
            })
        }
    }

    pub fn connection_lost(&mut self) {
        self.snapshot_epoch = None;
        self.state = IpcClientState::Disconnected;
    }

    pub fn begin_reconnect(&mut self) -> Result<(), IpcError> {
        self.go(IpcClientState::Reconnecting)
    }

    pub fn connected(&mut self) -> Result<(), IpcError> {
        self.go(IpcClientState::AwaitingSnapshot)
    }

    /// A fresh snapshot arrived, stamped with the daemon's epoch.
    pub fn snapshot_received(&mut self, epoch: RuntimeGeneration) -> Result<(), IpcError> {
        self.go(IpcClientState::Resubscribing)?;
        self.snapshot_epoch = Some(epoch);
        Ok(())
    }

    pub fn resubscribed(&mut self) -> Result<(), IpcError> {
        self.go(IpcClientState::Live)
    }

    /// Gate for every incremental event.
    pub fn accept_event(&self, event_epoch: RuntimeGeneration) -> Result<(), IpcError> {
        if self.state != IpcClientState::Live {
            return Err(IpcError::NotLive);
        }
        match self.snapshot_epoch {
            Some(s) if s == event_epoch => Ok(()),
            Some(s) => Err(IpcError::StaleEpoch {
                event: event_epoch,
                snapshot: s,
            }),
            None => Err(IpcError::NotLive),
        }
    }
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// §72
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessHealth {
    Healthy,
    Degraded,
    NeedsAdmin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadlessOutcome {
    pub health: HeadlessHealth,
    /// §72: routine recovery never prompts.
    pub needs_prompt: bool,
    /// Resume bundle/file/outbox work automatically.
    pub auto_resume: bool,
}

/// Maps the recovery severity to unattended behaviour. `ReadOnly` and
/// `Fatal` (keys, corruption) are the cases §72 says may need an
/// administrator; everything else resumes by itself.
pub const fn headless_outcome(s: RecoverySeverity) -> HeadlessOutcome {
    match s {
        RecoverySeverity::Clean | RecoverySeverity::Repaired => HeadlessOutcome {
            health: HeadlessHealth::Healthy,
            needs_prompt: false,
            auto_resume: true,
        },
        RecoverySeverity::Degraded => HeadlessOutcome {
            health: HeadlessHealth::Degraded,
            needs_prompt: false,
            auto_resume: true,
        },
        RecoverySeverity::ReadOnly | RecoverySeverity::Fatal => HeadlessOutcome {
            health: HeadlessHealth::NeedsAdmin,
            needs_prompt: false,
            auto_resume: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecoveryTier::*;

    #[test]
    fn ui_success_requires_a_durable_receipt() {
        let s = UiSuccess::new(DurableReceipt::from_commit(42));
        assert_eq!(s.receipt().committed_seq(), 42);
    }

    fn w(tier: RecoveryTier, ms: u64, done: bool) -> WorkItem {
        WorkItem {
            tier,
            estimated_millis: ms,
            finished: done,
        }
    }

    #[test]
    fn remaining_work_skips_finished_and_orders_by_priority() {
        let items = [
            w(Search, 5, false),
            w(Identity, 5, true),
            w(PendingMessages, 5, false),
            w(SosCriticalDtn, 5, false),
        ];
        assert_eq!(remaining_work(&items), vec![2, 3, 0]);
    }

    #[test]
    fn opportunity_fits_work_by_priority_within_budget() {
        let items = [
            w(PendingMessages, 40, false),
            w(ActiveTransfers, 40, false),
            w(Search, 5, false),
        ];
        assert_eq!(select_for_opportunity(&items, 90), vec![0, 1, 2]);
        assert_eq!(select_for_opportunity(&items, 50), vec![0, 2]);
    }

    #[test]
    fn first_item_is_admitted_even_if_it_exceeds_the_window() {
        let items = [w(PendingMessages, 500, false), w(Search, 1, false)];
        assert_eq!(select_for_opportunity(&items, 10), vec![0]);
        assert!(select_for_opportunity(&[], 10).is_empty());
    }

    #[test]
    fn only_the_daemon_repairs_storage() {
        assert!(may_repair_storage(ProcessRole::Daemon));
        assert!(!may_repair_storage(ProcessRole::Gui));
        assert!(!may_repair_storage(ProcessRole::Cli));
    }

    fn live(epoch: u64) -> IpcClient {
        let mut c = IpcClient::new();
        c.begin_reconnect().unwrap();
        c.connected().unwrap();
        c.snapshot_received(RuntimeGeneration(epoch)).unwrap();
        c.resubscribed().unwrap();
        c
    }

    #[test]
    fn reconnect_requires_a_fresh_snapshot_before_going_live() {
        let mut c = IpcClient::new();
        c.begin_reconnect().unwrap();
        assert!(c.resubscribed().is_err());
        c.connected().unwrap();
        assert!(c.resubscribed().is_err(), "cannot skip the snapshot");
        c.snapshot_received(RuntimeGeneration(1)).unwrap();
        c.resubscribed().unwrap();
        assert_eq!(c.state(), IpcClientState::Live);
    }

    #[test]
    fn events_flow_only_when_live_and_same_epoch() {
        let c = live(5);
        assert!(c.accept_event(RuntimeGeneration(5)).is_ok());
        assert_eq!(
            c.accept_event(RuntimeGeneration(4)),
            Err(IpcError::StaleEpoch {
                event: RuntimeGeneration(4),
                snapshot: RuntimeGeneration(5)
            })
        );
    }

    #[test]
    fn daemon_restart_drops_the_old_stream_and_forces_a_resnapshot() {
        let mut c = live(5);
        c.connection_lost();
        assert_eq!(c.accept_event(RuntimeGeneration(5)), Err(IpcError::NotLive));
        c.begin_reconnect().unwrap();
        c.connected().unwrap();
        // new daemon epoch 6; old epoch's events must stay rejected
        c.snapshot_received(RuntimeGeneration(6)).unwrap();
        c.resubscribed().unwrap();
        assert!(c.accept_event(RuntimeGeneration(6)).is_ok());
        assert!(c.accept_event(RuntimeGeneration(5)).is_err());
    }

    #[test]
    fn headless_never_prompts_and_only_escalates_for_key_or_corruption() {
        use RecoverySeverity::*;
        for s in [Clean, Repaired, Degraded, ReadOnly, Fatal] {
            assert!(!headless_outcome(s).needs_prompt, "{s:?}");
        }
        assert!(headless_outcome(Repaired).auto_resume);
        assert!(headless_outcome(Degraded).auto_resume);
        assert_eq!(headless_outcome(Degraded).health, HeadlessHealth::Degraded);
        assert!(!headless_outcome(Fatal).auto_resume);
        assert_eq!(
            headless_outcome(ReadOnly).health,
            HeadlessHealth::NeedsAdmin
        );
    }
}
