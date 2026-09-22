//! An identifier for [`crate::report::EmergencyReport`] — a real gap
//! found while wiring `04-offline-event-log-architecture.md` §37
//! "Emergency Events" (`events.rs`): `EmergencyReport` itself has no
//! identifier of its own, only content fields (`kind`/`sender`/
//! `location`/`created_at`/`people`/`note`). Without one, nothing can
//! name "this specific report" across its own lifecycle (created →
//! trust reclassified → acknowledged → resolved) the way every other
//! domain in this workspace already can for its own aggregate
//! (`siar_blob_manifest::TransferId`, `siar_dtn_bundle::BundleId`, a
//! message's own `siar_domain::MessageId`). Same UUID-newtype shape as
//! those, for the same reason: a report has no content-derived hash to
//! use instead, and two reports with byte-identical fields (say, two
//! different people both signaling `Sos` with no location) are still
//! two different real-world events, not one.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ReportId(Uuid);

impl ReportId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ReportId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ReportId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_freshly_generated_ids_are_different() {
        assert_ne!(ReportId::new(), ReportId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = ReportId::new();
        let parsed: Uuid = id.to_string().parse().unwrap();
        assert_eq!(parsed.to_string(), id.to_string());
    }
}
