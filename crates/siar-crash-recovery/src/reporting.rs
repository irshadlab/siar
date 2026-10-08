//! §115 "Recovery Diagnostics", §116 "User-Facing Recovery UX", §117
//! "Developer Diagnostics", §118 "Recovery Metrics", §119 "Privacy".
//!
//! §115/§119's "do not include secret material" is enforced by
//! *shape*: [`RecoveryAction`] and [`RecoveryWarning`] carry only
//! enum codes and counts — there is no free-text field a secret,
//! plaintext, file name or contact could be put into.

use crate::recovery_mode::RecoverySeverity;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §115
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryActionKind {
    ProjectionRebuilt,
    TransferResumed,
    TransferPaused,
    OrphanTempDeleted,
    PartialChunkReverified,
    DtnBundlesRestored,
    ReservationReleased,
    BlobQuarantined,
    OutboxReconstructed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAction {
    pub kind: RecoveryActionKind,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WarningCode {
    UncleanShutdown,
    ClockWentBackward,
    SearchIndexUnavailable,
    UnknownOutboxEntry,
    IdentityForkDetected,
    DowngradeReadOnly,
    QuarantineFull,
    SubsystemRecoveryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryWarning {
    pub code: WarningCode,
    pub count: u64,
}

/// §115, verbatim fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub severity: RecoverySeverity,
    pub repaired_items: Vec<RecoveryAction>,
    pub warnings: Vec<RecoveryWarning>,
}

impl RecoveryReport {
    pub fn clean() -> Self {
        Self {
            severity: RecoverySeverity::Clean,
            repaired_items: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn count_of(&self, k: RecoveryActionKind) -> u64 {
        self.repaired_items
            .iter()
            .filter(|a| a.kind == k)
            .map(|a| a.count)
            .fold(0u64, u64::saturating_add)
    }
}

// ---------------------------------------------------------------------
// §116
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UxMessage {
    RecoveringMessages,
    TransfersPausedWillResume,
    ReadOnlyRecoveryMode,
    RecoveryFailed,
}

impl UxMessage {
    /// Stable key for localisation.
    pub const fn key(self) -> &'static str {
        match self {
            UxMessage::RecoveringMessages => "recovery.recovering",
            UxMessage::TransfersPausedWillResume => "recovery.transfers_paused",
            UxMessage::ReadOnlyRecoveryMode => "recovery.read_only",
            UxMessage::RecoveryFailed => "recovery.failed",
        }
    }

    /// Default English text (the §116 wording).
    pub const fn default_text(self) -> &'static str {
        match self {
            UxMessage::RecoveringMessages => "Recovering your messages…",
            UxMessage::TransfersPausedWillResume => {
                "Some file transfers were paused and will resume."
            }
            UxMessage::ReadOnlyRecoveryMode => "App is in read-only recovery mode.",
            UxMessage::RecoveryFailed => "Recovery could not complete.",
        }
    }
}

pub fn ux_message(r: &RecoveryReport) -> UxMessage {
    match r.severity {
        RecoverySeverity::Fatal => UxMessage::RecoveryFailed,
        RecoverySeverity::ReadOnly => UxMessage::ReadOnlyRecoveryMode,
        RecoverySeverity::Degraded | RecoverySeverity::Repaired
            if r.count_of(RecoveryActionKind::TransferPaused) > 0 =>
        {
            UxMessage::TransfersPausedWillResume
        }
        _ => UxMessage::RecoveringMessages,
    }
}

/// "Avoid long blocking if not necessary": only block the UI when the
/// estimate exceeds the threshold *and* the app can't be usable
/// without it.
pub const fn should_block_ui(
    estimated_millis: u64,
    threshold_millis: u64,
    critical_tiers_pending: bool,
) -> bool {
    critical_tiers_pending && estimated_millis > threshold_millis
}

// ---------------------------------------------------------------------
// §117
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeveloperDiagnostics {
    pub unclean_shutdown: bool,
    pub event_log_healthy: bool,
    pub projection_rebuild_events: u64,
    pub transfers_resumed: u64,
    pub orphan_temp_cleaned: u64,
    pub dtn_bundles_restored: u64,
    pub identity_verified_generation: Option<u64>,
}

impl DeveloperDiagnostics {
    /// The §117 panel, line for line.
    pub fn render(&self) -> String {
        let yn = |b| if b { "yes" } else { "no" };
        let identity = match self.identity_verified_generation {
            Some(g) => format!("verified generation {g}"),
            None => "NOT verified".to_owned(),
        };
        format!(
            "Unclean shutdown: {}\nEvent log: {}\nProjection rebuild: {} events\nTransfers resumed: {}\nOrphan temp cleaned: {}\nDTN bundles restored: {}\nIdentity: {}",
            yn(self.unclean_shutdown),
            if self.event_log_healthy { "healthy" } else { "UNHEALTHY" },
            self.projection_rebuild_events,
            self.transfers_resumed,
            self.orphan_temp_cleaned,
            self.dtn_bundles_restored,
            identity,
        )
    }
}

// ---------------------------------------------------------------------
// §118
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricEvent {
    RecoveryFinished {
        duration_millis: u64,
        unclean: bool,
        read_only: bool,
    },
    ProjectionRepaired,
    TransferResumed,
    OrphanBytesReclaimed(u64),
    CorruptionDetected,
}

/// §118's seven counters. All saturating: a metric must never panic
/// or wrap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryMetrics {
    pub last_recovery_duration_millis: u64,
    pub total_recovery_duration_millis: u64,
    pub unclean_startups: u64,
    pub repaired_projections: u64,
    pub resumed_transfers: u64,
    pub orphan_bytes_reclaimed: u64,
    pub corruption_count: u64,
    pub read_only_starts: u64,
}

impl RecoveryMetrics {
    pub fn record(&mut self, e: MetricEvent) {
        match e {
            MetricEvent::RecoveryFinished {
                duration_millis,
                unclean,
                read_only,
            } => {
                self.last_recovery_duration_millis = duration_millis;
                self.total_recovery_duration_millis = self
                    .total_recovery_duration_millis
                    .saturating_add(duration_millis);
                self.unclean_startups = self.unclean_startups.saturating_add(unclean as u64);
                self.read_only_starts = self.read_only_starts.saturating_add(read_only as u64);
            }
            MetricEvent::ProjectionRepaired => {
                self.repaired_projections = self.repaired_projections.saturating_add(1)
            }
            MetricEvent::TransferResumed => {
                self.resumed_transfers = self.resumed_transfers.saturating_add(1)
            }
            MetricEvent::OrphanBytesReclaimed(b) => {
                self.orphan_bytes_reclaimed = self.orphan_bytes_reclaimed.saturating_add(b)
            }
            MetricEvent::CorruptionDetected => {
                self.corruption_count = self.corruption_count.saturating_add(1)
            }
        }
    }
}

// ---------------------------------------------------------------------
// §119
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogField {
    MessagePlaintext,
    FileName,
    PrivateKey,
    FullContactGraph,
    Counts,
    OpaqueIds,
    SeverityAndCodes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRule {
    Allowed,
    /// "File names unless needed."
    OnlyIfNeeded,
    Forbidden,
}

pub const fn log_rule(f: LogField) -> LogRule {
    match f {
        LogField::MessagePlaintext | LogField::PrivateKey | LogField::FullContactGraph => {
            LogRule::Forbidden
        }
        LogField::FileName => LogRule::OnlyIfNeeded,
        LogField::Counts | LogField::OpaqueIds | LogField::SeverityAndCodes => LogRule::Allowed,
    }
}

pub const fn may_log(f: LogField, needed: bool) -> bool {
    match log_rule(f) {
        LogRule::Allowed => true,
        LogRule::OnlyIfNeeded => needed,
        LogRule::Forbidden => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RecoveryActionKind::*;

    fn report(sev: RecoverySeverity, paused: u64) -> RecoveryReport {
        let mut r = RecoveryReport::clean();
        r.severity = sev;
        if paused > 0 {
            r.repaired_items.push(RecoveryAction {
                kind: TransferPaused,
                count: paused,
            });
        }
        r
    }

    #[test]
    fn report_has_the_spec_fields_and_counts_by_kind() {
        let mut r = RecoveryReport::clean();
        r.repaired_items.push(RecoveryAction {
            kind: OrphanTempDeleted,
            count: 7,
        });
        r.repaired_items.push(RecoveryAction {
            kind: OrphanTempDeleted,
            count: 3,
        });
        r.warnings.push(RecoveryWarning {
            code: WarningCode::UncleanShutdown,
            count: 1,
        });
        assert_eq!(r.count_of(OrphanTempDeleted), 10);
        assert_eq!(r.count_of(TransferResumed), 0);
        assert_eq!(r.severity, RecoverySeverity::Clean);
    }

    #[test]
    fn ux_messages_follow_severity_with_the_spec_wording() {
        assert_eq!(
            ux_message(&report(RecoverySeverity::Clean, 0)),
            UxMessage::RecoveringMessages
        );
        assert_eq!(
            ux_message(&report(RecoverySeverity::Degraded, 2)),
            UxMessage::TransfersPausedWillResume
        );
        assert_eq!(
            ux_message(&report(RecoverySeverity::ReadOnly, 2)),
            UxMessage::ReadOnlyRecoveryMode
        );
        assert_eq!(
            ux_message(&report(RecoverySeverity::Fatal, 0)),
            UxMessage::RecoveryFailed
        );
        assert_eq!(
            UxMessage::RecoveringMessages.default_text(),
            "Recovering your messages…"
        );
        assert_eq!(
            UxMessage::TransfersPausedWillResume.default_text(),
            "Some file transfers were paused and will resume."
        );
        assert_eq!(
            UxMessage::ReadOnlyRecoveryMode.default_text(),
            "App is in read-only recovery mode."
        );
    }

    #[test]
    fn ui_blocks_only_when_critical_work_is_slow() {
        assert!(!should_block_ui(10_000, 500, false));
        assert!(!should_block_ui(100, 500, true));
        assert!(should_block_ui(10_000, 500, true));
    }

    #[test]
    fn developer_panel_matches_the_spec_example_exactly() {
        let d = DeveloperDiagnostics {
            unclean_shutdown: true,
            event_log_healthy: true,
            projection_rebuild_events: 1200,
            transfers_resumed: 3,
            orphan_temp_cleaned: 7,
            dtn_bundles_restored: 12,
            identity_verified_generation: Some(41),
        };
        assert_eq!(
            d.render(),
            "Unclean shutdown: yes\nEvent log: healthy\nProjection rebuild: 1200 events\nTransfers resumed: 3\nOrphan temp cleaned: 7\nDTN bundles restored: 12\nIdentity: verified generation 41"
        );
    }

    #[test]
    fn developer_panel_flags_problems_loudly() {
        let d = DeveloperDiagnostics {
            unclean_shutdown: false,
            event_log_healthy: false,
            projection_rebuild_events: 0,
            transfers_resumed: 0,
            orphan_temp_cleaned: 0,
            dtn_bundles_restored: 0,
            identity_verified_generation: None,
        };
        let s = d.render();
        assert!(s.contains("Event log: UNHEALTHY"));
        assert!(s.contains("Identity: NOT verified"));
        assert!(s.contains("Unclean shutdown: no"));
    }

    #[test]
    fn metrics_accumulate_all_seven_counters() {
        let mut m = RecoveryMetrics::default();
        m.record(MetricEvent::RecoveryFinished {
            duration_millis: 300,
            unclean: true,
            read_only: false,
        });
        m.record(MetricEvent::RecoveryFinished {
            duration_millis: 100,
            unclean: false,
            read_only: true,
        });
        m.record(MetricEvent::ProjectionRepaired);
        m.record(MetricEvent::TransferResumed);
        m.record(MetricEvent::TransferResumed);
        m.record(MetricEvent::OrphanBytesReclaimed(4_096));
        m.record(MetricEvent::CorruptionDetected);
        assert_eq!(m.last_recovery_duration_millis, 100);
        assert_eq!(m.total_recovery_duration_millis, 400);
        assert_eq!(m.unclean_startups, 1);
        assert_eq!(m.read_only_starts, 1);
        assert_eq!(m.repaired_projections, 1);
        assert_eq!(m.resumed_transfers, 2);
        assert_eq!(m.orphan_bytes_reclaimed, 4_096);
        assert_eq!(m.corruption_count, 1);
    }

    #[test]
    fn metrics_saturate_instead_of_wrapping() {
        let mut m = RecoveryMetrics {
            orphan_bytes_reclaimed: u64::MAX - 1,
            ..Default::default()
        };
        m.record(MetricEvent::OrphanBytesReclaimed(10));
        assert_eq!(m.orphan_bytes_reclaimed, u64::MAX);
    }

    #[test]
    fn privacy_rules_match_the_spec_list() {
        assert!(!may_log(LogField::MessagePlaintext, true));
        assert!(!may_log(LogField::PrivateKey, true));
        assert!(!may_log(LogField::FullContactGraph, true));
        assert!(!may_log(LogField::FileName, false));
        assert!(may_log(LogField::FileName, true));
        assert!(may_log(LogField::Counts, false));
        assert!(may_log(LogField::SeverityAndCodes, false));
    }
}
