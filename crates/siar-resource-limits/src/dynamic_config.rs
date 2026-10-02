//! §143 dynamic config: admins/users change relay quota, parallel
//! transfers and data saver at runtime, and "runtime applies safely".
//!
//! Safe means: (1) the candidate is validated as a whole against the hard
//! ceilings and committed only if valid — never half-applied; (2)
//! tightening below current usage is accepted but reported as a *drain*
//! the caller must perform gradually, instead of killing live work;
//! (3) lowering a global limit automatically clamps the per-peer limit
//! that depends on it, since tightening can never be inconsistent.

use crate::policy_layers::HardLimits;
use crate::profile::{ConfigError, ResourceConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConfigUpdate {
    pub relay_storage_quota: Option<u64>,
    pub max_parallel_transfers: Option<u32>,
    pub data_saver: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CurrentUsage {
    pub relay_bytes: u64,
    pub active_transfers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Tightened,
    Loosened,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub field: &'static str,
    pub direction: Direction,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyReport {
    pub changes: Vec<Change>,
    /// Relay bytes above the new quota that must be evicted over time.
    pub relay_bytes_to_drain: u64,
    /// Transfers above the new effective limit that must finish/pause.
    pub transfers_to_drain: u32,
}

fn dir<T: PartialOrd>(old: T, new: T) -> Option<Direction> {
    if new < old {
        Some(Direction::Tightened)
    } else if new > old {
        Some(Direction::Loosened)
    } else {
        None
    }
}

pub fn apply_update(
    cfg: &mut ResourceConfig,
    hard: &HardLimits,
    update: &ConfigUpdate,
    usage: &CurrentUsage,
) -> Result<ApplyReport, Vec<ConfigError>> {
    let mut cand = *cfg;
    let mut report = ApplyReport::default();

    if let Some(q) = update.relay_storage_quota {
        if let Some(d) = dir(cand.storage.relay_quota_bytes, q) {
            report.changes.push(Change {
                field: "relay_storage_quota",
                direction: d,
            });
        }
        cand.storage.relay_quota_bytes = q;
    }
    if let Some(t) = update.max_parallel_transfers {
        if let Some(d) = dir(cand.network.max_transfers, t) {
            report.changes.push(Change {
                field: "max_parallel_transfers",
                direction: d,
            });
        }
        cand.network.max_transfers = t;
        // Tightening is always allowed to be consistent: clamp dependents.
        cand.network.max_transfers_per_peer = cand.network.max_transfers_per_peer.min(t.max(1));
    }
    if let Some(s) = update.data_saver {
        if s != cand.data_saver {
            // Turning data saver on restricts; off relaxes.
            report.changes.push(Change {
                field: "data_saver",
                direction: if s {
                    Direction::Tightened
                } else {
                    Direction::Loosened
                },
            });
        }
        cand.data_saver = s;
    }

    cand.validate(hard)?;

    report.relay_bytes_to_drain = usage
        .relay_bytes
        .saturating_sub(cand.storage.relay_quota_bytes);
    report.transfers_to_drain = usage
        .active_transfers
        .saturating_sub(cand.effective_max_transfers());
    *cfg = cand;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ResourceProfile;

    fn setup() -> (ResourceConfig, HardLimits) {
        let p = ResourceProfile::Desktop;
        (ResourceConfig::for_profile(p), HardLimits::for_profile(p))
    }

    #[test]
    fn valid_update_is_committed_and_classified() {
        let (mut c, h) = setup();
        let old = c.storage.relay_quota_bytes;
        let r = apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                relay_storage_quota: Some(old / 2),
                max_parallel_transfers: Some(16),
                data_saver: None,
            },
            &CurrentUsage::default(),
        )
        .unwrap();
        assert_eq!(c.storage.relay_quota_bytes, old / 2);
        assert_eq!(c.network.max_transfers, 16);
        assert!(r.changes.contains(&Change {
            field: "relay_storage_quota",
            direction: Direction::Tightened
        }));
        assert!(r.changes.contains(&Change {
            field: "max_parallel_transfers",
            direction: Direction::Loosened
        }));
    }

    #[test]
    fn invalid_update_changes_nothing() {
        let (mut c, h) = setup();
        let before = c;
        let too_big = h.max_dtn_storage_bytes + 1;
        let err = apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                relay_storage_quota: Some(too_big),
                max_parallel_transfers: Some(4),
                data_saver: Some(true),
            },
            &CurrentUsage::default(),
        )
        .unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(
            c, before,
            "all-or-nothing, including the otherwise-valid fields"
        );
    }

    #[test]
    fn tightening_below_usage_reports_a_drain_instead_of_failing() {
        let (mut c, h) = setup();
        let r = apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                relay_storage_quota: Some(1_000),
                max_parallel_transfers: Some(2),
                data_saver: None,
            },
            &CurrentUsage {
                relay_bytes: 5_000,
                active_transfers: 6,
            },
        )
        .unwrap();
        assert_eq!(r.relay_bytes_to_drain, 4_000);
        assert_eq!(r.transfers_to_drain, 4);
    }

    #[test]
    fn lowering_global_transfers_clamps_per_peer_instead_of_rejecting() {
        let (mut c, h) = setup(); // per-peer is 3
        apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                max_parallel_transfers: Some(1),
                ..Default::default()
            },
            &CurrentUsage::default(),
        )
        .unwrap();
        assert_eq!(
            (c.network.max_transfers, c.network.max_transfers_per_peer),
            (1, 1)
        );
    }

    #[test]
    fn data_saver_shrinks_effective_transfers_and_reports_drain() {
        let (mut c, h) = setup();
        let r = apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                data_saver: Some(true),
                ..Default::default()
            },
            &CurrentUsage {
                relay_bytes: 0,
                active_transfers: 8,
            },
        )
        .unwrap();
        assert_eq!(r.transfers_to_drain, 6);
        assert!(r.changes.contains(&Change {
            field: "data_saver",
            direction: Direction::Tightened
        }));
    }

    #[test]
    fn zero_transfers_is_rejected() {
        let (mut c, h) = setup();
        let err = apply_update(
            &mut c,
            &h,
            &ConfigUpdate {
                max_parallel_transfers: Some(0),
                ..Default::default()
            },
            &CurrentUsage::default(),
        )
        .unwrap_err();
        assert!(err.contains(&ConfigError::Zero("network.max_transfers")));
    }
}
