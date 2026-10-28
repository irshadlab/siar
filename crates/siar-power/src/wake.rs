//! §30-§32: radio wake-up cost — batch small work into one wake window.
//!
//! Waking the radio costs far more than sending a receipt. Receipts,
//! presence and small sync items are therefore collected and flushed together;
//! anything urgent opens a window immediately and carries the batch with it.

use serde::{Deserialize, Serialize};

use crate::profile::PowerPolicyProfile;

/// §31.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBudget {
    pub max_items: u32,
    pub max_bytes: u32,
}

/// §31.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeWindow {
    pub started_at: u64,
    pub max_duration_ms: u64,
    pub work_budget: WorkBudget,
}

impl WakeWindow {
    #[must_use]
    pub const fn expired(&self, now: u64) -> bool {
        now.saturating_sub(self.started_at) >= self.max_duration_ms
    }
}

/// How long small work may wait to be batched, per profile.
#[must_use]
pub const fn coalesce_ms(profile: PowerPolicyProfile) -> u64 {
    match profile {
        PowerPolicyProfile::Performance | PowerPolicyProfile::AlwaysOnNode => 250,
        PowerPolicyProfile::Balanced => 2_000,
        PowerPolicyProfile::Emergency => 1_000,
        PowerPolicyProfile::Saver => 60_000,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WakeBatcher {
    profile: PowerPolicyProfile,
    pending: u32,
    pending_bytes: u64,
    oldest_at: Option<u64>,
    /// Flush early once this many items are waiting.
    pub max_pending: u32,
}

impl WakeBatcher {
    #[must_use]
    pub const fn new(profile: PowerPolicyProfile) -> Self {
        Self {
            profile,
            pending: 0,
            pending_bytes: 0,
            oldest_at: None,
            max_pending: 64,
        }
    }

    pub fn set_profile(&mut self, profile: PowerPolicyProfile) {
        self.profile = profile;
    }

    /// Record one small item. Returns `true` when a wake window should open
    /// **now** (urgent item, batch full, or the oldest item waited long enough).
    pub fn note(&mut self, now: u64, bytes: u32, urgent: bool) -> bool {
        self.pending = self.pending.saturating_add(1);
        self.pending_bytes = self.pending_bytes.saturating_add(u64::from(bytes));
        self.oldest_at.get_or_insert(now);
        urgent || self.pending >= self.max_pending || self.due(now)
    }

    /// Has the oldest item waited long enough?
    #[must_use]
    pub fn due(&self, now: u64) -> bool {
        self.oldest_at
            .is_some_and(|t| now.saturating_sub(t) >= coalesce_ms(self.profile))
    }

    #[must_use]
    pub const fn pending(&self) -> u32 {
        self.pending
    }

    /// Open a window and clear the batch. The window is sized for what is
    /// pending, bounded so a window can never be used for bulk.
    pub fn open(&mut self, now: u64) -> WakeWindow {
        let w = WakeWindow {
            started_at: now,
            max_duration_ms: match self.profile {
                PowerPolicyProfile::Saver => 5_000,
                _ => 10_000,
            },
            work_budget: WorkBudget {
                max_items: self.pending.max(1).min(self.max_pending.saturating_mul(2)),
                max_bytes: u32::try_from(self.pending_bytes.min(256 * 1024)).unwrap_or(256 * 1024),
            },
        };
        self.pending = 0;
        self.pending_bytes = 0;
        self.oldest_at = None;
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_receipts_one_window() {
        let mut b = WakeBatcher::new(PowerPolicyProfile::Balanced);
        let mut opens = 0;
        for i in 0..20 {
            if b.note(i * 50, 100, false) {
                opens += 1;
                b.open(i * 50);
            }
        }
        // 20 items over 1 s with a 2 s coalescing window: far fewer wakes than items.
        assert!(opens <= 1, "{opens} wake windows for 20 receipts");
    }

    #[test]
    fn urgent_item_opens_immediately_and_takes_the_batch() {
        let mut b = WakeBatcher::new(PowerPolicyProfile::Saver);
        assert!(!b.note(0, 10, false));
        assert!(!b.note(1, 10, false));
        assert!(b.note(2, 10, true));
        let w = b.open(2);
        assert_eq!(w.work_budget.max_items, 3);
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn saver_waits_longer_than_performance() {
        assert!(
            coalesce_ms(PowerPolicyProfile::Saver)
                > 10 * coalesce_ms(PowerPolicyProfile::Performance)
        );
    }

    #[test]
    fn full_batch_flushes_early() {
        let mut b = WakeBatcher::new(PowerPolicyProfile::Saver);
        b.max_pending = 4;
        assert!(!b.note(0, 1, false) && !b.note(0, 1, false) && !b.note(0, 1, false));
        assert!(b.note(0, 1, false));
    }

    #[test]
    fn window_expires() {
        let w = WakeBatcher::new(PowerPolicyProfile::Balanced).open(1_000);
        assert!(!w.expired(5_000));
        assert!(w.expired(11_000));
    }
}
