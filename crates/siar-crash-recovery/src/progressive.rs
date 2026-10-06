//! §45 "Progressive Recovery" and §46 "Recovery Priority".
//!
//! The app becomes usable once the *critical* tiers (§46's first
//! list) are recovered — not after the years-long rebuild of
//! secondary indexes. Order is enforced: a secondary tier cannot be
//! reported done while a critical one is still pending, so a caller
//! can't accidentally start (or believe it has finished) background
//! work first. Tiers are bits in a `u16`: no allocation, `Copy`.

use serde::{Deserialize, Serialize};

/// §46's two lists, in priority order (discriminant = rank).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum RecoveryTier {
    // Critical
    Identity = 0,
    SecurityState = 1,
    EventStoreIntegrity = 2,
    PendingMessages = 3,
    SosCriticalDtn = 4,
    ActiveTransfers = 5,
    // Secondary
    Search = 6,
    Analytics = 7,
    OldThumbnails = 8,
}

impl RecoveryTier {
    pub const ALL: [RecoveryTier; 9] = [
        RecoveryTier::Identity,
        RecoveryTier::SecurityState,
        RecoveryTier::EventStoreIntegrity,
        RecoveryTier::PendingMessages,
        RecoveryTier::SosCriticalDtn,
        RecoveryTier::ActiveTransfers,
        RecoveryTier::Search,
        RecoveryTier::Analytics,
        RecoveryTier::OldThumbnails,
    ];

    pub const fn is_critical(self) -> bool {
        (self as u8) <= RecoveryTier::ActiveTransfers as u8
    }

    const fn bit(self) -> u16 {
        1 << (self as u8)
    }

    const CRITICAL_MASK: u16 = (1 << 6) - 1;
    const ALL_MASK: u16 = (1 << 9) - 1;
}

/// §45's three stages plus completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPhase {
    /// Recovering critical state; UI must not present trusted data.
    Critical,
    /// Critical done — render recent/local data now.
    RenderLocal,
    /// Local data rendered; secondary indexes rebuilding.
    Background,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProgressError {
    #[error("{tier:?} cannot complete before critical tier {blocking:?}")]
    CriticalPending {
        tier: RecoveryTier,
        blocking: RecoveryTier,
    },
    #[error("local render reported before critical recovery finished")]
    RenderBeforeCritical,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProgressiveRecovery {
    done: u16,
    rendered: bool,
}

impl ProgressiveRecovery {
    pub const fn new() -> Self {
        Self {
            done: 0,
            rendered: false,
        }
    }

    const fn critical_done(&self) -> bool {
        self.done & RecoveryTier::CRITICAL_MASK == RecoveryTier::CRITICAL_MASK
    }

    /// Lowest-ranked critical tier not yet done.
    fn first_pending_critical(&self) -> Option<RecoveryTier> {
        RecoveryTier::ALL
            .iter()
            .copied()
            .filter(|t| t.is_critical())
            .find(|t| self.done & t.bit() == 0)
    }

    /// Marks a tier recovered. Secondary tiers are refused while any
    /// critical tier is pending; critical tiers may complete in any
    /// order (they can run in parallel) — only the critical/secondary
    /// boundary is enforced.
    pub fn mark_done(&mut self, tier: RecoveryTier) -> Result<(), ProgressError> {
        if !tier.is_critical() {
            if let Some(blocking) = self.first_pending_critical() {
                return Err(ProgressError::CriticalPending { tier, blocking });
            }
        }
        self.done |= tier.bit();
        Ok(())
    }

    /// §45: usable once critical state is recovered.
    pub const fn is_usable(&self) -> bool {
        self.critical_done()
    }

    pub fn mark_local_rendered(&mut self) -> Result<(), ProgressError> {
        if !self.critical_done() {
            return Err(ProgressError::RenderBeforeCritical);
        }
        self.rendered = true;
        Ok(())
    }

    pub const fn phase(&self) -> RecoveryPhase {
        if !self.critical_done() {
            RecoveryPhase::Critical
        } else if !self.rendered {
            RecoveryPhase::RenderLocal
        } else if self.done & RecoveryTier::ALL_MASK != RecoveryTier::ALL_MASK {
            RecoveryPhase::Background
        } else {
            RecoveryPhase::Complete
        }
    }

    /// The next tier to work on, in strict §46 rank order.
    pub fn next_tier(&self) -> Option<RecoveryTier> {
        RecoveryTier::ALL
            .iter()
            .copied()
            .find(|t| self.done & t.bit() == 0)
    }

    pub const fn is_done(&self, t: RecoveryTier) -> bool {
        self.done & t.bit() != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecoveryTier::*;

    #[test]
    fn ranks_match_spec_order_and_critical_split() {
        assert!(Identity < SecurityState && SecurityState < EventStoreIntegrity);
        assert!(ActiveTransfers.is_critical());
        assert!(!Search.is_critical() && !Analytics.is_critical() && !OldThumbnails.is_critical());
        assert!(SosCriticalDtn < ActiveTransfers);
    }

    #[test]
    fn app_is_usable_after_critical_without_waiting_for_secondary() {
        let mut p = ProgressiveRecovery::new();
        assert!(!p.is_usable());
        assert_eq!(p.phase(), RecoveryPhase::Critical);
        for t in RecoveryTier::ALL.iter().filter(|t| t.is_critical()) {
            p.mark_done(*t).unwrap();
        }
        assert!(p.is_usable());
        assert_eq!(p.phase(), RecoveryPhase::RenderLocal);
        assert!(!p.is_done(Search));
    }

    #[test]
    fn phases_advance_critical_render_background_complete() {
        let mut p = ProgressiveRecovery::new();
        assert_eq!(
            p.mark_local_rendered(),
            Err(ProgressError::RenderBeforeCritical)
        );
        for t in RecoveryTier::ALL.iter().filter(|t| t.is_critical()) {
            p.mark_done(*t).unwrap();
        }
        p.mark_local_rendered().unwrap();
        assert_eq!(p.phase(), RecoveryPhase::Background);
        for t in [Search, Analytics, OldThumbnails] {
            p.mark_done(t).unwrap();
        }
        assert_eq!(p.phase(), RecoveryPhase::Complete);
        assert_eq!(p.next_tier(), None);
    }

    #[test]
    fn secondary_work_is_refused_while_a_critical_tier_is_pending() {
        let mut p = ProgressiveRecovery::new();
        p.mark_done(Identity).unwrap();
        assert_eq!(
            p.mark_done(Search),
            Err(ProgressError::CriticalPending {
                tier: Search,
                blocking: SecurityState
            })
        );
        assert!(!p.is_done(Search));
    }

    #[test]
    fn critical_tiers_may_finish_in_any_order() {
        let mut p = ProgressiveRecovery::new();
        for t in [ActiveTransfers, SosCriticalDtn, PendingMessages] {
            p.mark_done(t).unwrap();
        }
        assert!(!p.is_usable());
        for t in [EventStoreIntegrity, SecurityState, Identity] {
            p.mark_done(t).unwrap();
        }
        assert!(p.is_usable());
    }

    #[test]
    fn next_tier_follows_strict_priority_and_marking_is_idempotent() {
        let mut p = ProgressiveRecovery::new();
        assert_eq!(p.next_tier(), Some(Identity));
        p.mark_done(Identity).unwrap();
        p.mark_done(Identity).unwrap();
        assert_eq!(p.next_tier(), Some(SecurityState));
    }
}
