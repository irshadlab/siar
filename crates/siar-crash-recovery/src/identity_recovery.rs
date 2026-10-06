//! §31 "Identity Recovery", §32 "Identity Fork Recovery", §33
//! "Revocation Recovery".
//!
//! Part 02 identity state is security critical, so every rule here
//! fails *closed*: an identity check that cannot be performed counts
//! as failed, a fork is never merged automatically, and a committed
//! revocation can never be undone by a crash. This module takes plain
//! summaries rather than depending on `siar-identity-multidevice`
//! (whose verification stays that crate's job); it decides what
//! recovery does with the results.

use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;
use std::collections::HashSet;

// ---------------------------------------------------------------------
// §31
// ---------------------------------------------------------------------

/// Outcome of one verification the identity crate performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Pass,
    Fail,
    /// Could not be evaluated (missing data, I/O error). Treated as a
    /// failure: unverifiable identity state is not trusted.
    NotVerifiable,
}

impl CheckOutcome {
    const fn ok(self) -> bool {
        matches!(self, CheckOutcome::Pass)
    }
}

/// §31's five required verifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityEvidence {
    pub device_event_chain: CheckOutcome,
    /// Highest generation per the persisted account state.
    pub persisted_highest_generation: u64,
    /// Highest generation proven by the verified device-event chain.
    pub chain_highest_generation: u64,
    pub persisted_revoked: HashSet<DeviceId>,
    pub chain_revoked: HashSet<DeviceId>,
    pub root_continuity: CheckOutcome,
    pub certificate_integrity: CheckOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityFailure {
    DeviceEventChain,
    /// Persisted state is behind the chain (rollback / lost write).
    GenerationBehindChain {
        persisted: u64,
        chain: u64,
    },
    /// Persisted state claims more than the chain proves.
    GenerationAheadOfChain {
        persisted: u64,
        chain: u64,
    },
    /// Chain says revoked, persisted set does not — the dangerous
    /// direction.
    RevocationMissing(Vec<DeviceId>),
    /// Persisted set revokes devices the chain never revoked.
    RevocationUnexplained(Vec<DeviceId>),
    RootContinuity,
    CertificateIntegrity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityVerdict {
    Consistent,
    /// §31: "fail closed for trusted network operations". Local,
    /// read-only access may continue; anything that asserts or relies
    /// on identity to a peer must not.
    FailClosed(Vec<IdentityFailure>),
}

impl IdentityVerdict {
    pub fn allows_trusted_network_operations(&self) -> bool {
        matches!(self, IdentityVerdict::Consistent)
    }
}

fn sorted(set: impl Iterator<Item = DeviceId>) -> Vec<DeviceId> {
    let mut v: Vec<_> = set.collect();
    v.sort_by_key(|d| d.as_uuid());
    v
}

/// Runs all five §31 checks and reports every failure, not just the
/// first.
pub fn verify_identity(e: &IdentityEvidence) -> IdentityVerdict {
    let mut f = Vec::new();
    if !e.device_event_chain.ok() {
        f.push(IdentityFailure::DeviceEventChain);
    }
    match e
        .persisted_highest_generation
        .cmp(&e.chain_highest_generation)
    {
        std::cmp::Ordering::Less => f.push(IdentityFailure::GenerationBehindChain {
            persisted: e.persisted_highest_generation,
            chain: e.chain_highest_generation,
        }),
        std::cmp::Ordering::Greater => f.push(IdentityFailure::GenerationAheadOfChain {
            persisted: e.persisted_highest_generation,
            chain: e.chain_highest_generation,
        }),
        std::cmp::Ordering::Equal => {}
    }
    let missing = sorted(e.chain_revoked.difference(&e.persisted_revoked).copied());
    if !missing.is_empty() {
        f.push(IdentityFailure::RevocationMissing(missing));
    }
    let extra = sorted(e.persisted_revoked.difference(&e.chain_revoked).copied());
    if !extra.is_empty() {
        f.push(IdentityFailure::RevocationUnexplained(extra));
    }
    if !e.root_continuity.ok() {
        f.push(IdentityFailure::RootContinuity);
    }
    if !e.certificate_integrity.ok() {
        f.push(IdentityFailure::CertificateIntegrity);
    }
    if f.is_empty() {
        IdentityVerdict::Consistent
    } else {
        IdentityVerdict::FailClosed(f)
    }
}

// ---------------------------------------------------------------------
// §32
// ---------------------------------------------------------------------

/// A candidate account state: its generation and a hash of its
/// canonical content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountStateSummary {
    pub generation: u64,
    pub state_hash: [u8; 32],
}

/// §32's `IdentityForkDetected`: two or more *different* states claim
/// the same generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityForkDetected {
    pub generation: u64,
    /// Distinct conflicting hashes, sorted.
    pub conflicting_hashes: Vec<[u8; 32]>,
}

