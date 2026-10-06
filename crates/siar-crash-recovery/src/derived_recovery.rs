//! §40 "Outbox Reconciliation", §41 "Projection Recovery", §42
//! "Projection Corruption", §43 "Search Index Recovery", §44
//! "Conversation Summary Recovery".
//!
//! Everything here follows one rule: the semantic event log is
//! authoritative, everything else is derived and may be repaired,
//! dropped, or rebuilt — but corruption of the authoritative store is
//! never papered over.

use crate::state_machine::RecoveryState;
use serde::{Deserialize, Serialize};
use siar_domain::{ConversationId, MessageId};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------
// §40
// ---------------------------------------------------------------------

/// A message's state as the authoritative event log sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SemanticMessageState {
    /// `MessageQueued` and nothing later.
    Queued,
    Delivered,
    /// Failed or cancelled — terminal, nothing left to send.
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutboxAction {
    /// Event says queued, outbox row missing → rebuild it.
    Reconstruct,
    /// Outbox row is stale (message delivered/terminal) → delete it.
    Remove,
    /// Outbox row for a message the event log has never heard of.
    /// Not deleted: the event log, not the outbox, might be the one
    /// that is behind. Flag for investigation.
    FlagUnknown,
    Keep,
}

/// §40's table, for one message.
pub const fn reconcile_outbox_entry(
    state: Option<SemanticMessageState>,
    outbox_present: bool,
) -> OutboxAction {
    use OutboxAction::*;
    use SemanticMessageState::*;
    match (state, outbox_present) {
        (Some(Queued), false) => Reconstruct,
        (Some(Queued), true) => Keep,
        (Some(Delivered | Terminal), true) => Remove,
        (Some(Delivered | Terminal), false) => Keep,
        (None, true) => FlagUnknown,
        (None, false) => Keep,
    }
}

/// Whole-store reconciliation. Output contains only entries needing
/// action (everything `Keep` is omitted), sorted by message id bytes
/// for determinism.
pub fn reconcile_outbox(
    events: &HashMap<MessageId, SemanticMessageState>,
    outbox: &HashSet<MessageId>,
) -> Vec<(MessageId, OutboxAction)> {
    let mut out = Vec::new();
    for (id, st) in events {
        let a = reconcile_outbox_entry(Some(*st), outbox.contains(id));
        if a != OutboxAction::Keep {
            out.push((*id, a));
        }
    }
    for id in outbox {
        if !events.contains_key(id) {
            out.push((*id, OutboxAction::FlagUnknown));
        }
    }
    out.sort_by_key(|(id, _)| id.as_uuid());
    out
}

