//! `04-offline-event-log-architecture.md` §48 "Work Queue
//! Reconciliation," verbatim: "if an event indicates a pending
//! operation but the optimized work table is missing/corrupt, a
//! recovery reconciler can reconstruct it. This is a key advantage of
//! retaining semantic history." `ROADMAP.md`'s own §48 row: "Not
//! started."
//!
//! ## Generic on purpose, not tied to any one domain's events
//!
//! "Pending operation" means something different in every domain this
//! workspace has built a real event catalog for — an unqueued
//! `MessageQueued`, an in-flight `TransferState::InProgress`, an
//! unacknowledged `EmergencyEvent::ReportCreated`. Rather than pick
//! one domain (or worse, half-heartedly cover several), this module
//! is generic over what "opens" and "closes" a pending item — a real
//! caller supplies those two rules for its own event type, and
//! [`reconcile`] does the actual replay-and-compare regardless of
//! domain. The same shape [`siar_blob_manifest::transfer_state::
//! decide`]'s whole family already established for §29: this crate
//! doesn't know what a `TransferId` or a `ReportId` is, and doesn't
//! need to.
//!
//! ## What "reconstruct" means here
//!
//! [`reconcile`] doesn't touch a real work table at all — it computes
//! what a CORRECT one would contain by replaying history, then diffs
//! that against whatever the caller says the CURRENT one actually
//! contains. The result is a [`ReconciliationReport`] naming exactly
//! two kinds of drift, both explicitly named in the spec's own
//! "missing/corrupt": items real history says are still pending but
//! the current table is missing ([`ReconciliationReport::missing`]),
//! and items the current table claims are pending that history says
//! are already closed — or were never really opened —
//! ([`ReconciliationReport::stale`]). Actually writing either
//! correction into a real work table is left to the caller, which
//! already owns that table and this crate deliberately doesn't.

use std::collections::HashSet;
use std::hash::Hash;

/// What replaying history alone says about pending work, plus a diff
/// against whatever the caller's own current work table claims. See
/// this module's own doc comment for what "missing"/"stale" mean.
///
/// `missing`/`stale` are built from a `HashSet` diff, so their ORDER
/// is not meaningful or stable across runs — a caller comparing more
/// than one entry (in a test, say) should sort first rather than
/// relying on insertion or iteration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationReport<Id> {
    /// History says these are still pending; the current work table
    /// doesn't have them. §48's own "missing" case — reconstruct these.
    pub missing: Vec<Id>,
    /// The current work table claims these are pending; history says
    /// otherwise (already closed, or never validly opened at all).
    /// §48's own "corrupt" case — these should be removed.
    pub stale: Vec<Id>,
}

impl<Id> ReconciliationReport<Id> {
    /// No drift at all — a real caller's common-case check before
    /// doing anything else with a report.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.stale.is_empty()
    }
}

/// Replays `events` in order, tracking which `Id`s `opens` has marked
/// pending and `closes` hasn't yet marked closed — the "what does
/// history alone say is still pending" half of [`reconcile`], exposed
/// on its own for a caller that just wants that answer without
/// needing to already have a current work table to diff against (a
/// first-ever build of the table, say, rather than a repair of one
/// that already exists).
///
/// `closes` for an `Id` that was never `opens`-ed is a no-op, not an
/// error — closing something that was never open leaves "still
/// pending" correctly empty either way, and this function has no way
/// to distinguish that from a legitimate close arriving in the same
/// batch as (or after, given at-least-once redelivery) its own open.
pub fn pending_from_history<T, Id: Eq + Hash + Clone>(
    events: impl IntoIterator<Item = T>,
    opens: impl Fn(&T) -> Option<Id>,
    closes: impl Fn(&T) -> Option<Id>,
) -> HashSet<Id> {
    let mut pending = HashSet::new();
    for event in events {
        if let Some(id) = opens(&event) {
            pending.insert(id);
        }
        if let Some(id) = closes(&event) {
            pending.remove(&id);
        }
    }
    pending
}

