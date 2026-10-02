//! §28-32: downgrade resistance.
//!
//! Three independent defences, none of which trusts the peer's stated
//! preference order:
//! 1. [`select_algorithm`] (§29): strongest mutually supported **and**
//!    locally allowed, ranked by a catalog both sides share, so both
//!    peers pick the same algorithm regardless of whose list is read.
//! 2. [`MinimumSecurityProfile`] (§31): a hard local floor.
//!    Compatibility never overrides it.
//! 3. [`SecurityBaseline`] (§32): a ratchet of the highest generation
//!    seen from a verified contact. A drop yields
//!    [`SecurityVerdict::DowngradeSuspected`] and the caller's
//!    [`BaselinePolicy`] decides — the spec warns that device/platform
//!    changes are legitimate, so a drop is not auto-rejected.
//!
//! The authenticated-transcript half of §28 is `transcript.rs`.

use serde::{Deserialize, Serialize};

/// §30: algorithms are numeric IDs so migration doesn't change types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AlgorithmId(pub u16);

/// Monotonic "how modern is this session's security suite" number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SecurityGeneration(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MinimumSecurityProfile {
    Legacy,
    Standard,
    Modern,
}

impl MinimumSecurityProfile {
    /// Lowest [`SecurityGeneration`] this profile accepts.
    pub const fn minimum_generation(self) -> SecurityGeneration {
        match self {
            Self::Legacy => SecurityGeneration(1),
            Self::Standard => SecurityGeneration(2),
            Self::Modern => SecurityGeneration(3),
        }
    }
}

/// A shared strength ranking. Higher `strength` wins; ties break on the
/// larger `AlgorithmId`, so the choice is total and deterministic.
#[derive(Debug, Clone, Default)]
pub struct AlgorithmCatalog {
    entries: Vec<(AlgorithmId, u16)>,
}

impl AlgorithmCatalog {
    pub fn new(entries: impl IntoIterator<Item = (AlgorithmId, u16)>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
        }
    }

    fn strength(&self, id: AlgorithmId) -> Option<u16> {
        self.entries.iter().find(|(a, _)| *a == id).map(|(_, s)| *s)
    }
}

/// Picks the strongest algorithm that is in the catalog, locally allowed,
/// and offered by the peer. Unknown algorithms are never selectable —
/// an attacker can't win by advertising an ID the catalog can't rank.
pub fn select_algorithm(
    catalog: &AlgorithmCatalog,
    locally_allowed: &[AlgorithmId],
    remote_offered: &[AlgorithmId],
) -> Option<AlgorithmId> {
    locally_allowed
        .iter()
        .filter(|a| remote_offered.contains(a))
        .filter_map(|a| catalog.strength(*a).map(|s| (s, *a)))
        .max()
        .map(|(_, a)| a)
}

