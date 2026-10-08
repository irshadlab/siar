//! §27 "DTN Recovery", §28 "Relay ACK Ambiguity", §29 "Destination
//! ACK Ambiguity", §30 "DTN Forward History Recovery".
//!
//! The DTN bundle store is durable, so recovery is mostly *not
//! destroying* state: §27's headline rule is "do not reset all bundle
//! states". The two ACK sections reduce to one property — ACKs are
//! idempotent and the side that is unsure simply repeats itself — and
//! §30 asks for just enough forward history to prevent an immediate
//! bounce, explicitly not a perfect path record.
//!
//! Bundle ids are `siar_dtn_bundle::BundleId`; this module never mints
//! one (§12 "do not create a new BundleId for every hop").

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;
use siar_dtn_bundle::BundleId;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------
// §27
// ---------------------------------------------------------------------

/// §27's five restart steps, in the only safe order. In particular the
/// encounter scheduler resumes *last*: gossiping before tombstones are
/// loaded could re-accept a bundle that was deliberately deleted, and
/// before budgets are restored could over-replicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DtnRestoreStep {
    LoadBundles,
    RemoveExpired,
    RestoreReplicationBudget,
    RestoreTombstones,
    ResumeEncounterScheduler,
}

impl DtnRestoreStep {
    pub const ORDER: [DtnRestoreStep; 5] = [
        DtnRestoreStep::LoadBundles,
        DtnRestoreStep::RemoveExpired,
        DtnRestoreStep::RestoreReplicationBudget,
        DtnRestoreStep::RestoreTombstones,
        DtnRestoreStep::ResumeEncounterScheduler,
    ];

    /// The step that must be done next given the steps already
    /// completed (a prefix of [`Self::ORDER`]); `None` when all done.
    pub fn next_after(completed: usize) -> Option<DtnRestoreStep> {
        Self::ORDER.get(completed).copied()
    }
}

/// What the store holds about one bundle, as far as recovery needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredBundle {
    pub id: BundleId,
    pub expires_at_millis: u64,
    /// Persisted remaining replication budget; `None` if the record is
    /// missing.
    pub remaining_budget: Option<u32>,
    /// The budget the bundle was created with.
    pub initial_budget: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetRestore {
    /// Persisted value used unchanged.
    Restored(u32),
    /// Persisted value exceeded the initial budget (corrupt or from a
    /// bug); clamped down, never up.
    ClampedToInitial(u32),
    /// No persisted record. Restored as 0 remaining replications — the
    /// fail-safe direction (no flooding). Direct delivery to the
    /// bundle's destination is not governed by this budget, so the
    /// bundle is still deliverable.
    MissingHeldBack,
}

impl BudgetRestore {
    pub const fn remaining(self) -> u32 {
        match self {
            BudgetRestore::Restored(n) | BudgetRestore::ClampedToInitial(n) => n,
            BudgetRestore::MissingHeldBack => 0,
        }
    }
}