/// Finds every generation with conflicting states. Identical
/// duplicates (same generation, same hash) are not a fork.
pub fn detect_identity_forks(states: &[AccountStateSummary]) -> Vec<IdentityForkDetected> {
    use std::collections::BTreeMap;
    let mut by_gen: BTreeMap<u64, Vec<[u8; 32]>> = BTreeMap::new();
    for s in states {
        let v = by_gen.entry(s.generation).or_default();
        if !v.contains(&s.state_hash) {
            v.push(s.state_hash);
        }
    }
    by_gen
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(generation, mut v)| {
            v.sort_unstable();
            IdentityForkDetected {
                generation,
                conflicting_hashes: v,
            }
        })
        .collect()
}

/// §32: "do not auto-merge." There is deliberately **no** function in
/// this module that picks a winner from a fork. The only way forward
/// is an [`ExplicitReconciliation`], which can only be built from a
/// user-confirmed choice among the fork's own candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExplicitReconciliation {
    generation: u64,
    chosen_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReconciliationError {
    #[error("reconciliation requires explicit user confirmation")]
    NotConfirmed,
    #[error("chosen state is not one of the fork's candidates")]
    NotACandidate,
}

impl ExplicitReconciliation {
    pub fn from_user_choice(
        fork: &IdentityForkDetected,
        chosen_hash: [u8; 32],
        user_confirmed: bool,
    ) -> Result<Self, ReconciliationError> {
        if !user_confirmed {
            return Err(ReconciliationError::NotConfirmed);
        }
        if !fork.conflicting_hashes.contains(&chosen_hash) {
            return Err(ReconciliationError::NotACandidate);
        }
        Ok(Self {
            generation: fork.generation,
            chosen_hash,
        })
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn chosen_hash(&self) -> &[u8; 32] {
        &self.chosen_hash
    }
}

// ---------------------------------------------------------------------
// §33
// ---------------------------------------------------------------------

/// §33's safe flow, in order. Only stages ≥ `Committed` are durable
/// evidence that the user's approval took effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RevocationStage {
    SignedAppended,
    Committed,
    SessionsInvalidated,
    Propagated,
}

/// What recovery observed about one approved revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevocationFacts {
    /// Is the signed revocation present in committed storage?
    pub committed: bool,
    /// Are all local sessions for the revoked device already invalid?
    pub sessions_invalid: bool,
    pub propagated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevocationRecovery {
    /// Must be done before the runtime serves trusted traffic.
    pub invalidate_sessions: bool,
    /// May run asynchronously after startup.
    pub propagate: bool,
    /// The crash predates the commit, so the approval is not durable
    /// and (per §9, the uncommitted append is rolled back) the user
    /// must be told and asked again. This is the only case in which
    /// an approved revocation can be lost, and it is surfaced, never
    /// silent.
    pub reapproval_required: bool,
}

pub const fn recover_revocation(f: RevocationFacts) -> RevocationRecovery {
    if !f.committed {
        return RevocationRecovery {
            invalidate_sessions: false,
            propagate: false,
            reapproval_required: true,
        };
    }
    RevocationRecovery {
        invalidate_sessions: !f.sessions_invalid,
        propagate: !f.propagated,
        reapproval_required: false,
    }
}

