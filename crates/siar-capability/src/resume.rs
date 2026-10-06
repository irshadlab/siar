//! §97 session reconnect, §98 session resumption optimization.
//!
//! Default and only guaranteed behaviour: **every reconnect runs a new
//! authenticated negotiation**. The optional shortcut — skip re-sending
//! full capability sets — is allowed only when *all* of these hold:
//! resumption is enabled by policy ("correctness first"; off by default),
//! both peers explicitly support safe resume, the resumed session is
//! itself authenticated, and the peer's claimed capability hash equals
//! the hash of the set we cached for it. Anything else falls back to full
//! negotiation, never to an assumption.

use crate::hash::CapabilitySetHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResumePolicy {
    /// Off until the project explicitly turns the optimisation on.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeInputs<'a> {
    pub local_supports_resume: bool,
    pub remote_supports_resume: bool,
    /// Is the *resumed transport session* authenticated (not just a
    /// cached hash)?
    pub session_authenticated: bool,
    /// Hash of what we cached for this peer, if anything.
    pub cached: Option<&'a CapabilitySetHash>,
    /// Hash the peer claims its set currently has.
    pub claimed: Option<&'a CapabilitySetHash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    /// Exchange and negotiate full capability sets (always safe).
    FullNegotiation,
    /// Reuse the cached sets; nothing about capabilities is re-sent.
    ReuseCached,
}

/// Why a shortcut was refused — for diagnostics, not behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullReason {
    PolicyDisabled,
    PeerDoesNotSupport,
    SessionNotAuthenticated,
    NoCachedSet,
    NoClaimedHash,
    HashChanged,
}

pub fn decide(policy: ResumePolicy, i: &ResumeInputs<'_>) -> (ResumeDecision, Option<FullReason>) {
    let full = |why| (ResumeDecision::FullNegotiation, Some(why));
    if !policy.enabled {
        return full(FullReason::PolicyDisabled);
    }
    if !(i.local_supports_resume && i.remote_supports_resume) {
        return full(FullReason::PeerDoesNotSupport);
    }
    if !i.session_authenticated {
        return full(FullReason::SessionNotAuthenticated);
    }
    let Some(cached) = i.cached else {
        return full(FullReason::NoCachedSet);
    };
    let Some(claimed) = i.claimed else {
        return full(FullReason::NoClaimedHash);
    };
    // Compare via the digest type's equality (constant time in blake3).
    if cached != claimed {
        return full(FullReason::HashChanged);
    }
    (ResumeDecision::ReuseCached, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::{CapabilityId, CapabilityNamespace};
    use crate::set::CapabilitySet;
    use crate::version::CapabilityVersion;

    fn hash_of(code: u32) -> CapabilitySetHash {
        let mut s = CapabilitySet::new();
        s.insert(CapabilityDescriptor::new(
            CapabilityId::new(CapabilityNamespace::Files, code),
            CapabilityVersion::new(1, 0),
            CapabilityRequirement::Optional,
            CapabilityParameters::None,
        ))
        .unwrap();
        CapabilitySetHash::of(&s)
    }

    fn ok_inputs<'a>(a: &'a CapabilitySetHash, b: &'a CapabilitySetHash) -> ResumeInputs<'a> {
        ResumeInputs {
            local_supports_resume: true,
            remote_supports_resume: true,
            session_authenticated: true,
            cached: Some(a),
            claimed: Some(b),
        }
    }

    #[test]
    fn default_policy_always_negotiates_in_full() {
        let h = hash_of(1);
        let (d, why) = decide(ResumePolicy::default(), &ok_inputs(&h, &h));
        assert_eq!(
            (d, why),
            (
                ResumeDecision::FullNegotiation,
                Some(FullReason::PolicyDisabled)
            )
        );
    }

    #[test]
    fn the_shortcut_needs_every_condition() {
        let (h, other) = (hash_of(1), hash_of(2));
        let on = ResumePolicy { enabled: true };
        assert_eq!(
            decide(on, &ok_inputs(&h, &h)),
            (ResumeDecision::ReuseCached, None)
        );

        let base = ok_inputs(&h, &h);
        let cases = [
            (
                ResumeInputs {
                    local_supports_resume: false,
                    ..base
                },
                FullReason::PeerDoesNotSupport,
            ),
            (
                ResumeInputs {
                    remote_supports_resume: false,
                    ..base
                },
                FullReason::PeerDoesNotSupport,
            ),
            (
                ResumeInputs {
                    session_authenticated: false,
                    ..base
                },
                FullReason::SessionNotAuthenticated,
            ),
            (
                ResumeInputs {
                    cached: None,
                    ..base
                },
                FullReason::NoCachedSet,
            ),
            (
                ResumeInputs {
                    claimed: None,
                    ..base
                },
                FullReason::NoClaimedHash,
            ),
            (
                ResumeInputs {
                    claimed: Some(&other),
                    ..base
                },
                FullReason::HashChanged,
            ),
        ];
        for (inputs, why) in cases {
            assert_eq!(
                decide(on, &inputs),
                (ResumeDecision::FullNegotiation, Some(why)),
                "{why:?}"
            );
        }
    }

    #[test]
    fn an_unauthenticated_session_never_reuses_even_with_a_matching_hash() {
        let h = hash_of(1);
        let i = ResumeInputs {
            session_authenticated: false,
            ..ok_inputs(&h, &h)
        };
        assert_eq!(
            decide(ResumePolicy { enabled: true }, &i).0,
            ResumeDecision::FullNegotiation
        );
    }
}
