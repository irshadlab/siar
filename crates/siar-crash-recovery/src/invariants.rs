//! §129 "Projection Fault Injection", §130 "Property Tests", §131
//! "Recovery Determinism".
//!
//! This module is test-only: it states the spec's invariants as
//! `proptest` properties over this crate's real recovery functions.
//! §131 is the interesting one — "same durable state ⇒ same recovery
//! result". The properties present identical state in different
//! insertion orders (hash-map iteration order is exactly what a
//! crash-and-restart changes) and they **failed** on first run:
//! `EffectJournal::recover_after_crash`, `EffectJournal::unknown_outcomes`
//! and `blob_refs::audit_ref_counts` returned results in hash order.
//! All three now sort their output (verified: removing the sorts makes
//! the properties fail again). `ForwardHistory::snapshot` got a full
//! tie-break as a defensive fix; its property passes either way.

#[cfg(test)]
mod tests {
    use crate::blob_refs::{audit_ref_counts, rebuild_ref_counts, RefCounts};
    use crate::derived_recovery::{
        plan_projection, reconcile_outbox, OutboxAction, ProjectionCheckpoint, ProjectionPlan,
        SemanticMessageState,
    };
    use crate::dtn_recovery::{plan_dtn_restore, ForwardHistory, StoredBundle, Tombstone};
    use crate::effect_identity::{
        DeviceEventId, EffectId, IdempotentReceiver, ReceiveKey, ReceiveOutcome,
    };
    use crate::effect_journal::{EffectJournal, EffectState};
    use crate::finalization_recovery::{
        reconcile_finalization, FinalizationRecoveryAction, FinalizationState,
    };
    use crate::identity_recovery::sessions_to_invalidate;
    use proptest::prelude::*;
    use siar_blob_manifest::ids::BlobId;
    use siar_domain::{DeviceId, MessageId};
    use siar_dtn_bundle::BundleId;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use uuid::Uuid;

    fn msg(n: u16) -> MessageId {
        MessageId::from_uuid(Uuid::from_u128(n as u128 + 1))
    }
    fn dev(n: u16) -> DeviceId {
        DeviceId::from_uuid(Uuid::from_u128(n as u128 + 1))
    }
    fn bundle(n: u16) -> BundleId {
        BundleId::from_bytes(Uuid::from_u128(n as u128 + 1).into_bytes())
    }

    // ---- §129: projection fault injection -------------------------