/// §33: "recovery sees revocation → sessions remain invalid." Returns
/// the indexes of sessions that must be killed: every session whose
/// peer device is in the committed revoked set. Idempotent by
/// construction (a pure filter).
pub fn sessions_to_invalidate(
    session_devices: &[DeviceId],
    committed_revoked: &HashSet<DeviceId>,
) -> Vec<usize> {
    session_devices
        .iter()
        .enumerate()
        .filter(|(_, d)| committed_revoked.contains(d))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheckOutcome::*;

    fn good() -> IdentityEvidence {
        IdentityEvidence {
            device_event_chain: Pass,
            persisted_highest_generation: 4,
            chain_highest_generation: 4,
            persisted_revoked: HashSet::new(),
            chain_revoked: HashSet::new(),
            root_continuity: Pass,
            certificate_integrity: Pass,
        }
    }

    #[test]
    fn clean_evidence_is_consistent_and_allows_trusted_ops() {
        let v = verify_identity(&good());
        assert_eq!(v, IdentityVerdict::Consistent);
        assert!(v.allows_trusted_network_operations());
    }

    #[test]
    fn each_check_failing_fails_closed() {
        let cases: Vec<(IdentityEvidence, IdentityFailure)> = vec![
            (
                IdentityEvidence {
                    device_event_chain: Fail,
                    ..good()
                },
                IdentityFailure::DeviceEventChain,
            ),
            (
                IdentityEvidence {
                    root_continuity: Fail,
                    ..good()
                },
                IdentityFailure::RootContinuity,
            ),
            (
                IdentityEvidence {
                    certificate_integrity: Fail,
                    ..good()
                },
                IdentityFailure::CertificateIntegrity,
            ),
            (
                IdentityEvidence {
                    persisted_highest_generation: 3,
                    ..good()
                },
                IdentityFailure::GenerationBehindChain {
                    persisted: 3,
                    chain: 4,
                },
            ),
            (
                IdentityEvidence {
                    persisted_highest_generation: 9,
                    ..good()
                },
                IdentityFailure::GenerationAheadOfChain {
                    persisted: 9,
                    chain: 4,
                },
            ),
        ];
        for (ev, expected) in cases {
            match verify_identity(&ev) {
                IdentityVerdict::FailClosed(f) => assert!(f.contains(&expected), "{expected:?}"),
                IdentityVerdict::Consistent => panic!("{expected:?} should fail closed"),
            }
        }
    }

    #[test]
    fn unverifiable_is_not_trusted() {
        let ev = IdentityEvidence {
            root_continuity: NotVerifiable,
            ..good()
        };
        assert!(!verify_identity(&ev).allows_trusted_network_operations());
    }

    #[test]
    fn revocation_set_mismatch_is_reported_in_both_directions() {
        let (a, b) = (DeviceId::new(), DeviceId::new());
        let ev = IdentityEvidence {
            chain_revoked: [a].into(),
            persisted_revoked: [b].into(),
            ..good()
        };
        let IdentityVerdict::FailClosed(f) = verify_identity(&ev) else {
            panic!("must fail closed");
        };
        assert!(f.contains(&IdentityFailure::RevocationMissing(vec![a])));
        assert!(f.contains(&IdentityFailure::RevocationUnexplained(vec![b])));
    }

    #[test]
    fn all_failures_are_reported_not_just_the_first() {
        let ev = IdentityEvidence {
            device_event_chain: Fail,
            root_continuity: Fail,
            certificate_integrity: Fail,
            ..good()
        };
        let IdentityVerdict::FailClosed(f) = verify_identity(&ev) else {
            panic!()
        };
        assert_eq!(f.len(), 3);
    }

    fn st(g: u64, h: u8) -> AccountStateSummary {
        AccountStateSummary {
            generation: g,
            state_hash: [h; 32],
        }
    }

    #[test]
    fn same_generation_different_state_is_a_fork() {
        let forks = detect_identity_forks(&[st(1, 1), st(2, 7), st(2, 8), st(3, 1)]);
        assert_eq!(forks.len(), 1);
        assert_eq!(forks[0].generation, 2);
        assert_eq!(forks[0].conflicting_hashes, vec![[7; 32], [8; 32]]);
    }

    #[test]
    fn identical_duplicates_and_distinct_generations_are_not_forks() {
        assert!(detect_identity_forks(&[st(1, 1), st(1, 1), st(2, 2)]).is_empty());
        assert!(detect_identity_forks(&[]).is_empty());
    }

    #[test]
    fn reconciliation_needs_user_confirmation_and_a_real_candidate() {
        let fork = detect_identity_forks(&[st(2, 7), st(2, 8)]).remove(0);
        assert_eq!(
            ExplicitReconciliation::from_user_choice(&fork, [7; 32], false),
            Err(ReconciliationError::NotConfirmed)
        );
        assert_eq!(
            ExplicitReconciliation::from_user_choice(&fork, [9; 32], true),
            Err(ReconciliationError::NotACandidate)
        );
        let ok = ExplicitReconciliation::from_user_choice(&fork, [7; 32], true).unwrap();
        assert_eq!((ok.generation(), *ok.chosen_hash()), (2, [7; 32]));
    }

    #[test]
    fn revocation_stages_are_ordered() {
        assert!(RevocationStage::SignedAppended < RevocationStage::Committed);
        assert!(RevocationStage::Committed < RevocationStage::SessionsInvalidated);
        assert!(RevocationStage::SessionsInvalidated < RevocationStage::Propagated);
    }

    #[test]
    fn crash_after_commit_keeps_revocation_and_finishes_the_rest() {
        let r = recover_revocation(RevocationFacts {
            committed: true,
            sessions_invalid: false,
            propagated: false,
        });
        assert!(r.invalidate_sessions && r.propagate && !r.reapproval_required);
    }

    #[test]
    fn fully_done_revocation_needs_nothing_and_is_idempotent() {
        let f = RevocationFacts {
            committed: true,
            sessions_invalid: true,
            propagated: true,
        };
        let r = recover_revocation(f);
        assert!(!r.invalidate_sessions && !r.propagate && !r.reapproval_required);
        assert_eq!(recover_revocation(f), r);
    }

    #[test]
    fn crash_before_commit_surfaces_reapproval_instead_of_pretending() {
        let r = recover_revocation(RevocationFacts {
            committed: false,
            sessions_invalid: false,
            propagated: false,
        });
        assert!(r.reapproval_required);
        assert!(!r.invalidate_sessions && !r.propagate);
    }

    #[test]
    fn only_sessions_of_revoked_devices_are_invalidated() {
        let (a, b, c) = (DeviceId::new(), DeviceId::new(), DeviceId::new());
        let revoked: HashSet<_> = [b].into();
        assert_eq!(sessions_to_invalidate(&[a, b, c, b], &revoked), vec![1, 3]);
        assert!(sessions_to_invalidate(&[a, c], &revoked).is_empty());
    }
}
