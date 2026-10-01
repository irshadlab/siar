//! §54 "Eviction".
//!
//! The spec's own seven-tier list, made a real, `Ord`-comparable type
//! rather than an ad hoc sort closure a caller would have to
//! re-derive correctly at every call site — "never use FIFO blindly"
//! is enforced by there being no field on [`EvictionTier`] for
//! insertion order at all.

use crate::state::BundleState;
use crate::types::DtnPriority;

/// §54's own seven tiers, in the spec's own order — declaration order
/// IS eviction order here (`derive(Ord)` on a fieldless enum ranks by
/// declaration position), so evicting "lowest tier first" is just
/// sorting ascending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvictionTier {
    Expired,
    Delivered,
    Cancelled,
    BulkLowPriority,
    OldNormal,
    High,
    CriticalLast,
}

/// Maps a stored bundle's own state and priority to its eviction tier.
/// State is checked first — an expired or delivered bundle is evicted
/// first regardless of how important its content was, since the
/// content either can never arrive (expired) or already has
/// (delivered); priority only matters for bundles still legitimately
/// in flight.
pub fn eviction_tier(state: BundleState, priority: DtnPriority) -> EvictionTier {
    if state == BundleState::Expired {
        return EvictionTier::Expired;
    }
    if state.is_delivered() {
        return EvictionTier::Delivered;
    }
    if state == BundleState::Cancelled {
        return EvictionTier::Cancelled;
    }
    match priority {
        DtnPriority::Low => EvictionTier::BulkLowPriority,
        DtnPriority::Normal => EvictionTier::OldNormal,
        DtnPriority::Important => EvictionTier::High,
        DtnPriority::Sos => EvictionTier::CriticalLast,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_tiers_rank_in_the_spec_s_own_order() {
        assert!(EvictionTier::Expired < EvictionTier::Delivered);
        assert!(EvictionTier::Delivered < EvictionTier::Cancelled);
        assert!(EvictionTier::Cancelled < EvictionTier::BulkLowPriority);
        assert!(EvictionTier::BulkLowPriority < EvictionTier::OldNormal);
        assert!(EvictionTier::OldNormal < EvictionTier::High);
        assert!(EvictionTier::High < EvictionTier::CriticalLast);
    }

    #[test]
    fn state_wins_over_priority_expired_sos_still_evicts_before_a_healthy_low_priority_bundle() {
        let expired_sos = eviction_tier(BundleState::Expired, DtnPriority::Sos);
        let healthy_low = eviction_tier(BundleState::Eligible, DtnPriority::Low);
        assert!(expired_sos < healthy_low);
    }

    #[test]
    fn a_forwarded_bundle_ranks_by_priority_not_specially() {
        assert_eq!(
            eviction_tier(BundleState::Forwarded, DtnPriority::Important),
            EvictionTier::High
        );
    }

    #[test]
    fn a_delivered_critical_bundle_still_evicts_before_a_stored_low_one() {
        // Delivered content has already reached its destination — the
        // copy sitting in this relay's own store is no longer needed
        // regardless of how urgent it was.
        let delivered_sos = eviction_tier(BundleState::Acknowledged, DtnPriority::Sos);
        let stored_low = eviction_tier(BundleState::Stored, DtnPriority::Low);
        assert!(delivered_sos < stored_low);
    }
}
