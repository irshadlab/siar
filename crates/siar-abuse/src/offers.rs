//! §64: unknown file offers. **Never auto-download a large attachment from
//! someone we do not know**, and never auto-download *anything* from an
//! unknown peer without the user asking.

use crate::gate::Standing;

#[derive(Debug, Clone, Copy)]
pub struct OfferPolicy {
    /// Above this, an unknown peer's offer is refused outright (the user is
    /// not even asked: it costs them attention for nothing).
    pub unknown_max: u64,
    /// Largest file a *contact* may send without asking.
    pub contact_auto_max: u64,
    pub verified_auto_max: u64,
    /// Nothing above this, from anyone, is auto-accepted or offered.
    pub hard_max: u64,
}

impl Default for OfferPolicy {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Self {
            unknown_max: 10 * MIB,
            contact_auto_max: 5 * MIB,
            verified_auto_max: 25 * MIB,
            hard_max: 2048 * MIB,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferReject {
    Blocked,
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferDecision {
    AutoAccept,
    /// Show the user an offer; download only if they agree.
    AskUser,
    Reject(OfferReject),
}

impl OfferPolicy {
    /// `metered`: on a costly network even trusted peers' files ask first.
    #[must_use]
    pub const fn decide(&self, standing: Standing, size: u64, metered: bool) -> OfferDecision {
        if matches!(standing, Standing::Blocked) {
            return OfferDecision::Reject(OfferReject::Blocked);
        }
        if size > self.hard_max {
            return OfferDecision::Reject(OfferReject::TooLarge);
        }
        match standing {
            Standing::Blocked => OfferDecision::Reject(OfferReject::Blocked),
            Standing::Unknown => {
                if size > self.unknown_max {
                    OfferDecision::Reject(OfferReject::TooLarge)
                } else {
                    OfferDecision::AskUser
                }
            }
            Standing::Contact => {
                if size <= self.contact_auto_max && !metered {
                    OfferDecision::AutoAccept
                } else {
                    OfferDecision::AskUser
                }
            }
            Standing::Verified => {
                if size <= self.verified_auto_max && !metered {
                    OfferDecision::AutoAccept
                } else {
                    OfferDecision::AskUser
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIB: u64 = 1024 * 1024;

    #[test]
    fn unknown_peers_never_auto_download_anything() {
        let p = OfferPolicy::default();
        for size in [0, 1, 1024, MIB, 9 * MIB] {
            assert_eq!(
                p.decide(Standing::Unknown, size, false),
                OfferDecision::AskUser,
                "{size}"
            );
        }
    }

    #[test]
    fn large_offers_from_strangers_are_refused_without_bothering_the_user() {
        let p = OfferPolicy::default();
        assert_eq!(
            p.decide(Standing::Unknown, 11 * MIB, false),
            OfferDecision::Reject(OfferReject::TooLarge)
        );
        assert_eq!(
            p.decide(Standing::Unknown, u64::MAX, false),
            OfferDecision::Reject(OfferReject::TooLarge)
        );
    }

    #[test]
    fn trusted_peers_auto_accept_small_files_but_ask_for_large_ones_or_on_metered() {
        let p = OfferPolicy::default();
        assert_eq!(
            p.decide(Standing::Contact, MIB, false),
            OfferDecision::AutoAccept
        );
        assert_eq!(
            p.decide(Standing::Contact, 6 * MIB, false),
            OfferDecision::AskUser
        );
        assert_eq!(
            p.decide(Standing::Verified, 20 * MIB, false),
            OfferDecision::AutoAccept
        );
        assert_eq!(
            p.decide(Standing::Verified, 20 * MIB, true),
            OfferDecision::AskUser
        );
    }

    #[test]
    fn nothing_beyond_the_hard_cap_and_blocked_peers_never_get_through() {
        let p = OfferPolicy::default();
        assert_eq!(
            p.decide(Standing::Verified, 3000 * MIB, false),
            OfferDecision::Reject(OfferReject::TooLarge)
        );
        assert_eq!(
            p.decide(Standing::Blocked, 1, false),
            OfferDecision::Reject(OfferReject::Blocked)
        );
    }

    #[test]
    fn standing_never_makes_a_decision_more_permissive_than_a_less_trusted_one() {
        let p = OfferPolicy::default();
        let rank = |d: OfferDecision| match d {
            OfferDecision::Reject(_) => 0,
            OfferDecision::AskUser => 1,
            OfferDecision::AutoAccept => 2,
        };
        for size in [0, MIB, 4 * MIB, 8 * MIB, 20 * MIB, 100 * MIB] {
            for m in [false, true] {
                let r: Vec<_> = [
                    Standing::Blocked,
                    Standing::Unknown,
                    Standing::Contact,
                    Standing::Verified,
                ]
                .iter()
                .map(|s| rank(p.decide(*s, size, m)))
                .collect();
                assert!(
                    r.windows(2).all(|w| w[0] <= w[1]) || size > 10 * MIB,
                    "{r:?} at {size}"
                );
            }
        }
    }
}
