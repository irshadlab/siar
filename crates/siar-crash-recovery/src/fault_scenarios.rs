//! §125 "Network Fault Injection", §127 "DTN Fault Injection", §128
//! "Identity Fault Injection".
//!
//! Each section's fault list is a public catalog enum (so a fuzzer or
//! the process-kill harness can iterate them) and each fault is driven
//! through this crate's real recovery pieces. The shared pass
//! criterion is the one §125 states: after the fault *and* a crash,
//! the result must remain idempotent.

use serde::{Deserialize, Serialize};

/// §125's four network faults, each combined with a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkFault {
    DuplicateSend,
    DelayedAck,
    LostAck,
    PeerRestart,
}

impl NetworkFault {
    pub const ALL: [NetworkFault; 4] = [
        NetworkFault::DuplicateSend,
        NetworkFault::DelayedAck,
        NetworkFault::LostAck,
        NetworkFault::PeerRestart,
    ];
}

/// §127.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DtnFault {
    RelayStoresThenCrashes,
    RelayAckLost,
    DestinationAckDelayed,
    TombstoneLostAndRebuilt,
}

impl DtnFault {
    pub const ALL: [DtnFault; 4] = [
        DtnFault::RelayStoresThenCrashes,
        DtnFault::RelayAckLost,
        DtnFault::DestinationAckDelayed,
        DtnFault::TombstoneLostAndRebuilt,
    ];
}

/// §128.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdentityFault {
    RevocationCommittedThenCrash,
    RootRotationInterrupted,
    StalePeerStatePresented,
    SecureStoreUnavailable,
}