/// §27: "restore replication budget" — never reset to the initial
/// budget, which would let every restart re-flood the network.
pub const fn restore_budget(b: &StoredBundle) -> BudgetRestore {
    match b.remaining_budget {
        Some(n) if n <= b.initial_budget => BudgetRestore::Restored(n),
        Some(_) => BudgetRestore::ClampedToInitial(b.initial_budget),
        None => BudgetRestore::MissingHeldBack,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tombstone {
    pub id: BundleId,
    pub retain_until_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtnRestorePlan {
    /// Bundles kept exactly as persisted — state untouched.
    pub keep: Vec<(BundleId, BudgetRestore)>,
    pub expire: Vec<BundleId>,
    /// Bundles that are still in the store but have a live tombstone;
    /// they must be removed, not resumed.
    pub purge_tombstoned: Vec<BundleId>,
    /// Tombstones still within retention, to be reloaded.
    pub live_tombstones: Vec<Tombstone>,
    /// Tombstones past retention, safe to drop.
    pub expired_tombstones: Vec<BundleId>,
}

/// Pure planning half of §27. Applies expiry (`expires_at <= now`),
/// restores budgets, and reconciles stored bundles against live
/// tombstones. Does not touch any bundle's delivery state.
pub fn plan_dtn_restore(
    bundles: &[StoredBundle],
    tombstones: &[Tombstone],
    now_millis: u64,
) -> DtnRestorePlan {
    let (live, dead): (Vec<&Tombstone>, Vec<&Tombstone>) = tombstones
        .iter()
        .partition(|t| t.retain_until_millis > now_millis);
    let live_ids: HashSet<BundleId> = live.iter().map(|t| t.id).collect();

    let mut plan = DtnRestorePlan {
        keep: Vec::with_capacity(bundles.len()),
        expire: Vec::new(),
        purge_tombstoned: Vec::new(),
        live_tombstones: live.into_iter().copied().collect(),
        expired_tombstones: dead.into_iter().map(|t| t.id).collect(),
    };
    for b in bundles {
        if live_ids.contains(&b.id) {
            plan.purge_tombstoned.push(b.id);
        } else if b.expires_at_millis <= now_millis {
            plan.expire.push(b.id);
        } else {
            plan.keep.push((b.id, restore_budget(b)));
        }
    }
    plan
}

/// §127 "tombstone lost and rebuilt": the delivered/acknowledged
/// bundle ids are the ground truth a lost tombstone set can be rebuilt
/// from. Each rebuilt tombstone is retained for `retention_millis`
/// from `now`.
pub fn rebuild_tombstones(
    delivered: &[BundleId],
    now_millis: u64,
    retention_millis: u64,
) -> Vec<Tombstone> {
    let mut ids: Vec<BundleId> = delivered.to_vec();
    ids.sort_by_key(|b| *b.as_bytes());
    ids.dedup();
    ids.into_iter()
        .map(|id| Tombstone {
            id,
            retain_until_millis: now_millis.saturating_add(retention_millis),
        })
        .collect()
}

// ---------------------------------------------------------------------
// §28
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReceiveOutcome {
    /// First time seen: persist durably, *then* ACK.
    NewlyStored,
    /// Already durable (the crash happened after storage but before the
    /// ACK reached the origin): do not store again, just ACK.
    DuplicateReAck,
}

impl RelayReceiveOutcome {
    /// Both outcomes end in an ACK — that is the whole of §28's
    /// "Safe."
    pub const fn should_ack(self) -> bool {
        true
    }
}

/// Relay-side dedup on [`BundleId`]. Callers back it with the durable
/// bundle store; the in-memory set is the seam stand-in.
#[derive(Debug, Clone, Default)]
pub struct RelayLedger {
    durable: HashSet<BundleId>,
}

impl RelayLedger {
    pub fn from_durable<I: IntoIterator<Item = BundleId>>(ids: I) -> Self {
        Self {
            durable: ids.into_iter().collect(),
        }
    }

    /// Records receipt. For [`RelayReceiveOutcome::NewlyStored`] the
    /// caller must persist the bundle before sending the ACK.
    pub fn receive(&mut self, id: BundleId) -> RelayReceiveOutcome {
        if self.durable.insert(id) {
            RelayReceiveOutcome::NewlyStored
        } else {
            RelayReceiveOutcome::DuplicateReAck
        }
    }

    pub fn len(&self) -> usize {
        self.durable.len()
    }

    pub fn is_empty(&self) -> bool {
        self.durable.is_empty()
    }
}

// ---------------------------------------------------------------------
// §29
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckStatus {
    /// Payload committed; the origin has not been confirmed to know.
    Owed,
    /// The origin has confirmed it holds the ACK.
    ConfirmedByPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationReceive {
    /// First arrival: deliver to the application, ACK owed.
    Deliver,
    /// Already committed (origin never saw our ACK and resent): do
    /// **not** deliver again; resend the ACK.
    AlreadyCommittedResendAck,
}

/// Destination-side ACK bookkeeping. A destination cannot know whether
/// its ACK arrived, so an ACK stays [`AckStatus::Owed`] until the peer
/// confirms — which is exactly what lets a crash between "commit
/// payload" and "send ACK" heal on the next contact.
#[derive(Debug, Clone, Default)]
pub struct DestinationAckLedger {
    acks: HashMap<BundleId, AckStatus>,
}

impl DestinationAckLedger {
    pub fn from_durable<I: IntoIterator<Item = (BundleId, AckStatus)>>(it: I) -> Self {
        Self {
            acks: it.into_iter().collect(),
        }
    }

    pub fn receive_payload(&mut self, id: BundleId) -> DestinationReceive {
        match self.acks.entry(id) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(AckStatus::Owed);
                DestinationReceive::Deliver
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                // A resend means the origin still wants an ACK, even
                // if we had marked it confirmed.
                o.insert(AckStatus::Owed);
                DestinationReceive::AlreadyCommittedResendAck
            }
        }
    }

    pub fn confirm_by_peer(&mut self, id: BundleId) {
        if let Some(s) = self.acks.get_mut(&id) {
            *s = AckStatus::ConfirmedByPeer;
        }
    }

    /// §29: on next contact/session, every owed ACK is (re)sent.
    pub fn acks_to_send(&self) -> Vec<BundleId> {
        let mut v: Vec<_> = self
            .acks
            .iter()
            .filter(|(_, s)| **s == AckStatus::Owed)
            .map(|(id, _)| *id)
            .collect();
        v.sort_by_key(|id| *id.as_bytes());
        v
    }

    pub fn status(&self, id: &BundleId) -> Option<AckStatus> {
        self.acks.get(id).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckApply {
    Applied,
    /// Same ACK seen again — a no-op, never an error (§29: "ACK must
    /// be idempotent").
    AlreadyApplied,
    /// ACK for a bundle this origin is not waiting on.
    Unknown,
}

/// Origin-side: the set of bundles awaiting an ACK.
#[derive(Debug, Clone, Default)]
pub struct OriginAckTracker {
    pending: HashSet<BundleId>,
    acked: HashSet<BundleId>,
}

impl OriginAckTracker {
    pub fn expect(&mut self, id: BundleId) {
        if !self.acked.contains(&id) {
            self.pending.insert(id);
        }
    }

    pub fn apply_ack(&mut self, id: BundleId) -> AckApply {
        if self.pending.remove(&id) {
            self.acked.insert(id);
            AckApply::Applied
        } else if self.acked.contains(&id) {
            AckApply::AlreadyApplied
        } else {
            AckApply::Unknown
        }
    }

    pub fn is_pending(&self, id: &BundleId) -> bool {
        self.pending.contains(id)
    }
}

// ---------------------------------------------------------------------
// §30
// ---------------------------------------------------------------------

/// §30's preserved triple: peer, bundle, last forward time — and
/// nothing else. No path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardRecord {
    pub peer: DeviceId,
    pub bundle: BundleId,
    pub last_forward_millis: u64,
}

/// Bounded forward history. Missing or evicted history only ever makes
/// the node *more willing* to forward (a possible redundant send),
/// never blocks delivery — §30: "do not require perfect full path
/// history."
#[derive(Debug, Clone)]
pub struct ForwardHistory {
    entries: HashMap<(DeviceId, BundleId), u64>,
    max_entries: usize,
}

impl ForwardHistory {
    /// `max_entries` is clamped to at least 1.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: max_entries.max(1),
        }
    }

    /// Rebuilds from persisted records (restart path). Keeps the
    /// newest record per (peer, bundle) and enforces the bound.
    pub fn restore<I: IntoIterator<Item = ForwardRecord>>(records: I, max_entries: usize) -> Self {
        let mut h = Self::new(max_entries);
        for r in records {
            h.record(r.peer, r.bundle, r.last_forward_millis);
        }
        h
    }

    pub fn record(&mut self, peer: DeviceId, bundle: BundleId, at_millis: u64) {
        let e = self.entries.entry((peer, bundle)).or_insert(at_millis);
        if at_millis > *e {
            *e = at_millis;
        }
        if self.entries.len() > self.max_entries {
            self.evict_oldest_eighth();
        }
    }

    /// Drops the oldest ⅛ (at least one) in one pass so eviction is
    /// amortised O(1) per insert instead of an O(n) scan each time.
    fn evict_oldest_eighth(&mut self) {
        let drop_n = (self.entries.len() / 8).max(1);
        let mut times: Vec<u64> = self.entries.values().copied().collect();
        let (_, nth, _) = times.select_nth_unstable(drop_n - 1);
        let cutoff = *nth;
        let mut dropped = 0;
        self.entries.retain(|_, t| {
            if *t <= cutoff && dropped < drop_n {
                dropped += 1;
                false
            } else {
                true
            }
        });
    }

    /// Whether forwarding `bundle` to `peer` at `now` would bounce it
    /// straight back: it was sent there within `window_millis`.
    pub fn would_bounce(
        &self,
        peer: DeviceId,
        bundle: BundleId,
        now_millis: u64,
        window_millis: u64,
    ) -> bool {
        self.entries
            .get(&(peer, bundle))
            .is_some_and(|&t| now_millis.saturating_sub(t) < window_millis)
    }

    /// Drops records older than `retention_millis`.
    pub fn prune(&mut self, now_millis: u64, retention_millis: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, &mut t| now_millis.saturating_sub(t) < retention_millis);
        before - self.entries.len()
    }

    /// Records for persistence, oldest first.
    pub fn snapshot(&self) -> Vec<ForwardRecord> {
        let mut v: Vec<_> = self
            .entries
            .iter()
            .map(|(&(peer, bundle), &t)| ForwardRecord {
                peer,
                bundle,
                last_forward_millis: t,
            })
            .collect();
        v.sort_by_key(|r| {
            (
                r.last_forward_millis,
                r.peer.as_uuid(),
                *r.bundle.as_bytes(),
            )
        });
        v
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(n: u8) -> StoredBundle {
        StoredBundle {
            id: BundleId::from_bytes([n; 16]),
            expires_at_millis: 1_000,
            remaining_budget: Some(3),
            initial_budget: 8,
        }
    }

    #[test]
    fn restore_order_puts_scheduler_last() {
        assert_eq!(
            DtnRestoreStep::next_after(0),
            Some(DtnRestoreStep::LoadBundles)
        );
        assert_eq!(
            DtnRestoreStep::ORDER.last(),
            Some(&DtnRestoreStep::ResumeEncounterScheduler)
        );
        assert_eq!(DtnRestoreStep::next_after(5), None);
    }

    #[test]
    fn budget_is_restored_not_reset() {
        assert_eq!(restore_budget(&bundle(1)), BudgetRestore::Restored(3));
        assert_eq!(restore_budget(&bundle(1)).remaining(), 3);
    }

    #[test]
    fn budget_over_initial_is_clamped_down_only() {
        let mut b = bundle(1);
        b.remaining_budget = Some(99);
        assert_eq!(restore_budget(&b), BudgetRestore::ClampedToInitial(8));
    }

    #[test]
    fn missing_budget_record_fails_safe_to_zero() {
        let mut b = bundle(1);
        b.remaining_budget = None;
        assert_eq!(restore_budget(&b), BudgetRestore::MissingHeldBack);
        assert_eq!(restore_budget(&b).remaining(), 0);
    }

    #[test]
    fn plan_expires_keeps_and_purges_tombstoned() {
        let live = bundle(1);
        let mut expired = bundle(2);
        expired.expires_at_millis = 500; // <= now
        let dead_by_tombstone = bundle(3);
        let tombs = [
            Tombstone {
                id: dead_by_tombstone.id,
                retain_until_millis: 2_000,
            },
            Tombstone {
                id: BundleId::from_bytes([9; 16]),
                retain_until_millis: 400, // past retention
            },
        ];
        let plan = plan_dtn_restore(&[live, expired, dead_by_tombstone], &tombs, 500);
        assert_eq!(plan.keep, vec![(live.id, BudgetRestore::Restored(3))]);
        assert_eq!(plan.expire, vec![expired.id]);
        assert_eq!(plan.purge_tombstoned, vec![dead_by_tombstone.id]);
        assert_eq!(plan.live_tombstones.len(), 1);
        assert_eq!(plan.expired_tombstones, vec![BundleId::from_bytes([9; 16])]);
    }

    #[test]
    fn plan_is_idempotent_under_repeated_runs() {
        let bs = [bundle(1), bundle(2)];
        assert_eq!(
            plan_dtn_restore(&bs, &[], 10),
            plan_dtn_restore(&bs, &[], 10)
        );
    }

    #[test]
    fn relay_crash_before_ack_heals_on_origin_resend() {
        let id = BundleId::new();
        let mut relay = RelayLedger::default();
        // First delivery stored durably, ACK lost to the crash.
        assert_eq!(relay.receive(id), RelayReceiveOutcome::NewlyStored);
        // Restart: ledger rebuilt from the durable store.
        let mut relay = RelayLedger::from_durable([id]);
        // Origin resends.
        let out = relay.receive(id);
        assert_eq!(out, RelayReceiveOutcome::DuplicateReAck);
        assert!(out.should_ack());
        assert_eq!(relay.len(), 1, "no second copy stored");
    }

    #[test]
    fn destination_resends_ack_until_peer_confirms() {
        let id = BundleId::new();
        let mut d = DestinationAckLedger::default();
        assert_eq!(d.receive_payload(id), DestinationReceive::Deliver);
        // crash before ACK sent; after restart ledger is durable-loaded
        let mut d = DestinationAckLedger::from_durable([(id, AckStatus::Owed)]);
        assert_eq!(d.acks_to_send(), vec![id]);
        d.confirm_by_peer(id);
        assert!(d.acks_to_send().is_empty());
    }

    #[test]
    fn duplicate_payload_is_not_delivered_twice_but_reacks() {
        let id = BundleId::new();
        let mut d = DestinationAckLedger::default();
        assert_eq!(d.receive_payload(id), DestinationReceive::Deliver);
        d.confirm_by_peer(id);
        assert_eq!(
            d.receive_payload(id),
            DestinationReceive::AlreadyCommittedResendAck
        );
        assert_eq!(d.status(&id), Some(AckStatus::Owed));
    }

    #[test]
    fn origin_ack_is_idempotent() {
        let id = BundleId::new();
        let mut o = OriginAckTracker::default();
        o.expect(id);
        assert_eq!(o.apply_ack(id), AckApply::Applied);
        assert_eq!(o.apply_ack(id), AckApply::AlreadyApplied);
        assert_eq!(o.apply_ack(BundleId::new()), AckApply::Unknown);
        o.expect(id); // re-expecting an acked bundle must not reopen it
        assert!(!o.is_pending(&id));
    }

    #[test]
    fn forward_history_prevents_immediate_bounce_only_within_window() {
        let (p, b) = (DeviceId::new(), BundleId::new());
        let mut h = ForwardHistory::new(100);
        h.record(p, b, 1_000);
        assert!(h.would_bounce(p, b, 1_500, 1_000));
        assert!(!h.would_bounce(p, b, 2_000, 1_000));
        assert!(!h.would_bounce(DeviceId::new(), b, 1_500, 1_000));
    }

    #[test]
    fn missing_history_never_blocks_forwarding() {
        let h = ForwardHistory::new(10);
        assert!(!h.would_bounce(DeviceId::new(), BundleId::new(), 5, u64::MAX));
    }

    #[test]
    fn history_keeps_newest_time_and_survives_snapshot_restore() {
        let (p, b) = (DeviceId::new(), BundleId::new());
        let mut h = ForwardHistory::new(10);
        h.record(p, b, 900);
        h.record(p, b, 100); // older record must not rewind
        let snap = h.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].last_forward_millis, 900);
        let h2 = ForwardHistory::restore(snap, 10);
        assert!(h2.would_bounce(p, b, 950, 100));
    }

    #[test]
    fn history_is_bounded_and_evicts_oldest_first() {
        let mut h = ForwardHistory::new(16);
        let p = DeviceId::new();
        for t in 0..64u64 {
            h.record(p, BundleId::new(), t);
        }
        assert!(h.len() <= 16);
        let oldest_kept = h.snapshot().first().unwrap().last_forward_millis;
        assert!(oldest_kept > 30, "oldest retained = {oldest_kept}");
    }

    #[test]
    fn prune_drops_only_stale_records() {
        let p = DeviceId::new();
        let mut h = ForwardHistory::new(10);
        h.record(p, BundleId::new(), 0);
        h.record(p, BundleId::new(), 900);
        assert_eq!(h.prune(1_000, 500), 1);
        assert_eq!(h.len(), 1);
    }
}
