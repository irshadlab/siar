//! §140 "Periodic Scrubbing", §141 "Bit Rot", §142 "Database Backup
//! Strategy", §143 "Backup Recovery Test".
//!
//! Background integrity work that is *not* part of every startup:
//! sampled blob scrubbing, repair-source selection when a hash no
//! longer matches, a safe-backup-method gate (never a blind live copy
//! of a WAL database), and the rule that an untested backup is not a
//! recovery plan.

use crate::corruption::{plan_blob_corruption, BlobCorruptionPlan};
use crate::recovery_scheduler::jitter_millis;
use crate::timing_recovery::WallMillis;

// ---------------------------------------------------------------------
// §140
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrubPolicy {
    /// Fraction of the store to verify per run, in permille.
    pub sample_permille: u32,
    /// Hard cap per run, so a huge store can't monopolise I/O.
    pub max_per_run: usize,
    pub interval_millis: u64,
}

/// Scrubbing runs on a schedule, "not every startup".
pub const fn scrub_due(last_scrub: Option<WallMillis>, now: WallMillis, p: ScrubPolicy) -> bool {
    match last_scrub {
        None => true,
        Some(t) => now.0.saturating_sub(t.0) >= p.interval_millis,
    }
}

/// How many of `total` blobs to hash this run.
pub const fn scrub_sample_size(total: usize, p: ScrubPolicy) -> usize {
    if total == 0 {
        return 0;
    }
    let wanted = (total as u128 * p.sample_permille as u128).div_ceil(1000) as usize;
    let wanted = if wanted == 0 { 1 } else { wanted };
    if wanted > p.max_per_run {
        p.max_per_run
    } else if wanted > total {
        total
    } else {
        wanted
    }
}

/// Picks `scrub_sample_size` *distinct* indices from `0..total`
/// pseudo-randomly from `seed` (partial Fisher–Yates over a sparse
/// map: O(k) memory, not O(total)). Different seeds over time give
/// eventual coverage of the whole store.
pub fn select_scrub_sample(total: usize, seed: u64, p: ScrubPolicy) -> Vec<usize> {
    use std::collections::HashMap;
    let k = scrub_sample_size(total, p);
    let mut swapped: HashMap<usize, usize> = HashMap::with_capacity(k);
    let mut out = Vec::with_capacity(k);
    for i in 0..k {
        let remaining = (total - i) as u64;
        let j = i + (jitter_millis(seed.wrapping_add(i as u64), remaining - 1) as usize);
        let vj = *swapped.get(&j).unwrap_or(&j);
        let vi = *swapped.get(&i).unwrap_or(&i);
        swapped.insert(j, vi);
        out.push(vj);
    }
    out
}