    /// A projection whose rows are written by `seq`-keyed upsert, so
    /// replaying an already-applied event changes nothing.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct Projection {
        rows: BTreeMap<u16, u32>,
        checkpoint: u64,
    }

    #[derive(Debug, Clone, Copy)]
    struct Event {
        key: u16,
        value: u32,
    }

    impl Projection {
        fn apply_row(&mut self, e: Event) {
            self.rows.insert(e.key, e.value); // idempotent upsert
        }
    }

    fn rebuild(history: &[Event]) -> Projection {
        let mut p = Projection::default();
        for e in history {
            p.apply_row(*e);
        }
        p.checkpoint = history.len() as u64;
        p
    }

    /// Applies `history[from..]`, but "crashes" after row update `crash_after`
    /// (0-based count of events applied this run) *before* the
    /// checkpoint is persisted.
    fn run_with_crash(p: &mut Projection, history: &[Event], crash_after: Option<usize>) -> bool {
        let start = p.checkpoint as usize;
        let mut applied = 0;
        for e in &history[start..] {
            p.apply_row(*e);
            applied += 1;
            if Some(applied) == crash_after {
                return true; // crashed: checkpoint NOT advanced
            }
        }
        p.checkpoint = history.len() as u64;
        false
    }

    #[test]
    fn projection_crash_after_row_before_checkpoint_replays_idempotently() {
        let history: Vec<Event> = (0..10)
            .map(|i| Event {
                key: (i % 4) as u16,
                value: i as u32,
            })
            .collect();
        let expected = rebuild(&history);
        let mut p = Projection::default();
        assert!(run_with_crash(&mut p, &history, Some(6)));
        assert_ne!(p.checkpoint, history.len() as u64);
        // restart: plan says replay from the stale checkpoint
        let plan = plan_projection(
            Some(ProjectionCheckpoint {
                schema_version: 1,
                last_applied_seq: p.checkpoint,
            }),
            1,
            history.len() as u64,
        );
        assert!(matches!(plan, ProjectionPlan::Replay { .. }));
        assert!(!run_with_crash(&mut p, &history, None));
        assert_eq!(p, expected);
    }

    proptest! {
        // ---- §130 P7: projection rebuild equals committed history ---
        #[test]
        fn projection_recovery_equals_rebuild_for_any_crash_point(
            vals in prop::collection::vec((0u16..8, any::<u32>()), 0..40),
            crash in prop::option::of(1usize..40),
            second_crash in prop::option::of(1usize..40),
        ) {
            let history: Vec<Event> =
                vals.iter().map(|(k, v)| Event { key: *k, value: *v }).collect();
            let expected = rebuild(&history);
            let mut p = Projection::default();
            run_with_crash(&mut p, &history, crash);
            run_with_crash(&mut p, &history, second_crash); // crash during recovery too
            while run_with_crash(&mut p, &history, None) {}
            prop_assert_eq!(p, expected);
        }

        // ---- §130 P1/P2: queued intent never lost, delivered never lingers
        #[test]
        fn outbox_reconciliation_converges_and_never_loses_queued_intent(
            events in prop::collection::vec((0u16..30, 0u8..3), 0..30),
            outbox in prop::collection::vec(0u16..40, 0..30),
        ) {
            let mut ev: HashMap<MessageId, SemanticMessageState> = HashMap::new();
            for (id, st) in &events {
                let st = match st {
                    0 => SemanticMessageState::Queued,
                    1 => SemanticMessageState::Delivered,
                    _ => SemanticMessageState::Terminal,
                };
                ev.insert(msg(*id), st);
            }
            let mut ob: HashSet<MessageId> = outbox.iter().map(|i| msg(*i)).collect();
            for (id, act) in reconcile_outbox(&ev, &ob) {
                match act {
                    OutboxAction::Reconstruct => { ob.insert(id); }
                    OutboxAction::Remove => { ob.remove(&id); }
                    _ => {}
                }
            }
            for (id, st) in &ev {
                if *st == SemanticMessageState::Queued {
                    prop_assert!(ob.contains(id), "queued intent lost");
                } else {
                    prop_assert!(!ob.contains(id), "stale outbox row survived");
                }
            }
            // converged: nothing left to do except flagged unknowns
            for (_, act) in reconcile_outbox(&ev, &ob) {
                prop_assert_eq!(act, OutboxAction::FlagUnknown);
            }
        }

        // ---- §130 P3: duplicate effect doesn't duplicate state ------
        #[test]
        fn receiver_applies_each_distinct_key_exactly_once(
            keys in prop::collection::vec(0u16..20, 0..80),
        ) {
            let mut r = IdempotentReceiver::new();
            let mut processed = 0usize;
            for k in &keys {
                if r.receive(ReceiveKey::Message(msg(*k))) == ReceiveOutcome::Process {
                    processed += 1;
                }
            }
            let distinct: HashSet<_> = keys.iter().collect();
            prop_assert_eq!(processed, distinct.len());
        }

        // ---- §130 P5: revoked device never active after recovery ----
        #[test]
        fn no_session_of_a_revoked_device_survives(
            sessions in prop::collection::vec(0u16..12, 0..30),
            revoked in prop::collection::vec(0u16..12, 0..8),
        ) {
            let devs: Vec<DeviceId> = sessions.iter().map(|i| dev(*i)).collect();
            let rev: HashSet<DeviceId> = revoked.iter().map(|i| dev(*i)).collect();
            let kill: HashSet<usize> = sessions_to_invalidate(&devs, &rev).into_iter().collect();
            for (i, d) in devs.iter().enumerate() {
                if rev.contains(d) {
                    prop_assert!(kill.contains(&i), "revoked session {} left alive", i);
                } else {
                    prop_assert!(!kill.contains(&i), "healthy session killed");
                }
            }
        }

        // ---- §130 P6: expired DTN bundle never resumes forwarding ---
        #[test]
        fn expired_or_tombstoned_bundles_never_resume(
            bundles in prop::collection::vec((0u16..30, 0u64..1_000), 0..30),
            tombs in prop::collection::vec((0u16..30, 0u64..1_000), 0..10),
            now in 0u64..1_000,
        ) {
            // A real store keys bundles by id, so ids are unique.
            let unique: BTreeMap<u16, u64> = bundles.iter().copied().collect();
            let stored: Vec<StoredBundle> = unique.iter().map(|(i, exp)| StoredBundle {
                id: bundle(*i),
                expires_at_millis: *exp,
                remaining_budget: Some(1),
                initial_budget: 4,
            }).collect();
            let ts: Vec<Tombstone> = tombs.iter().map(|(i, t)| Tombstone {
                id: bundle(*i),
                retain_until_millis: *t,
            }).collect();
            let plan = plan_dtn_restore(&stored, &ts, now);
            let live_tombs: HashSet<_> =
                ts.iter().filter(|t| t.retain_until_millis > now).map(|t| t.id).collect();
            for (id, _) in &plan.keep {
                let s = stored.iter().find(|s| s.id == *id).unwrap();
                prop_assert!(s.expires_at_millis > now, "expired bundle resumed");
                prop_assert!(!live_tombs.contains(id), "tombstoned bundle resumed");
            }
        }

        // ---- §130 P4: completed blob stays complete across restart --
        #[test]
        fn a_completed_object_is_never_demoted_by_restart(_x in 0u8..1) {
            prop_assert_eq!(
                reconcile_finalization(FinalizationState::Completed, true),
                FinalizationRecoveryAction::NothingToDo
            );
        }

        // ---- §131: recovery output independent of iteration order ---
        #[test]
        fn effect_journal_recovery_is_order_independent(
            entries in prop::collection::vec((0u16..25, 0u8..5), 1..25),
            seed in any::<u64>(),
        ) {
            let to_state = |n: u8| match n {
                0 => EffectState::Pending,
                1 => EffectState::InFlight,
                2 => EffectState::Succeeded,
                3 => EffectState::Failed,
                _ => EffectState::Deferred,
            };
            let mut dedup: BTreeMap<u16, u8> = BTreeMap::new();
            for (i, s) in &entries { dedup.insert(*i, *s); }
            let mk = |i: u16| match i % 4 {
                0 => EffectId::Message(msg(i)),
                1 => EffectId::Bundle(bundle(i)),
                2 => EffectId::DeviceEvent(DeviceEventId::new_for_tests(i)),
                _ => EffectId::Transfer(crate::chunk_recovery::TransferId::new_for_tests(i)),
            };
            let forward: Vec<_> = dedup.iter().map(|(i, s)| (mk(*i), to_state(*s))).collect();
            let mut shuffled = forward.clone();
            // deterministic pseudo-shuffle from `seed`
            let mut x = seed | 1;
            for i in (1..shuffled.len()).rev() {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                shuffled.swap(i, (x % (i as u64 + 1)) as usize);
            }
            let a = EffectJournal::from_durable(forward);
            let b = EffectJournal::from_durable(shuffled);
            prop_assert_eq!(a.recover_after_crash(), b.recover_after_crash());
            prop_assert_eq!(a.unknown_outcomes(), b.unknown_outcomes());
        }

        #[test]
        fn ref_count_audit_is_order_independent(
            auth_refs in prop::collection::vec(0u8..12, 0..40),
            cached in prop::collection::vec((0u8..12, 0u64..5), 0..12),
        ) {
            let auth = rebuild_ref_counts(auth_refs.iter().map(|b| BlobId::from_ciphertext(&[*b])));
            let mut c1 = RefCounts::new();
            for (b, n) in &cached { c1.insert(BlobId::from_ciphertext(&[*b]), *n); }
            let mut c2 = RefCounts::new();
            for (b, n) in cached.iter().rev() { c2.entry(BlobId::from_ciphertext(&[*b])).or_insert(*n); }
            // c1 keeps the LAST value per key, c2 the first of the reversed
            // order == the same last value: identical maps, built differently.
            prop_assert_eq!(&c1, &c2);
            prop_assert_eq!(audit_ref_counts(&c1, &auth), audit_ref_counts(&c2, &auth));
            let a1 = audit_ref_counts(&c1, &auth);
            let keys: Vec<_> = a1.iter().map(|d| *d.blob.as_bytes()).collect();
            let mut sorted = keys.clone();
            sorted.sort();
            prop_assert_eq!(keys, sorted);
        }

        #[test]
        fn forward_history_snapshot_is_deterministic(
            recs in prop::collection::vec((0u16..6, 0u16..6, 0u64..4), 0..30),
        ) {
            let build = |it: &mut dyn Iterator<Item = &(u16, u16, u64)>| {
                let mut h = ForwardHistory::new(1_000);
                for (p, b, t) in it { h.record(dev(*p), bundle(*b), *t); }
                h.snapshot()
            };
            let a = build(&mut recs.iter());
            let b = build(&mut recs.iter().rev());
            // same (peer,bundle)->max time regardless of insertion order
            prop_assert_eq!(a, b);
        }
    }
}