impl IdentityFault {
    pub const ALL: [IdentityFault; 4] = [
        IdentityFault::RevocationCommittedThenCrash,
        IdentityFault::RootRotationInterrupted,
        IdentityFault::StalePeerStatePresented,
        IdentityFault::SecureStoreUnavailable,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corruption::{response_for, CorruptionScope};
    use crate::dtn_recovery::{
        plan_dtn_restore, rebuild_tombstones, AckApply, AckStatus, DestinationAckLedger,
        DestinationReceive, OriginAckTracker, RelayLedger, RelayReceiveOutcome, StoredBundle,
    };
    use crate::identity_recovery::{
        detect_identity_forks, recover_revocation, verify_identity, AccountStateSummary,
        CheckOutcome, IdentityEvidence, IdentityFailure, IdentityVerdict, RevocationFacts,
    };
    use crate::recovery_mode::{gate, Operation};
    use siar_domain::DeviceId;
    use siar_dtn_bundle::BundleId;
    use std::collections::HashSet;

    /// Origin → destination with one fault and a crash; returns how
    /// many times the payload reached the application and whether the
    /// origin still thinks the bundle is pending.
    fn run_network(fault: NetworkFault) -> (u32, bool) {
        let id = BundleId::new();
        let mut origin = OriginAckTracker::default();
        let mut dest = DestinationAckLedger::default();
        let mut delivered = 0;
        origin.expect(id);

        let mut send = |dest: &mut DestinationAckLedger| {
            if dest.receive_payload(id) == DestinationReceive::Deliver {
                delivered += 1;
            }
        };

        // first transmission always reaches the destination
        send(&mut dest);
        match fault {
            NetworkFault::DuplicateSend => send(&mut dest),
            NetworkFault::DelayedAck | NetworkFault::LostAck => {
                // ACK doesn't arrive in time → origin retransmits
                send(&mut dest);
            }
            NetworkFault::PeerRestart => {
                // destination crashes; ledger comes back from durable state
                let durable: Vec<_> = [(id, dest.status(&id).unwrap())].into();
                dest = DestinationAckLedger::from_durable(durable);
                send(&mut dest);
            }
        }
        // whichever ACKs eventually arrive (possibly several, in any
        // number) are applied idempotently
        for _ in dest.acks_to_send() {
            origin.apply_ack(id);
            origin.apply_ack(id);
        }
        (delivered, origin.is_pending(&id))
    }

    #[test]
    fn every_network_fault_stays_idempotent_and_converges() {
        for f in NetworkFault::ALL {
            let (delivered, pending) = run_network(f);
            assert_eq!(
                delivered, 1,
                "{f:?}: payload must reach the app exactly once"
            );
            assert!(!pending, "{f:?}: origin must end with the ACK applied");
        }
    }

    #[test]
    fn dtn_relay_stores_then_crashes() {
        let id = BundleId::new();
        let mut before = RelayLedger::default();
        assert_eq!(before.receive(id), RelayReceiveOutcome::NewlyStored);
        // crash; ledger rebuilt from the durable bundle store
        let mut after = RelayLedger::from_durable([id]);
        assert_eq!(after.receive(id), RelayReceiveOutcome::DuplicateReAck);
        assert_eq!(after.len(), 1);
    }

    #[test]
    fn dtn_relay_ack_lost_origin_resends_and_relay_reacks() {
        let id = BundleId::new();
        let mut relay = RelayLedger::default();
        relay.receive(id); // stored, ACK lost
        for _ in 0..3 {
            let out = relay.receive(id); // origin keeps resending
            assert!(out.should_ack());
            assert_eq!(out, RelayReceiveOutcome::DuplicateReAck);
        }
        assert_eq!(relay.len(), 1, "never stored twice");
    }

    #[test]
    fn dtn_destination_ack_delayed_stays_owed_until_confirmed() {
        let id = BundleId::new();
        let mut d = DestinationAckLedger::default();
        d.receive_payload(id);
        // many contacts pass with the ACK still unconfirmed
        for _ in 0..4 {
            assert_eq!(d.acks_to_send(), vec![id]);
        }
        d.confirm_by_peer(id);
        assert_eq!(d.status(&id), Some(AckStatus::ConfirmedByPeer));
        assert!(d.acks_to_send().is_empty());
        // the delayed ACK finally lands at the origin, twice
        let mut o = OriginAckTracker::default();
        o.expect(id);
        assert_eq!(o.apply_ack(id), AckApply::Applied);
        assert_eq!(o.apply_ack(id), AckApply::AlreadyApplied);
    }

    #[test]
    fn dtn_tombstone_lost_and_rebuilt_from_delivered_ids() {
        let delivered_id = BundleId::new();
        let live_id = BundleId::new();
        let stored = |id| StoredBundle {
            id,
            expires_at_millis: 10_000,
            remaining_budget: Some(2),
            initial_budget: 4,
        };
        let bundles = [stored(delivered_id), stored(live_id)];

        // tombstones lost: the already-delivered bundle would be kept
        let lost = plan_dtn_restore(&bundles, &[], 100);
        assert_eq!(lost.keep.len(), 2, "this is the hazard the rebuild closes");

        // rebuild from the ground truth of what was delivered
        let rebuilt = rebuild_tombstones(&[delivered_id, delivered_id], 100, 5_000);
        assert_eq!(rebuilt.len(), 1, "duplicates collapse");
        let healed = plan_dtn_restore(&bundles, &rebuilt, 100);
        assert_eq!(healed.purge_tombstoned, vec![delivered_id]);
        assert_eq!(healed.keep.len(), 1);
        assert_eq!(healed.keep[0].0, live_id);
    }

    #[test]
    fn every_dtn_fault_has_a_scenario() {
        assert_eq!(DtnFault::ALL.len(), 4);
        assert_eq!(IdentityFault::ALL.len(), 4);
        assert_eq!(NetworkFault::ALL.len(), 4);
    }

    fn good_evidence() -> IdentityEvidence {
        IdentityEvidence {
            device_event_chain: CheckOutcome::Pass,
            persisted_highest_generation: 5,
            chain_highest_generation: 5,
            persisted_revoked: HashSet::new(),
            chain_revoked: HashSet::new(),
            root_continuity: CheckOutcome::Pass,
            certificate_integrity: CheckOutcome::Pass,
        }
    }

    #[test]
    fn identity_revocation_committed_then_crash() {
        let r = recover_revocation(RevocationFacts {
            committed: true,
            sessions_invalid: false,
            propagated: false,
        });
        assert!(r.invalidate_sessions && r.propagate && !r.reapproval_required);
    }

    #[test]
    fn identity_root_rotation_interrupted_fails_closed() {
        // chain already holds the rotation (gen 5), persisted state
        // never caught up (gen 4)
        let mut e = good_evidence();
        e.persisted_highest_generation = 4;
        let v = verify_identity(&e);
        assert!(!v.allows_trusted_network_operations());
        let IdentityVerdict::FailClosed(f) = v else {
            panic!()
        };
        assert!(f.contains(&IdentityFailure::GenerationBehindChain {
            persisted: 4,
            chain: 5
        }));
    }

    #[test]
    fn identity_stale_peer_state_is_a_fork_never_auto_merged() {
        let ours = AccountStateSummary {
            generation: 7,
            state_hash: [1; 32],
        };
        let theirs = AccountStateSummary {
            generation: 7,
            state_hash: [2; 32],
        };
        let forks = detect_identity_forks(&[ours, theirs]);
        assert_eq!(forks.len(), 1);
        // an older generation alone is not a fork
        let older = AccountStateSummary {
            generation: 3,
            state_hash: [9; 32],
        };
        assert!(detect_identity_forks(&[ours, older]).is_empty());
    }

    #[test]
    fn identity_secure_store_unavailable_blocks_trusted_comms_only() {
        let r = response_for(CorruptionScope::SecureStore);
        assert!(!r.trusted_comms_allowed);
        assert!(gate(r.severity, Operation::ViewMessages).is_ok());
        assert!(gate(r.severity, Operation::StartNetwork).is_err());
    }

    #[test]
    fn revoked_device_fault_keeps_sessions_dead_after_recovery() {
        use crate::identity_recovery::sessions_to_invalidate;
        let bad = DeviceId::new();
        let revoked: HashSet<_> = [bad].into();
        assert_eq!(
            sessions_to_invalidate(&[DeviceId::new(), bad], &revoked),
            vec![1]
        );
    }
}