// ---------------------------------------------------------------------
// §41
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionCheckpoint {
    pub schema_version: u32,
    /// Sequence of the last committed event this projection applied.
    pub last_applied_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    NoCheckpoint,
    SchemaChanged,
    /// Checkpoint claims events the log doesn't have — the projection
    /// (or its checkpoint) can't be trusted.
    CheckpointAheadOfLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionPlan {
    UpToDate,
    /// Replay committed events in `(from_exclusive, to_inclusive]`.
    Replay {
        from_exclusive: u64,
        to_inclusive: u64,
    },
    Rebuild(RebuildReason),
}

/// §41: read the checkpoint, replay committed events after it; if the
/// schema version changed, rebuild.
pub const fn plan_projection(
    checkpoint: Option<ProjectionCheckpoint>,
    current_schema_version: u32,
    committed_head_seq: u64,
) -> ProjectionPlan {
    let Some(c) = checkpoint else {
        return ProjectionPlan::Rebuild(RebuildReason::NoCheckpoint);
    };
    if c.schema_version != current_schema_version {
        return ProjectionPlan::Rebuild(RebuildReason::SchemaChanged);
    }
    if c.last_applied_seq > committed_head_seq {
        return ProjectionPlan::Rebuild(RebuildReason::CheckpointAheadOfLog);
    }
    if c.last_applied_seq == committed_head_seq {
        ProjectionPlan::UpToDate
    } else {
        ProjectionPlan::Replay {
            from_exclusive: c.last_applied_seq,
            to_inclusive: committed_head_seq,
        }
    }
}

// ---------------------------------------------------------------------
// §42
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreClass {
    /// Derived and rebuildable: projections, indexes, caches.
    DerivedNoncritical,
    /// The authoritative event store and other non-rebuildable data.
    Authoritative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptionResponse {
    DropAndRebuild,
    /// "Do not continue blindly." Recovery stops here; the caller
    /// moves the state machine to [`CorruptionResponse::halt_state`].
    Halt,
}

impl CorruptionResponse {
    /// State-machine target for a halt. `ManualRepairRequired` is the
    /// conservative default; §50-52's classification may escalate it
    /// to `FatalCorruption`.
    pub const fn halt_state(self) -> Option<RecoveryState> {
        match self {
            CorruptionResponse::Halt => Some(RecoveryState::ManualRepairRequired),
            CorruptionResponse::DropAndRebuild => None,
        }
    }
}

pub const fn respond_to_corruption(class: StoreClass) -> CorruptionResponse {
    match class {
        StoreClass::DerivedNoncritical => CorruptionResponse::DropAndRebuild,
        StoreClass::Authoritative => CorruptionResponse::Halt,
    }
}

// ---------------------------------------------------------------------
// §43
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchIndexState {
    Ready,
    Unavailable,
    Rebuilding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid search-index transition {from:?} -> {to:?}")]
pub struct InvalidSearchTransition {
    pub from: SearchIndexState,
    pub to: SearchIndexState,
}

/// §43's lifecycle. Search failing never gates messaging — nothing in
/// this type is consulted by any messaging path.
#[derive(Debug, Clone, Copy)]
pub struct SearchIndexRecovery {
    state: SearchIndexState,
}

impl SearchIndexRecovery {
    /// Start state after a crash with an unverified index.
    pub const fn after_crash() -> Self {
        Self {
            state: SearchIndexState::Unavailable,
        }
    }

    pub const fn state(&self) -> SearchIndexState {
        self.state
    }

    /// Only a fully rebuilt index serves queries.
    pub const fn queries_allowed(&self) -> bool {
        matches!(self.state, SearchIndexState::Ready)
    }

    pub fn transition(&mut self, to: SearchIndexState) -> Result<(), InvalidSearchTransition> {
        use SearchIndexState::*;
        let ok = matches!(
            (self.state, to),
            (Unavailable, Rebuilding)
                | (Rebuilding, Ready)
                | (Rebuilding, Unavailable) // rebuild failed
                | (Ready, Unavailable) // corruption detected later
        );
        if ok {
            self.state = to;
            Ok(())
        } else {
            Err(InvalidSearchTransition {
                from: self.state,
                to,
            })
        }
    }
}

// ---------------------------------------------------------------------
// §44
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamActivity {
    pub conversation: ConversationId,
    pub last_activity_millis: u64,
}

/// Order in which to rebuild chat-list rows: most recent first, ties
/// broken by conversation id so the order is deterministic.
pub fn chat_list_rebuild_order(streams: &[StreamActivity]) -> Vec<ConversationId> {
    let mut v: Vec<_> = streams.to_vec();
    v.sort_by(|a, b| {
        b.last_activity_millis
            .cmp(&a.last_activity_millis)
            .then_with(|| a.conversation.as_uuid().cmp(&b.conversation.as_uuid()))
    });
    v.into_iter().map(|s| s.conversation).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use OutboxAction::*;
    use SemanticMessageState::*;

    #[test]
    fn outbox_table_matches_spec_examples() {
        // "event says MessageQueued, outbox missing → reconstruct"
        assert_eq!(reconcile_outbox_entry(Some(Queued), false), Reconstruct);
        // "outbox exists, message already Delivered → remove"
        assert_eq!(reconcile_outbox_entry(Some(Delivered), true), Remove);
        assert_eq!(reconcile_outbox_entry(Some(Terminal), true), Remove);
        assert_eq!(reconcile_outbox_entry(Some(Queued), true), Keep);
        assert_eq!(reconcile_outbox_entry(Some(Delivered), false), Keep);
        assert_eq!(reconcile_outbox_entry(None, true), FlagUnknown);
        assert_eq!(reconcile_outbox_entry(None, false), Keep);
    }

    #[test]
    fn outbox_reconcile_reports_only_drift_and_never_deletes_unknowns() {
        let (q_missing, delivered_stale, ok, ghost) = (
            MessageId::new(),
            MessageId::new(),
            MessageId::new(),
            MessageId::new(),
        );
        let events: HashMap<_, _> = [
            (q_missing, Queued),
            (delivered_stale, Delivered),
            (ok, Queued),
        ]
        .into();
        let outbox: HashSet<_> = [delivered_stale, ok, ghost].into();
        let plan: HashMap<_, _> = reconcile_outbox(&events, &outbox).into_iter().collect();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[&q_missing], Reconstruct);
        assert_eq!(plan[&delivered_stale], Remove);
        assert_eq!(plan[&ghost], FlagUnknown);
        assert!(!plan.contains_key(&ok));
    }

    #[test]
    fn outbox_repair_converges_in_one_pass() {
        let a = MessageId::new();
        let events: HashMap<_, _> = [(a, Queued)].into();
        let mut outbox: HashSet<MessageId> = HashSet::new();
        for (id, act) in reconcile_outbox(&events, &outbox) {
            if act == Reconstruct {
                outbox.insert(id);
            }
        }
        assert!(reconcile_outbox(&events, &outbox).is_empty());
    }

    #[test]
    fn projection_plans() {
        let cp = |v, s| {
            Some(ProjectionCheckpoint {
                schema_version: v,
                last_applied_seq: s,
            })
        };
        assert_eq!(plan_projection(cp(2, 10), 2, 10), ProjectionPlan::UpToDate);
        assert_eq!(
            plan_projection(cp(2, 10), 2, 15),
            ProjectionPlan::Replay {
                from_exclusive: 10,
                to_inclusive: 15
            }
        );
        assert_eq!(
            plan_projection(cp(1, 10), 2, 15),
            ProjectionPlan::Rebuild(RebuildReason::SchemaChanged)
        );
        assert_eq!(
            plan_projection(None, 2, 15),
            ProjectionPlan::Rebuild(RebuildReason::NoCheckpoint)
        );
        assert_eq!(
            plan_projection(cp(2, 20), 2, 15),
            ProjectionPlan::Rebuild(RebuildReason::CheckpointAheadOfLog)
        );
    }

    #[test]
    fn schema_change_beats_everything_else() {
        let c = Some(ProjectionCheckpoint {
            schema_version: 1,
            last_applied_seq: 99,
        });
        assert_eq!(
            plan_projection(c, 2, 5),
            ProjectionPlan::Rebuild(RebuildReason::SchemaChanged)
        );
    }

    #[test]
    fn noncritical_corruption_rebuilds_authoritative_halts() {
        assert_eq!(
            respond_to_corruption(StoreClass::DerivedNoncritical),
            CorruptionResponse::DropAndRebuild
        );
        let h = respond_to_corruption(StoreClass::Authoritative);
        assert_eq!(h, CorruptionResponse::Halt);
        assert_eq!(h.halt_state(), Some(RecoveryState::ManualRepairRequired));
        assert_eq!(CorruptionResponse::DropAndRebuild.halt_state(), None);
    }

    #[test]
    fn halt_state_is_reachable_from_integrity_checked() {
        let mut m = crate::state_machine::RecoveryStateMachine::new();
        m.transition(RecoveryState::StorageOpened).unwrap();
        m.transition(RecoveryState::IntegrityChecked).unwrap();
        let target = respond_to_corruption(StoreClass::Authoritative)
            .halt_state()
            .unwrap();
        assert!(m.transition(target).is_ok());
    }

    #[test]
    fn search_index_serves_queries_only_after_a_full_rebuild() {
        let mut s = SearchIndexRecovery::after_crash();
        assert!(!s.queries_allowed());
        assert!(s.transition(SearchIndexState::Ready).is_err());
        s.transition(SearchIndexState::Rebuilding).unwrap();
        assert!(!s.queries_allowed());
        s.transition(SearchIndexState::Ready).unwrap();
        assert!(s.queries_allowed());
        s.transition(SearchIndexState::Unavailable).unwrap();
        assert!(!s.queries_allowed());
    }

    #[test]
    fn failed_rebuild_falls_back_to_unavailable() {
        let mut s = SearchIndexRecovery::after_crash();
        s.transition(SearchIndexState::Rebuilding).unwrap();
        s.transition(SearchIndexState::Unavailable).unwrap();
        assert_eq!(s.state(), SearchIndexState::Unavailable);
    }

    #[test]
    fn chat_list_rebuilds_most_recent_first_deterministically() {
        let (a, b, c) = (
            ConversationId::new(),
            ConversationId::new(),
            ConversationId::new(),
        );
        let s = |conversation, t| StreamActivity {
            conversation,
            last_activity_millis: t,
        };
        let order = chat_list_rebuild_order(&[s(a, 10), s(b, 30), s(c, 20)]);
        assert_eq!(order, vec![b, c, a]);
        // ties are stable regardless of input order
        let t1 = chat_list_rebuild_order(&[s(a, 5), s(b, 5)]);
        let t2 = chat_list_rebuild_order(&[s(b, 5), s(a, 5)]);
        assert_eq!(t1, t2);
    }
}