// ---------------------------------------------------------------------
// §141
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BitRotRepairSource {
    OtherOwnDevice,
    Backup,
    AuthorizedPeer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairAvailability {
    pub other_own_device: bool,
    pub backup: bool,
    /// An authorized peer — see §147 for what "authorized" demands.
    pub authorized_peer: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitRotPlan {
    pub blob: BlobCorruptionPlan,
    /// Tried in this order (§141's order).
    pub sources: Vec<BitRotRepairSource>,
}

/// A content-hash mismatch was detected. The blob loses trust at once;
/// redownload is planned only if some source exists.
pub fn plan_bit_rot_repair(a: RepairAvailability) -> BitRotPlan {
    let mut sources = Vec::with_capacity(3);
    if a.other_own_device {
        sources.push(BitRotRepairSource::OtherOwnDevice);
    }
    if a.backup {
        sources.push(BitRotRepairSource::Backup);
    }
    if a.authorized_peer {
        sources.push(BitRotRepairSource::AuthorizedPeer);
    }
    BitRotPlan {
        blob: plan_blob_corruption(!sources.is_empty()),
        sources,
    }
}

// ---------------------------------------------------------------------
// §142
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteBackupMethod {
    OnlineBackupApi,
    ConsistentSnapshot,
    /// Copying the database file(s) directly.
    RawFileCopy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCopyFacts {
    pub writers_quiesced: bool,
    pub wal_present: bool,
    /// The WAL has been checkpointed into the main file.
    pub wal_checkpointed: bool,
}

/// §142: "never copy live DB files blindly without understanding
/// WAL." A raw copy is only safe with writers stopped and no
/// un-checkpointed WAL content that the copy would miss.
pub const fn raw_copy_is_safe(f: RawCopyFacts) -> bool {
    f.writers_quiesced && (!f.wal_present || f.wal_checkpointed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no safe database backup method is available")]
pub struct NoSafeBackupMethod;

/// Picks the strongest safe method; a raw copy is a last resort and
/// only when [`raw_copy_is_safe`].
pub fn choose_backup_method(
    online_api_available: bool,
    snapshot_available: bool,
    raw: RawCopyFacts,
) -> Result<SqliteBackupMethod, NoSafeBackupMethod> {
    if online_api_available {
        Ok(SqliteBackupMethod::OnlineBackupApi)
    } else if snapshot_available {
        Ok(SqliteBackupMethod::ConsistentSnapshot)
    } else if raw_copy_is_safe(raw) {
        Ok(SqliteBackupMethod::RawFileCopy)
    } else {
        Err(NoSafeBackupMethod)
    }
}

// ---------------------------------------------------------------------
// §143
// ---------------------------------------------------------------------

/// The four steps of §143's drill, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DrillStep {
    CreateBackup = 0,
    RestoreToFreshEnvironment = 1,
    RebuildProjections = 2,
    VerifyLogicalState = 3,
}

impl DrillStep {
    pub const ALL: [DrillStep; 4] = [
        DrillStep::CreateBackup,
        DrillStep::RestoreToFreshEnvironment,
        DrillStep::RebuildProjections,
        DrillStep::VerifyLogicalState,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DrillError {
    #[error("drill step {attempted:?} reported before {expected:?}")]
    OutOfOrder {
        attempted: DrillStep,
        expected: DrillStep,
    },
    #[error("drill already finished")]
    AlreadyFinished,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BackupDrill {
    passed: usize,
    failed: bool,
}

impl BackupDrill {
    pub const fn new() -> Self {
        Self {
            passed: 0,
            failed: false,
        }
    }

    /// Records a step's outcome. Steps must be reported in order; a
    /// failed step fails the whole drill.
    pub fn record(&mut self, step: DrillStep, ok: bool) -> Result<(), DrillError> {
        if self.failed || self.passed == DrillStep::ALL.len() {
            return Err(DrillError::AlreadyFinished);
        }
        let expected = DrillStep::ALL[self.passed];
        if step != expected {
            return Err(DrillError::OutOfOrder {
                attempted: step,
                expected,
            });
        }
        if ok {
            self.passed += 1;
        } else {
            self.failed = true;
        }
        Ok(())
    }

    /// True only when every step ran and passed.
    pub const fn passed(&self) -> bool {
        !self.failed && self.passed == DrillStep::ALL.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupTrust {
    /// Never restored successfully: "not a recovery plan".
    Untested,
    Verified,
    /// Last drill is older than policy allows.
    Stale,
}

pub const fn backup_trust(
    last_passed_drill: Option<WallMillis>,
    now: WallMillis,
    max_age_millis: u64,
) -> BackupTrust {
    match last_passed_drill {
        None => BackupTrust::Untested,
        Some(t) => {
            if now.0.saturating_sub(t.0) <= max_age_millis {
                BackupTrust::Verified
            } else {
                BackupTrust::Stale
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corruption::BlobTrust;

    const P: ScrubPolicy = ScrubPolicy {
        sample_permille: 50,
        max_per_run: 100,
        interval_millis: 86_400_000,
    };

    #[test]
    fn scrub_is_periodic_not_every_startup() {
        assert!(scrub_due(None, WallMillis(1), P));
        assert!(!scrub_due(Some(WallMillis(1_000)), WallMillis(2_000), P));
        assert!(scrub_due(
            Some(WallMillis(0)),
            WallMillis(P.interval_millis),
            P
        ));
    }

    #[test]
    fn sample_size_is_a_fraction_capped_and_never_zero_for_nonempty() {
        assert_eq!(scrub_sample_size(0, P), 0);
        assert_eq!(scrub_sample_size(10, P), 1, "5% of 10 rounds up to 1");
        assert_eq!(scrub_sample_size(1_000, P), 50);
        assert_eq!(scrub_sample_size(1_000_000, P), 100, "capped");
        let all = ScrubPolicy {
            sample_permille: 1_000,
            max_per_run: usize::MAX,
            ..P
        };
        assert_eq!(scrub_sample_size(7, all), 7);
    }

    #[test]
    fn sample_is_distinct_in_range_and_deterministic() {
        for total in [1usize, 2, 10, 500] {
            let p = ScrubPolicy {
                sample_permille: 400,
                max_per_run: 1_000,
                ..P
            };
            let s = select_scrub_sample(total, 42, p);
            assert_eq!(s.len(), scrub_sample_size(total, p));
            let set: std::collections::HashSet<_> = s.iter().collect();
            assert_eq!(set.len(), s.len(), "no index twice");
            assert!(s.iter().all(|i| *i < total));
            assert_eq!(s, select_scrub_sample(total, 42, p));
        }
    }

    #[test]
    fn a_full_sample_is_a_permutation() {
        let p = ScrubPolicy {
            sample_permille: 1_000,
            max_per_run: 1_000,
            ..P
        };
        let mut s = select_scrub_sample(37, 7, p);
        s.sort();
        assert_eq!(s, (0..37).collect::<Vec<_>>());
    }

    #[test]
    fn different_seeds_eventually_cover_the_whole_store() {
        let total = 200;
        let mut seen = vec![false; total];
        for seed in 0..400u64 {
            for i in select_scrub_sample(total, seed, P) {
                seen[i] = true;
            }
        }
        assert!(
            seen.iter().all(|b| *b),
            "latent corruption can hide nowhere forever"
        );
    }

    #[test]
    fn bit_rot_sources_follow_the_spec_order() {
        let p = plan_bit_rot_repair(RepairAvailability {
            other_own_device: true,
            backup: true,
            authorized_peer: true,
        });
        assert_eq!(
            p.sources,
            vec![
                BitRotRepairSource::OtherOwnDevice,
                BitRotRepairSource::Backup,
                BitRotRepairSource::AuthorizedPeer
            ]
        );
        assert!(p.blob.redownload);
    }

    #[test]
    fn corrupt_blob_loses_trust_even_when_nothing_can_repair_it() {
        let p = plan_bit_rot_repair(RepairAvailability {
            other_own_device: false,
            backup: false,
            authorized_peer: false,
        });
        assert!(p.sources.is_empty());
        assert_eq!(p.blob.new_trust, BlobTrust::Corrupt);
        assert!(!p.blob.redownload);
    }

    fn raw(q: bool, wal: bool, ckpt: bool) -> RawCopyFacts {
        RawCopyFacts {
            writers_quiesced: q,
            wal_present: wal,
            wal_checkpointed: ckpt,
        }
    }

    #[test]
    fn raw_copy_needs_quiesced_writers_and_a_checkpointed_wal() {
        assert!(raw_copy_is_safe(raw(true, false, false)));
        assert!(raw_copy_is_safe(raw(true, true, true)));
        assert!(
            !raw_copy_is_safe(raw(true, true, false)),
            "un-checkpointed WAL would be missed"
        );
        assert!(!raw_copy_is_safe(raw(false, false, false)), "live writers");
    }

    #[test]
    fn backup_method_prefers_online_api_then_snapshot_then_safe_raw() {
        let unsafe_raw = raw(false, true, false);
        assert_eq!(
            choose_backup_method(true, true, unsafe_raw),
            Ok(SqliteBackupMethod::OnlineBackupApi)
        );
        assert_eq!(
            choose_backup_method(false, true, unsafe_raw),
            Ok(SqliteBackupMethod::ConsistentSnapshot)
        );
        assert_eq!(
            choose_backup_method(false, false, raw(true, false, false)),
            Ok(SqliteBackupMethod::RawFileCopy)
        );
        assert_eq!(
            choose_backup_method(false, false, unsafe_raw),
            Err(NoSafeBackupMethod)
        );
    }

    #[test]
    fn drill_passes_only_when_all_four_steps_pass_in_order() {
        let mut d = BackupDrill::new();
        assert!(!d.passed());
        for s in DrillStep::ALL {
            d.record(s, true).unwrap();
        }
        assert!(d.passed());
        assert_eq!(
            d.record(DrillStep::CreateBackup, true),
            Err(DrillError::AlreadyFinished)
        );
    }

    #[test]
    fn drill_rejects_skipped_steps_and_fails_on_a_failed_step() {
        let mut d = BackupDrill::new();
        assert_eq!(
            d.record(DrillStep::VerifyLogicalState, true),
            Err(DrillError::OutOfOrder {
                attempted: DrillStep::VerifyLogicalState,
                expected: DrillStep::CreateBackup
            })
        );
        d.record(DrillStep::CreateBackup, true).unwrap();
        d.record(DrillStep::RestoreToFreshEnvironment, false)
            .unwrap();
        assert!(!d.passed());
        assert_eq!(
            d.record(DrillStep::RebuildProjections, true),
            Err(DrillError::AlreadyFinished)
        );
    }

    #[test]
    fn untested_or_stale_backups_are_not_a_recovery_plan() {
        let day = 86_400_000;
        assert_eq!(
            backup_trust(None, WallMillis(5), 30 * day),
            BackupTrust::Untested
        );
        assert_eq!(
            backup_trust(Some(WallMillis(0)), WallMillis(10 * day), 30 * day),
            BackupTrust::Verified
        );
        assert_eq!(
            backup_trust(Some(WallMillis(0)), WallMillis(31 * day), 30 * day),
            BackupTrust::Stale
        );
    }
}
