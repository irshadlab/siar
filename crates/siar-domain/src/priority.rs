//! `MessagePriority` — next.md §10, §30, §38.
//!
//! Originally added under `siar-dtn` (Phase 4), moved here once
//! `siar-protocol`'s `MeshEnvelope` (the next.md §29 wire-protocol
//! piece Phase 7 found missing) also needed it: `siar-protocol` sits
//! below `siar-dtn` in next.md §4's layer diagram, so `siar-protocol`
//! depending on `siar-dtn` for this would be a backwards dependency.
//! With the retirement of `siar-dtn` and `siar-routing` (see `MIGRATION.md`),
//! all consumers across the workspace import `MessagePriority` directly from
//! `siar_domain`.
//!
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessagePriority {
    Emergency,
    Critical,
    Interactive,
    Normal,
    Background,
}

impl MessagePriority {
    /// next.md §38's replication-budget examples ("ordinary DM = 2
    /// copies... SOS = 8 copies"), as this priority's starting budget
    /// for a freshly-created bundle. Callers may always pick a
    /// different starting value explicitly — this is the doc's own
    /// suggested default, not a hard rule this type enforces.
    pub fn default_replication_budget(self) -> u8 {
        match self {
            MessagePriority::Emergency => 8,
            MessagePriority::Critical => 4,
            MessagePriority::Interactive | MessagePriority::Normal => 2,
            MessagePriority::Background => 1,
        }
    }

    /// next.md §30's hop_limit examples ("normal messages: hop_limit =
    /// 4... emergency messages: hop_limit = 8").
    pub fn default_hop_limit(self) -> u8 {
        match self {
            MessagePriority::Emergency | MessagePriority::Critical => 8,
            MessagePriority::Interactive
            | MessagePriority::Normal
            | MessagePriority::Background => 4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_gets_the_highest_hop_limit_and_replication_budget() {
        assert_eq!(MessagePriority::Emergency.default_hop_limit(), 8);
        assert_eq!(MessagePriority::Emergency.default_replication_budget(), 8);
        assert!(
            MessagePriority::Emergency.default_hop_limit()
                >= MessagePriority::Normal.default_hop_limit()
        );
        assert!(
            MessagePriority::Emergency.default_replication_budget()
                >= MessagePriority::Normal.default_replication_budget()
        );
    }
}
