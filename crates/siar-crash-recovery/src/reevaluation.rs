//! §97 "Revocation Recovery Priority", §98 "Policy Re-Evaluation",
//! §99 "Capability Re-Evaluation", §100 "Routing Re-Evaluation", §101
//! "DTN vs Direct Re-Evaluation", §102 "Recovery of Multi-Path
//! Transfer".
//!
//! Durable pending work outlives the world it was queued in. Policy,
//! revocations, peer capabilities and available paths may all have
//! changed, so after a restart each is *re-derived*, never restored:
//! what is persisted is intent and verified progress, not a decision.

use serde::{Deserialize, Serialize};
use siar_domain::{DeviceId, MessageId};
use siar_dtn_bundle::BundleId;
use siar_routing_policy::types::PathId;
use std::collections::HashSet;

// ---------------------------------------------------------------------
// §97
// ---------------------------------------------------------------------

/// Startup order for the identity/send boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SendResumeStage {
    ApplyRevocations,
    ReevaluateQueuedSends,
    ResumeSends,
}

impl SendResumeStage {
    pub const ORDER: [SendResumeStage; 3] = [
        SendResumeStage::ApplyRevocations,
        SendResumeStage::ReevaluateQueuedSends,
        SendResumeStage::ResumeSends,
    ];
}

/// "Revocations before ordinary sends": sending may start only once
/// both earlier stages are done.
pub const fn sends_may_resume(revocations_applied: bool, queued_reevaluated: bool) -> bool {
    revocations_applied && queued_reevaluated
}

/// Indexes of queued sends whose target device is now revoked and
/// must be re-evaluated (not blindly sent).
pub fn queued_to_reevaluate(targets: &[DeviceId], revoked: &HashSet<DeviceId>) -> Vec<usize> {
    targets
        .iter()
        .enumerate()
        .filter(|(_, d)| revoked.contains(d))
        .map(|(i, _)| i)
        .collect()
}

// ---------------------------------------------------------------------
// §98
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentPolicy {
    pub mobile_data_allowed: bool,
    pub max_file_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingWorkFacts {
    pub needs_mobile_data: bool,
    pub file_bytes: Option<u64>,
    pub peer_revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyCancel {
    PeerRevoked,
    FileTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Proceed,
    /// May become allowed later (e.g. Wi-Fi returns): keep queued.
    DeferMobileDataDisabled,
    /// Will not become allowed by waiting: cancel.
    Cancel(PolicyCancel),
}

/// Checked against the *current* policy. Precedence: a revoked peer
/// (security) beats a size-policy change beats a connectivity
/// restriction.
pub const fn reevaluate_policy(w: &PendingWorkFacts, p: &CurrentPolicy) -> PolicyDecision {
    if w.peer_revoked {
        return PolicyDecision::Cancel(PolicyCancel::PeerRevoked);
    }
    if let Some(b) = w.file_bytes {
        if b > p.max_file_bytes {
            return PolicyDecision::Cancel(PolicyCancel::FileTooLarge);
        }
    }
    if w.needs_mobile_data && !p.mobile_data_allowed {
        return PolicyDecision::DeferMobileDataDisabled;
    }
    PolicyDecision::Proceed
}

// ---------------------------------------------------------------------
// §99
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityDecision {
    /// Capabilities negotiated in the *current* session may be used.
    UseNegotiated,
    /// Stay queued until a new session renegotiates. A cached value
    /// from before the restart is never consulted.
    StayQueuedUntilRenegotiated,
}

pub const fn capability_decision(negotiated_this_session: bool) -> CapabilityDecision {
    if negotiated_this_session {
        CapabilityDecision::UseNegotiated
    } else {
        CapabilityDecision::StayQueuedUntilRenegotiated
    }
}

// ---------------------------------------------------------------------
// §100
// ---------------------------------------------------------------------

/// What gets persisted for a pending send. There is intentionally no
/// transport/path field: "send via Bluetooth" is not durable truth.
/// Routing (Part 03) chooses the current path from this intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryIntent {
    pub message: MessageId,
    pub destination: DeviceId,
    pub expires_at_millis: u64,
}

// ---------------------------------------------------------------------
// §101
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Direct,
    Dtn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPlan {
    pub kind: PathKind,
    /// Always the original ids: switching path never changes
    /// message identity, so receiver-side dedup keeps working.
    pub message: MessageId,
    pub bundle: Option<BundleId>,
    /// The pending DTN bundle is kept (not cancelled) until the
    /// message is acknowledged, in case the direct attempt fails.
    pub retain_bundle_until_ack: bool,
}

/// Prefer a now-available direct path, preserving
/// `MessageId`/`BundleId`.
pub const fn choose_path_after_restart(
    message: MessageId,
    pending_bundle: Option<BundleId>,
    direct_available: bool,
) -> PathPlan {
    PathPlan {
        kind: if direct_available {
            PathKind::Direct
        } else {
            PathKind::Dtn
        },
        message,
        bundle: pending_bundle,
        retain_bundle_until_ack: pending_bundle.is_some(),
    }
}

// ---------------------------------------------------------------------
// §102
// ---------------------------------------------------------------------

/// Verified chunk progress — the only multipath state worth
/// persisting. A growable bitset: one bit per chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifiedChunks {
    words: Vec<u64>,
}

