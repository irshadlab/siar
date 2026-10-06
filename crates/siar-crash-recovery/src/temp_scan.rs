//! §23 "Temp Files" and §24 "Orphan Cleanup".
//!
//! §23 requires opaque temp-file ids and a recovery scan of
//! `staging/` and `temp/` that classifies each file as one of: owned
//! by an active transfer, orphan, too new, expired, corrupt. §24 then
//! forbids "delete every unknown temp file after crash" and names
//! three levers — ownership metadata, an age threshold, and a
//! transaction generation. [`classify_temp_file`] is §23's
//! classification; [`cleanup_decision`] is §24's policy over it. Both
//! are pure: time and the active-transfer set are passed in.

use crate::chunk_recovery::TransferId;
use crate::shutdown_marker::RuntimeGeneration;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// §23: temp files carry an opaque id — nothing about the transfer,
/// peer or content is derivable from the file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpaqueTempId(uuid::Uuid);

impl OpaqueTempId {
    const PREFIX: &'static str = "tmp-";

    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// File name to use on disk.
    pub fn file_name(&self) -> String {
        format!("{}{}", Self::PREFIX, self.0.simple())
    }

    /// Parses a file name produced by [`OpaqueTempId::file_name`];
    /// `None` for anything else (which recovery must treat as an
    /// unknown file, never silently as "ours").
    pub fn parse_file_name(name: &str) -> Option<Self> {
        let hex = name.strip_prefix(Self::PREFIX)?;
        if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        uuid::Uuid::try_parse(hex).ok().map(Self)
    }
}

impl Default for OpaqueTempId {
    fn default() -> Self {
        Self::new()
    }
}

/// The directories §23 says recovery scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TempArea {
    Staging,
    Temp,
}

/// §24 "ownership metadata": which transfer created the file, and in
/// which runtime generation ("transaction generation").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempOwnership {
    pub transfer: TransferId,
    pub generation: RuntimeGeneration,
}

/// One file found by the scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempFileEntry {
    pub area: TempArea,
    /// `None` when the file name is not an opaque id we issued.
    pub id: Option<OpaqueTempId>,
    /// `None` when no ownership sidecar/record exists.
    pub ownership: Option<TempOwnership>,
    pub created_at_millis: u64,
    /// Known-bad: failed a verification, or has unreadable metadata.
    pub corrupt: bool,
}

/// §23's five classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TempClass {
    OwnedByActiveTransfer,
    Orphan,
    TooNew,
    Expired,
    Corrupt,
}

/// §24's age threshold, plus the hard expiry after which even an
/// unowned file may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupPolicy {
    min_age_millis: u64,
    expiry_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cleanup policy needs min_age ({min_age_millis}) < expiry ({expiry_millis})")]
pub struct InvalidCleanupPolicy {
    pub min_age_millis: u64,
    pub expiry_millis: u64,
}

impl CleanupPolicy {
    pub fn new(min_age_millis: u64, expiry_millis: u64) -> Result<Self, InvalidCleanupPolicy> {
        if min_age_millis < expiry_millis {
            Ok(Self {
                min_age_millis,
                expiry_millis,
            })
        } else {
            Err(InvalidCleanupPolicy {
                min_age_millis,
                expiry_millis,
            })
        }
    }

    pub const fn min_age_millis(&self) -> u64 {
        self.min_age_millis
    }

    pub const fn expiry_millis(&self) -> u64 {
        self.expiry_millis
    }
}

/// Everything classification needs besides the file itself.
#[derive(Debug, Clone, Copy)]
pub struct ScanContext<'a> {
    pub now_millis: u64,
    /// The runtime generation performing this recovery. Files written
    /// by this generation are by definition not crash leftovers.
    pub current_generation: RuntimeGeneration,
    pub active_transfers: &'a HashSet<TransferId>,
    pub policy: CleanupPolicy,
}