/// §48 itself: replay `events` via [`pending_from_history`], then diff
/// the result against `current_work_table` — the caller's own account
/// of what its (possibly missing/corrupt) optimized table currently
/// holds. See this module's own doc comment for what the two sides of
/// the returned [`ReconciliationReport`] mean and for why this
/// function never writes anywhere itself.
pub fn reconcile<T, Id: Eq + Hash + Clone>(
    events: impl IntoIterator<Item = T>,
    opens: impl Fn(&T) -> Option<Id>,
    closes: impl Fn(&T) -> Option<Id>,
    current_work_table: &HashSet<Id>,
) -> ReconciliationReport<Id> {
    let derived_pending = pending_from_history(events, opens, closes);
    ReconciliationReport {
        missing: derived_pending
            .difference(current_work_table)
            .cloned()
            .collect(),
        stale: current_work_table
            .difference(&derived_pending)
            .cloned()
            .collect(),
    }
}

/// Same replay [`pending_from_history`] does, plus one more rule:
/// once `expires` marks an `Id` expired, no LATER `opens` for that
/// same `Id` ever re-adds it — §83's own "expired pending work is not
/// resurrected," made real rather than left as an untested claim.
/// Without this function's own `expired` bookkeeping, a redelivered or
/// out-of-order "open" arriving after its own expiry would silently
/// undo the expiry — exactly the resurrection the invariant names.
///
/// `expires` firing for an `Id` that was never `opens`-ed, or already
/// `closes`-ed, is a harmless no-op — same reasoning
/// [`pending_from_history`]'s own doc comment gives for an
/// unmatched `closes`.
pub fn pending_from_history_with_expiry<T, Id: Eq + Hash + Clone>(
    events: impl IntoIterator<Item = T>,
    opens: impl Fn(&T) -> Option<Id>,
    closes: impl Fn(&T) -> Option<Id>,
    expires: impl Fn(&T) -> Option<Id>,
) -> HashSet<Id> {
    let mut pending = HashSet::new();
    let mut expired = HashSet::new();
    for event in events {
        if let Some(id) = opens(&event) {
            // The one rule this function adds over `pending_from_history`:
            // an `Id` already marked `expired` ignores any further
            // `opens` for it, permanently.
            if !expired.contains(&id) {
                pending.insert(id);
            }
        }
        if let Some(id) = closes(&event) {
            pending.remove(&id);
        }
        if let Some(id) = expires(&event) {
            pending.remove(&id);
            expired.insert(id);
        }
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny stand-in domain event, deliberately not any real
    /// crate's own type — this module's whole point is not needing
    /// one.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum FakeEvent {
        Opened(u32),
        Closed(u32),
        Expired(u32),
        Unrelated,
    }

    fn opens(e: &FakeEvent) -> Option<u32> {
        match e {
            FakeEvent::Opened(id) => Some(*id),
            _ => None,
        }
    }

    fn closes(e: &FakeEvent) -> Option<u32> {
        match e {
            FakeEvent::Closed(id) => Some(*id),
            _ => None,
        }
    }

    fn expires(e: &FakeEvent) -> Option<u32> {
        match e {
            FakeEvent::Expired(id) => Some(*id),
            _ => None,
        }
    }

    #[test]
    fn an_opened_item_with_no_close_is_pending() {
        let events = vec![FakeEvent::Opened(1)];
        let pending = pending_from_history(events, opens, closes);
        assert_eq!(pending, HashSet::from([1]));
    }

    #[test]
    fn a_closed_item_is_no_longer_pending() {
        let events = vec![FakeEvent::Opened(1), FakeEvent::Closed(1)];
        let pending = pending_from_history(events, opens, closes);
        assert!(pending.is_empty());
    }

    #[test]
    fn unrelated_events_are_ignored() {
        let events = vec![
            FakeEvent::Unrelated,
            FakeEvent::Opened(1),
            FakeEvent::Unrelated,
        ];
        let pending = pending_from_history(events, opens, closes);
        assert_eq!(pending, HashSet::from([1]));
    }

    #[test]
    fn a_close_with_no_matching_open_is_a_harmless_no_op() {
        let events = vec![FakeEvent::Closed(99)];
        let pending = pending_from_history(events, opens, closes);
        assert!(pending.is_empty());
    }

    #[test]
    fn reconcile_finds_a_missing_item_the_work_table_forgot() {
        let events = vec![FakeEvent::Opened(1), FakeEvent::Opened(2)];
        let current_work_table = HashSet::from([1]);
        let report = reconcile(events, opens, closes, &current_work_table);
        assert_eq!(report.missing, vec![2]);
        assert!(report.stale.is_empty());
        assert!(!report.is_clean());
    }

    #[test]
    fn reconcile_finds_a_stale_item_history_says_is_already_closed() {
        let events = vec![FakeEvent::Opened(1), FakeEvent::Closed(1)];
        let current_work_table = HashSet::from([1]);
        let report = reconcile(events, opens, closes, &current_work_table);
        assert!(report.missing.is_empty());
        assert_eq!(report.stale, vec![1]);
        assert!(!report.is_clean());
    }

    #[test]
    fn reconcile_is_clean_when_the_work_table_already_matches_history() {
        let events = vec![
            FakeEvent::Opened(1),
            FakeEvent::Opened(2),
            FakeEvent::Closed(2),
        ];
        let current_work_table = HashSet::from([1]);
        let report = reconcile(events, opens, closes, &current_work_table);
        assert!(report.is_clean());
    }

    #[test]
    fn reconcile_handles_both_missing_and_stale_at_once() {
        // History: 1 is pending, 2 is closed. Table (corrupt/stale):
        // has 2 (shouldn't), is missing 1 (should have it).
        let events = vec![
            FakeEvent::Opened(1),
            FakeEvent::Opened(2),
            FakeEvent::Closed(2),
        ];
        let current_work_table = HashSet::from([2]);
        let report = reconcile(events, opens, closes, &current_work_table);
        assert_eq!(report.missing, vec![1]);
        assert_eq!(report.stale, vec![2]);
    }

    #[test]
    fn an_expired_item_is_removed_from_pending() {
        let events = vec![FakeEvent::Opened(1), FakeEvent::Expired(1)];
        let pending = pending_from_history_with_expiry(events, opens, closes, expires);
        assert!(pending.is_empty());
    }

    #[test]
    fn a_reopen_after_expiry_is_not_resurrected() {
        // §83's own invariant 6, as a concrete example: 1 is opened,
        // expires, and is then "opened" again — a plausible redelivery
        // or out-of-order replay of the original open — and must stay
        // gone.
        let events = vec![
            FakeEvent::Opened(1),
            FakeEvent::Expired(1),
            FakeEvent::Opened(1),
        ];
        let pending = pending_from_history_with_expiry(events, opens, closes, expires);
        assert!(!pending.contains(&1));
    }

    #[test]
    fn expiring_something_never_opened_or_already_closed_is_a_harmless_no_op() {
        let events = vec![FakeEvent::Expired(99)];
        let pending = pending_from_history_with_expiry(events, opens, closes, expires);
        assert!(pending.is_empty());
    }

    use proptest::prelude::*;

    proptest! {
        /// §83, invariant 6: "expired pending work is not
        /// resurrected." However many times (0 to 5) an `open` for
        /// the same id arrives AFTER that id has already expired —
        /// modeling redelivery, replay, or simple out-of-order
        /// arrival, all real possibilities for an at-least-once
        /// event log — the id never reappears as pending.
        #[test]
        fn expired_work_is_never_resurrected_by_any_number_of_later_opens(
            opens_after_expiry in 0usize..5
        ) {
            let id = 1u32;
            let mut events = vec![FakeEvent::Opened(id), FakeEvent::Expired(id)];
            for _ in 0..opens_after_expiry {
                events.push(FakeEvent::Opened(id));
            }
            let pending = pending_from_history_with_expiry(events, opens, closes, expires);
            prop_assert!(!pending.contains(&id));
        }

        /// The other direction, checked in the same property so a
        /// regression that makes EVERYTHING vanish (rather than just
        /// expired ids) can't slip through: an id that was opened but
        /// never expired is still pending regardless of how many
        /// *unrelated* ids in the same history did expire.
        #[test]
        fn an_unexpired_item_survives_regardless_of_other_ids_expiring(
            other_expired_ids in prop::collection::hash_set(2u32..100, 0..5)
        ) {
            let survivor = 1u32;
            let mut events = vec![FakeEvent::Opened(survivor)];
            for &id in &other_expired_ids {
                events.push(FakeEvent::Opened(id));
                events.push(FakeEvent::Expired(id));
            }
            let pending = pending_from_history_with_expiry(events, opens, closes, expires);
            prop_assert!(pending.contains(&survivor));
            for id in &other_expired_ids {
                prop_assert!(!pending.contains(id));
            }
        }
    }
}
