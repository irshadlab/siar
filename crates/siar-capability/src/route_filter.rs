//! §79 Route Integration: Part 03 filters candidate sessions by the
//! capabilities an operation needs ("file transfer requires `files/1`";
//! a candidate without it is *invalid for operation*).
//!
//! Three outcomes, because "we have not negotiated with this candidate yet"
//! is not the same as "it lacks the capability": the first must be resolved
//! (negotiate or consult the cache), the second removes the candidate.

use crate::id::CapabilityId;
use crate::set::CapabilitySet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateVerdict {
    /// Every required capability is in the negotiated set.
    Capable,
    /// Negotiated, and this capability is missing: invalid for the operation.
    Lacks(CapabilityId),
    /// No negotiated set for this candidate yet.
    NotNegotiated,
}

/// Matches on capability id (any version): version compatibility was already
/// settled when the set was negotiated.
pub fn check_candidate(
    negotiated: Option<&CapabilitySet>,
    required: &[CapabilityId],
) -> CandidateVerdict {
    let Some(set) = negotiated else {
        return CandidateVerdict::NotNegotiated;
    };
    match required.iter().find(|id| !set.contains(id)) {
        Some(missing) => CandidateVerdict::Lacks(*missing),
        None => CandidateVerdict::Capable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partitioned<C> {
    /// Usable for the operation now, in the caller's original order.
    pub capable: Vec<C>,
    /// Unknown yet: the router may negotiate these, but must not treat them as capable.
    pub not_negotiated: Vec<C>,
}

/// Splits candidates by [`check_candidate`]; candidates that lack a required
/// capability are dropped. Original order is preserved in both lists, so a
/// router's own ranking (§71/§72) is not disturbed by filtering.
pub fn partition_candidates<'a, C>(
    candidates: impl IntoIterator<Item = C>,
    required: &[CapabilityId],
    negotiated_of: impl Fn(&C) -> Option<&'a CapabilitySet>,
) -> Partitioned<C> {
    let mut out = Partitioned {
        capable: Vec::new(),
        not_negotiated: Vec::new(),
    };
    for c in candidates {
        match check_candidate(negotiated_of(&c), required) {
            CandidateVerdict::Capable => out.capable.push(c),
            CandidateVerdict::NotNegotiated => out.not_negotiated.push(c),
            CandidateVerdict::Lacks(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityNamespace;
    use crate::version::CapabilityVersion;

    const FILES: CapabilityId = CapabilityId::new(CapabilityNamespace::Files, 1);
    const DTN: CapabilityId = CapabilityId::new(CapabilityNamespace::Dtn, 1);

    fn set_of(ids: &[CapabilityId]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for id in ids {
            s.insert(CapabilityDescriptor::new(
                *id,
                CapabilityVersion::new(1, 0),
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        }
        s
    }

    #[test]
    fn verdicts_distinguish_lacking_from_not_yet_negotiated() {
        let has = set_of(&[FILES]);
        assert_eq!(
            check_candidate(Some(&has), &[FILES]),
            CandidateVerdict::Capable
        );
        assert_eq!(
            check_candidate(Some(&has), &[FILES, DTN]),
            CandidateVerdict::Lacks(DTN)
        );
        assert_eq!(
            check_candidate(None, &[FILES]),
            CandidateVerdict::NotNegotiated
        );
        // Nothing required: any negotiated candidate qualifies.
        assert_eq!(check_candidate(Some(&has), &[]), CandidateVerdict::Capable);
    }

    #[test]
    fn partition_drops_incapable_keeps_unknown_apart_and_preserves_order() {
        let a = set_of(&[FILES]);
        let b = set_of(&[DTN]);
        let c = set_of(&[FILES, DTN]);
        let cands = vec![
            ("a", Some(&a)),
            ("b", Some(&b)),
            ("pending", None),
            ("c", Some(&c)),
        ];
        let p = partition_candidates(cands, &[FILES], |x| x.1);
        assert_eq!(
            p.capable.iter().map(|x| x.0).collect::<Vec<_>>(),
            ["a", "c"]
        );
        assert_eq!(
            p.not_negotiated.iter().map(|x| x.0).collect::<Vec<_>>(),
            ["pending"]
        );
    }
}