/// §23 classification. Precedence (first match wins):
/// corrupt → active owner → too new → expired → orphan.
///
/// "Too new" covers both the age threshold and the generation rule: a
/// file stamped with the current (or a later) generation is never
/// treated as a leftover regardless of its timestamp, and a timestamp
/// in the future (clock skew) counts as age zero.
pub fn classify_temp_file(entry: &TempFileEntry, ctx: &ScanContext<'_>) -> TempClass {
    if entry.corrupt {
        return TempClass::Corrupt;
    }
    if let Some(o) = entry.ownership {
        if ctx.active_transfers.contains(&o.transfer) {
            return TempClass::OwnedByActiveTransfer;
        }
        if o.generation >= ctx.current_generation {
            return TempClass::TooNew;
        }
    }
    let age = ctx.now_millis.saturating_sub(entry.created_at_millis);
    if age < ctx.policy.min_age_millis {
        TempClass::TooNew
    } else if age >= ctx.policy.expiry_millis {
        TempClass::Expired
    } else {
        TempClass::Orphan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepReason {
    ActiveTransfer,
    TooNew,
    /// Not provably ours and not yet past hard expiry (§24: do not
    /// delete unknown files immediately).
    UnknownOwnerWithinRetention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupDecision {
    Keep(KeepReason),
    Delete,
    /// Corrupt files are set aside for inspection/§53-§54 handling
    /// rather than silently deleted.
    Quarantine,
}

/// §24 policy over a §23 class.
///
/// * Expired → delete (hard expiry, owner or not).
/// * Orphan with ownership metadata naming a dead transfer and an
///   older generation → delete.
/// * Orphan with *no* ownership metadata (or an unparseable name) →
///   keep until hard expiry.
pub fn cleanup_decision(entry: &TempFileEntry, class: TempClass) -> CleanupDecision {
    match class {
        TempClass::Corrupt => CleanupDecision::Quarantine,
        TempClass::OwnedByActiveTransfer => CleanupDecision::Keep(KeepReason::ActiveTransfer),
        TempClass::TooNew => CleanupDecision::Keep(KeepReason::TooNew),
        TempClass::Expired => CleanupDecision::Delete,
        TempClass::Orphan => {
            if entry.ownership.is_some() && entry.id.is_some() {
                CleanupDecision::Delete
            } else {
                CleanupDecision::Keep(KeepReason::UnknownOwnerWithinRetention)
            }
        }
    }
}

/// Classifies and decides for every entry, in input order.
pub fn plan_cleanup(
    entries: &[TempFileEntry],
    ctx: &ScanContext<'_>,
) -> Vec<(TempClass, CleanupDecision)> {
    entries
        .iter()
        .map(|e| {
            let c = classify_temp_file(e, ctx);
            (c, cleanup_decision(e, c))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;
    const EXP: u64 = 86_400_000;

    fn policy() -> CleanupPolicy {
        CleanupPolicy::new(MIN, EXP).unwrap()
    }

    fn entry(owner: Option<(TransferId, u64)>, created: u64) -> TempFileEntry {
        TempFileEntry {
            area: TempArea::Staging,
            id: Some(OpaqueTempId::new()),
            ownership: owner.map(|(t, g)| TempOwnership {
                transfer: t,
                generation: RuntimeGeneration(g),
            }),
            created_at_millis: created,
            corrupt: false,
        }
    }

    fn ctx<'a>(active: &'a HashSet<TransferId>, now: u64) -> ScanContext<'a> {
        ScanContext {
            now_millis: now,
            current_generation: RuntimeGeneration(5),
            active_transfers: active,
            policy: policy(),
        }
    }

    #[test]
    fn opaque_names_round_trip_and_reject_foreign_names() {
        let id = OpaqueTempId::new();
        assert_eq!(OpaqueTempId::parse_file_name(&id.file_name()), Some(id));
        assert_eq!(OpaqueTempId::parse_file_name("tmp-xyz"), None);
        assert_eq!(OpaqueTempId::parse_file_name("photo.jpg"), None);
        assert_eq!(
            OpaqueTempId::parse_file_name(&format!("tmp-{}", "g".repeat(32))),
            None
        );
    }

    #[test]
    fn policy_rejects_inverted_thresholds() {
        assert!(CleanupPolicy::new(10, 10).is_err());
        assert!(CleanupPolicy::new(11, 10).is_err());
    }

    #[test]
    fn active_owner_is_never_touched_even_when_ancient() {
        let t = TransferId::new();
        let active: HashSet<_> = [t].into();
        let e = entry(Some((t, 1)), 0);
        let c = classify_temp_file(&e, &ctx(&active, EXP * 10));
        assert_eq!(c, TempClass::OwnedByActiveTransfer);
        assert_eq!(
            cleanup_decision(&e, c),
            CleanupDecision::Keep(KeepReason::ActiveTransfer)
        );
    }

    #[test]
    fn dead_owner_older_generation_past_min_age_is_deleted() {
        let active = HashSet::new();
        let e = entry(Some((TransferId::new(), 3)), 0);
        let c = classify_temp_file(&e, &ctx(&active, MIN * 2));
        assert_eq!(c, TempClass::Orphan);
        assert_eq!(cleanup_decision(&e, c), CleanupDecision::Delete);
    }

    #[test]
    fn current_generation_files_are_too_new_regardless_of_age() {
        let active = HashSet::new();
        let e = entry(Some((TransferId::new(), 5)), 0);
        assert_eq!(
            classify_temp_file(&e, &ctx(&active, EXP * 3)),
            TempClass::TooNew
        );
    }

    #[test]
    fn young_files_and_future_timestamps_are_too_new() {
        let active = HashSet::new();
        let young = entry(Some((TransferId::new(), 1)), 1_000);
        assert_eq!(
            classify_temp_file(&young, &ctx(&active, 1_000 + MIN - 1)),
            TempClass::TooNew
        );
        let skewed = entry(None, 9_999_999);
        assert_eq!(
            classify_temp_file(&skewed, &ctx(&active, 1_000)),
            TempClass::TooNew
        );
    }

    #[test]
    fn unknown_owner_is_kept_until_hard_expiry_then_deleted() {
        let active = HashSet::new();
        let e = entry(None, 0);
        let mid = classify_temp_file(&e, &ctx(&active, MIN * 2));
        assert_eq!(mid, TempClass::Orphan);
        assert_eq!(
            cleanup_decision(&e, mid),
            CleanupDecision::Keep(KeepReason::UnknownOwnerWithinRetention)
        );
        let late = classify_temp_file(&e, &ctx(&active, EXP));
        assert_eq!(late, TempClass::Expired);
        assert_eq!(cleanup_decision(&e, late), CleanupDecision::Delete);
    }

    #[test]
    fn foreign_file_name_is_not_treated_as_ours() {
        let active = HashSet::new();
        let mut e = entry(Some((TransferId::new(), 1)), 0);
        e.id = None;
        let c = classify_temp_file(&e, &ctx(&active, MIN * 2));
        assert_eq!(
            cleanup_decision(&e, c),
            CleanupDecision::Keep(KeepReason::UnknownOwnerWithinRetention)
        );
    }

    #[test]
    fn corrupt_is_quarantined_not_deleted_and_wins_over_active_owner() {
        let t = TransferId::new();
        let active: HashSet<_> = [t].into();
        let mut e = entry(Some((t, 1)), 0);
        e.corrupt = true;
        let c = classify_temp_file(&e, &ctx(&active, MIN * 2));
        assert_eq!(c, TempClass::Corrupt);
        assert_eq!(cleanup_decision(&e, c), CleanupDecision::Quarantine);
    }

    #[test]
    fn plan_preserves_order_and_covers_all_five_classes() {
        let live = TransferId::new();
        let active: HashSet<_> = [live].into();
        let mut corrupt = entry(None, 0);
        corrupt.corrupt = true;
        let entries = vec![
            entry(Some((live, 1)), 0),                    // active
            entry(Some((TransferId::new(), 1)), 0),       // orphan (now = 2*MIN)
            entry(Some((TransferId::new(), 1)), MIN * 2), // too new
            corrupt,                                      // corrupt
        ];
        let plan = plan_cleanup(&entries, &ctx(&active, MIN * 2));
        let classes: Vec<_> = plan.iter().map(|p| p.0).collect();
        assert_eq!(
            classes,
            vec![
                TempClass::OwnedByActiveTransfer,
                TempClass::Orphan,
                TempClass::TooNew,
                TempClass::Corrupt
            ]
        );
        let exp = entry(None, 0);
        let (c, d) = plan_cleanup(&[exp], &ctx(&active, EXP))[0];
        assert_eq!((c, d), (TempClass::Expired, CleanupDecision::Delete));
    }
}