impl VerifiedChunks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_verified(&mut self, chunk: u32) {
        let (w, b) = ((chunk / 64) as usize, chunk % 64);
        if self.words.len() <= w {
            self.words.resize(w + 1, 0);
        }
        self.words[w] |= 1u64 << b;
    }

    pub fn is_verified(&self, chunk: u32) -> bool {
        let (w, b) = ((chunk / 64) as usize, chunk % 64);
        self.words.get(w).is_some_and(|x| x & (1u64 << b) != 0)
    }

    pub fn count(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    pub fn missing(&self, total_chunks: u32) -> Vec<u32> {
        (0..total_chunks)
            .filter(|c| !self.is_verified(*c))
            .collect()
    }
}

/// After restart, transient path assignment is built from scratch:
/// missing chunks round-robin over whichever paths exist *now*. Empty
/// `paths` yields no assignment (nothing is invented).
pub fn assign_missing_to_paths(missing: &[u32], paths: &[PathId]) -> Vec<(u32, PathId)> {
    if paths.is_empty() {
        return Vec::new();
    }
    missing
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, paths[i % paths.len()]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revocations_and_reevaluation_precede_sends() {
        assert!(SendResumeStage::ApplyRevocations < SendResumeStage::ReevaluateQueuedSends);
        assert!(SendResumeStage::ReevaluateQueuedSends < SendResumeStage::ResumeSends);
        assert!(!sends_may_resume(false, true));
        assert!(!sends_may_resume(true, false));
        assert!(sends_may_resume(true, true));
    }

    #[test]
    fn messages_to_now_revoked_devices_are_flagged() {
        let (a, b, c) = (DeviceId::new(), DeviceId::new(), DeviceId::new());
        let revoked: HashSet<_> = [b].into();
        assert_eq!(queued_to_reevaluate(&[a, b, c, b], &revoked), vec![1, 3]);
    }

    fn pol(mobile: bool, max: u64) -> CurrentPolicy {
        CurrentPolicy {
            mobile_data_allowed: mobile,
            max_file_bytes: max,
        }
    }

    fn work(mobile: bool, bytes: Option<u64>, revoked: bool) -> PendingWorkFacts {
        PendingWorkFacts {
            needs_mobile_data: mobile,
            file_bytes: bytes,
            peer_revoked: revoked,
        }
    }

    #[test]
    fn each_spec_example_policy_change_is_honoured() {
        assert_eq!(
            reevaluate_policy(&work(true, None, false), &pol(false, 10)),
            PolicyDecision::DeferMobileDataDisabled
        );
        assert_eq!(
            reevaluate_policy(&work(false, None, true), &pol(true, 10)),
            PolicyDecision::Cancel(PolicyCancel::PeerRevoked)
        );
        assert_eq!(
            reevaluate_policy(&work(false, Some(100), false), &pol(true, 10)),
            PolicyDecision::Cancel(PolicyCancel::FileTooLarge)
        );
        assert_eq!(
            reevaluate_policy(&work(true, Some(5), false), &pol(true, 10)),
            PolicyDecision::Proceed
        );
    }

    #[test]
    fn revocation_beats_size_beats_connectivity() {
        let w = work(true, Some(100), true);
        assert_eq!(
            reevaluate_policy(&w, &pol(false, 10)),
            PolicyDecision::Cancel(PolicyCancel::PeerRevoked)
        );
        let w = work(true, Some(100), false);
        assert_eq!(
            reevaluate_policy(&w, &pol(false, 10)),
            PolicyDecision::Cancel(PolicyCancel::FileTooLarge)
        );
    }

    #[test]
    fn capabilities_come_only_from_the_current_session() {
        assert_eq!(
            capability_decision(false),
            CapabilityDecision::StayQueuedUntilRenegotiated
        );
        assert_eq!(capability_decision(true), CapabilityDecision::UseNegotiated);
    }

    #[test]
    fn dtn_work_upgrades_to_direct_keeping_both_identities() {
        let (m, b) = (MessageId::new(), BundleId::new());
        let p = choose_path_after_restart(m, Some(b), true);
        assert_eq!(p.kind, PathKind::Direct);
        assert_eq!((p.message, p.bundle), (m, Some(b)));
        assert!(p.retain_bundle_until_ack);
        let p = choose_path_after_restart(m, Some(b), false);
        assert_eq!(p.kind, PathKind::Dtn);
        assert!(!choose_path_after_restart(m, None, true).retain_bundle_until_ack);
    }

    #[test]
    fn verified_chunks_track_progress_and_report_missing() {
        let mut v = VerifiedChunks::new();
        for c in [0, 1, 63, 64, 130] {
            v.mark_verified(c);
        }
        assert_eq!(v.count(), 5);
        assert!(v.is_verified(64) && !v.is_verified(65));
        assert!(!v.is_verified(10_000));
        assert_eq!(v.missing(4), vec![2, 3]);
        v.mark_verified(1);
        assert_eq!(v.count(), 5, "marking twice is idempotent");
    }

    #[test]
    fn path_assignment_is_rebuilt_from_current_paths_only() {
        let (p1, p2) = (PathId::new(), PathId::new());
        let a = assign_missing_to_paths(&[5, 6, 7], &[p1, p2]);
        assert_eq!(a, vec![(5, p1), (6, p2), (7, p1)]);
        assert!(assign_missing_to_paths(&[5], &[]).is_empty());
        // a different path set after restart gives a different layout
        let p3 = PathId::new();
        assert_eq!(
            assign_missing_to_paths(&[5, 6], &[p3]),
            vec![(5, p3), (6, p3)]
        );
    }
}