/// §32: highest generation previously observed from a verified contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityBaseline {
    pub highest_generation: SecurityGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaselinePolicy {
    /// Surface the suspicion, let the session proceed.
    WarnOnly,
    /// Refuse sessions below the remembered baseline.
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityVerdict {
    Acceptable,
    /// Hard failure: below the local floor. Never overridable by baseline policy.
    BelowMinimumProfile {
        required: SecurityGeneration,
        offered: SecurityGeneration,
    },
    /// Session is below what this contact used before.
    DowngradeSuspected {
        previous: SecurityGeneration,
        observed: SecurityGeneration,
        reject: bool,
    },
}

impl SecurityVerdict {
    pub fn allows_session(&self) -> bool {
        match self {
            Self::Acceptable => true,
            Self::BelowMinimumProfile { .. } => false,
            Self::DowngradeSuspected { reject, .. } => !reject,
        }
    }
}

pub fn assess_session(
    floor: MinimumSecurityProfile,
    baseline: Option<SecurityBaseline>,
    policy: BaselinePolicy,
    negotiated: SecurityGeneration,
) -> SecurityVerdict {
    let required = floor.minimum_generation();
    if negotiated < required {
        return SecurityVerdict::BelowMinimumProfile {
            required,
            offered: negotiated,
        };
    }
    match baseline {
        Some(b) if negotiated < b.highest_generation => SecurityVerdict::DowngradeSuspected {
            previous: b.highest_generation,
            observed: negotiated,
            reject: policy == BaselinePolicy::Reject,
        },
        _ => SecurityVerdict::Acceptable,
    }
}

impl SecurityBaseline {
    /// Ratchet: only ever raises. Call only after the session is
    /// authenticated (transcript confirmed); updating from an
    /// unauthenticated claim would let an attacker poison the baseline.
    pub fn raised_by(self, observed: SecurityGeneration) -> Self {
        Self {
            highest_generation: self.highest_generation.max(observed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: AlgorithmId = AlgorithmId(1);
    const B: AlgorithmId = AlgorithmId(2);
    const C: AlgorithmId = AlgorithmId(3);

    fn catalog() -> AlgorithmCatalog {
        AlgorithmCatalog::new([(A, 10), (B, 20), (C, 30)])
    }

    #[test]
    fn strongest_mutual_allowed_wins_regardless_of_peer_order() {
        let c = catalog();
        assert_eq!(select_algorithm(&c, &[A, B, C], &[A, B, C]), Some(C));
        assert_eq!(select_algorithm(&c, &[A, B, C], &[A, B]), Some(B));
        // Peer lists the weak one first; irrelevant.
        assert_eq!(select_algorithm(&c, &[C, B, A], &[A, B]), Some(B));
    }

    #[test]
    fn locally_disallowed_strong_algorithm_is_never_chosen() {
        assert_eq!(select_algorithm(&catalog(), &[A], &[A, B, C]), Some(A));
        assert_eq!(select_algorithm(&catalog(), &[A], &[C]), None);
    }

    #[test]
    fn unknown_ids_are_not_selectable() {
        let weird = AlgorithmId(999);
        assert_eq!(select_algorithm(&catalog(), &[weird], &[weird]), None);
    }

    #[test]
    fn selection_is_symmetric() {
        let c = catalog();
        let (x, y) = ([A, B, C], [B, C]);
        assert_eq!(select_algorithm(&c, &x, &y), select_algorithm(&c, &y, &x));
    }

    #[test]
    fn floor_is_a_hard_reject_even_with_lenient_baseline_policy() {
        let v = assess_session(
            MinimumSecurityProfile::Modern,
            None,
            BaselinePolicy::WarnOnly,
            SecurityGeneration(2),
        );
        assert!(matches!(v, SecurityVerdict::BelowMinimumProfile { .. }));
        assert!(!v.allows_session());
    }

    #[test]
    fn baseline_drop_is_suspected_and_policy_decides() {
        let base = Some(SecurityBaseline {
            highest_generation: SecurityGeneration(4),
        });
        let warn = assess_session(
            MinimumSecurityProfile::Standard,
            base,
            BaselinePolicy::WarnOnly,
            SecurityGeneration(3),
        );
        assert!(matches!(
            warn,
            SecurityVerdict::DowngradeSuspected { reject: false, .. }
        ));
        assert!(warn.allows_session());
        let rej = assess_session(
            MinimumSecurityProfile::Standard,
            base,
            BaselinePolicy::Reject,
            SecurityGeneration(3),
        );
        assert!(!rej.allows_session());
    }

    #[test]
    fn no_baseline_or_equal_generation_is_acceptable() {
        let g = SecurityGeneration(3);
        assert_eq!(
            assess_session(
                MinimumSecurityProfile::Modern,
                None,
                BaselinePolicy::Reject,
                g
            ),
            SecurityVerdict::Acceptable
        );
        let base = Some(SecurityBaseline {
            highest_generation: g,
        });
        assert_eq!(
            assess_session(
                MinimumSecurityProfile::Modern,
                base,
                BaselinePolicy::Reject,
                g
            ),
            SecurityVerdict::Acceptable
        );
    }

    #[test]
    fn baseline_only_ratchets_up() {
        let b = SecurityBaseline {
            highest_generation: SecurityGeneration(3),
        };
        assert_eq!(
            b.raised_by(SecurityGeneration(2)).highest_generation,
            SecurityGeneration(3)
        );
        assert_eq!(
            b.raised_by(SecurityGeneration(5)).highest_generation,
            SecurityGeneration(5)
        );
    }
}
