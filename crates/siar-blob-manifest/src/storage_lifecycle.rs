//! §62 "Cache vs Durable Blob", §63 "Garbage Collection", §64 "Grace
//! Period", §65 "Cache Eviction", §66 "Storage Pressure".

use serde::{Deserialize, Serialize};

use crate::blob_references::{PinReason, ReferenceSet};

/// §62, field-for-field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobClassification {
    Durable,
    Cache,
    Temporary,
    Relay,
}

/// §64: "Use configurable policy" — the one number §63's flow needs
/// that isn't itself a per-blob fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcPolicy {
    pub grace_period_millis: u64,
}

/// §63's own four-step flow, collapsed into the one real yes/no
/// question a caller needs: is this specific blob safe to delete
/// right now. `unreferenced_since_millis` is `None` for a blob that
/// currently has real references (never eligible, full stop,
/// regardless of how long ago it was created) — §63's own "never
/// delete a blob solely because it is old" is enforced by this
/// function never looking at a blob's creation time at all, only at
/// how long it's been *unreferenced* and whether an active transfer is
/// using it.
pub fn is_gc_eligible(
    references: &ReferenceSet,
    unreferenced_since_millis: Option<u64>,
    now_millis: u64,
    policy: &GcPolicy,
    has_active_transfer: bool,
) -> bool {
    if !references.is_gc_eligible() {
        return false;
    }
    if has_active_transfer {
        return false;
    }
    let Some(unreferenced_since) = unreferenced_since_millis else {
        // No references (checked above) but no recorded moment they
        // were all removed either — treat as "just became eligible,"
        // i.e. the grace period hasn't started counting down yet.
        return false;
    };
    now_millis.saturating_sub(unreferenced_since) >= policy.grace_period_millis
}

/// §65's own five-tier eviction order, same `Ord`-by-declaration-order
/// pattern this workspace already uses for
/// `siar_dtn_bundle::eviction::EvictionTier` — evict ascending, never
/// FIFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CacheEvictionTier {
    ExpiredTemp,
    UnreferencedPreview,
    OldCache,
    CompletedRelay,
    LargeLowPriority,
}

/// §65's own "do not evict" list, mapped onto the [`PinReason`]s that
/// mean the same three things: `UserSaved` ("user-saved"),
/// `ActiveTransfer` ("active"), `DtnRequired` ("critical DTN").
/// `MessageReference`/`BackupPending` are real pins but aren't on
/// §65's own protected list, so they're deliberately excluded here —
/// a blob pinned only for those reasons can still be a normal eviction
/// candidate.
pub fn is_eviction_protected(pins: &[PinReason]) -> bool {
    pins.iter().any(|p| {
        matches!(
            p,
            PinReason::UserSaved | PinReason::ActiveTransfer | PinReason::DtnRequired
        )
    })
}

/// §65's own escape hatch: "unless emergency storage policy explicitly
/// allows." A separate bool parameter here, not folded into
/// [`is_eviction_protected`] itself, so a caller can't accidentally
/// pass `true` as an ordinary default — bypassing protection has to be
/// a deliberate, named argument at every call site.
pub fn may_evict(pins: &[PinReason], emergency_override: bool) -> bool {
    emergency_override || !is_eviction_protected(pins)
}

/// §66, field-for-field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoragePressure {
    Normal,
    Elevated,
    Critical,
    Full,
}

/// §66's own five named effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoragePressureEffects {
    pub reduce_prefetch: bool,
    pub pause_bulk_receive: bool,
    pub evict_cache: bool,
    pub reject_new_large_files: bool,
    pub preserve_critical_data: bool,
}

/// A deterministic, escalating mapping — each tier includes every
/// effect the previous tier already triggers, plus its own. The spec
/// doesn't give this mapping as a table, so this is this crate's own
/// reasonable reading of "reduce prefetch" (mild, `Elevated`) through
/// "reject new large files" (severe, `Full`) as an ordered escalation
/// rather than four independent, possibly-inconsistent policies.
/// `preserve_critical_data` is `true` at every level above `Normal` —
/// it's the one effect that should already be happening the moment
/// pressure exists at all, not held back for the worst tier.
pub fn effects_for(pressure: StoragePressure) -> StoragePressureEffects {
    match pressure {
        StoragePressure::Normal => StoragePressureEffects {
            reduce_prefetch: false,
            pause_bulk_receive: false,
            evict_cache: false,
            reject_new_large_files: false,
            preserve_critical_data: false,
        },
        StoragePressure::Elevated => StoragePressureEffects {
            reduce_prefetch: true,
            pause_bulk_receive: false,
            evict_cache: false,
            reject_new_large_files: false,
            preserve_critical_data: true,
        },
        StoragePressure::Critical => StoragePressureEffects {
            reduce_prefetch: true,
            pause_bulk_receive: true,
            evict_cache: true,
            reject_new_large_files: false,
            preserve_critical_data: true,
        },
        StoragePressure::Full => StoragePressureEffects {
            reduce_prefetch: true,
            pause_bulk_receive: true,
            evict_cache: true,
            reject_new_large_files: true,
            preserve_critical_data: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_references::ReferenceKind;

    fn policy() -> GcPolicy {
        GcPolicy {
            grace_period_millis: 1_000,
        }
    }

    #[test]
    fn a_referenced_blob_is_never_gc_eligible_regardless_of_time() {
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::Draft);
        assert!(!is_gc_eligible(&refs, Some(0), 1_000_000, &policy(), false));
    }

    #[test]
    fn an_unreferenced_blob_still_inside_the_grace_period_is_not_yet_eligible() {
        let refs = ReferenceSet::new();
        assert!(!is_gc_eligible(&refs, Some(500), 1_000, &policy(), false));
    }

    #[test]
    fn an_unreferenced_blob_past_the_grace_period_is_eligible() {
        let refs = ReferenceSet::new();
        assert!(is_gc_eligible(&refs, Some(0), 1_000, &policy(), false));
    }

    #[test]
    fn an_active_transfer_blocks_eligibility_even_past_the_grace_period() {
        let refs = ReferenceSet::new();
        assert!(!is_gc_eligible(&refs, Some(0), 1_000_000, &policy(), true));
    }

    #[test]
    fn old_age_alone_never_makes_a_referenced_blob_eligible() {
        // §63: "never delete a blob solely because it is old" —
        // `now_millis` here is enormous, but the blob still has a real
        // reference, so it must stay ineligible.
        let mut refs = ReferenceSet::new();
        refs.add_reference(ReferenceKind::Backup);
        assert!(!is_gc_eligible(&refs, None, u64::MAX, &policy(), false));
    }

    #[test]
    fn the_five_eviction_tiers_rank_in_the_spec_s_own_order() {
        assert!(CacheEvictionTier::ExpiredTemp < CacheEvictionTier::UnreferencedPreview);
        assert!(CacheEvictionTier::UnreferencedPreview < CacheEvictionTier::OldCache);
        assert!(CacheEvictionTier::OldCache < CacheEvictionTier::CompletedRelay);
        assert!(CacheEvictionTier::CompletedRelay < CacheEvictionTier::LargeLowPriority);
    }

    #[test]
    fn user_saved_active_and_dtn_required_are_protected_from_eviction() {
        assert!(is_eviction_protected(&[PinReason::UserSaved]));
        assert!(is_eviction_protected(&[PinReason::ActiveTransfer]));
        assert!(is_eviction_protected(&[PinReason::DtnRequired]));
    }

    #[test]
    fn message_reference_and_backup_pending_are_real_pins_but_not_eviction_protected() {
        assert!(!is_eviction_protected(&[PinReason::MessageReference]));
        assert!(!is_eviction_protected(&[PinReason::BackupPending]));
    }

    #[test]
    fn a_protected_blob_can_still_be_evicted_under_an_explicit_emergency_override() {
        let pins = [PinReason::UserSaved];
        assert!(!may_evict(&pins, false));
        assert!(may_evict(&pins, true));
    }

    #[test]
    fn storage_pressure_effects_escalate_monotonically() {
        let normal = effects_for(StoragePressure::Normal);
        let elevated = effects_for(StoragePressure::Elevated);
        let critical = effects_for(StoragePressure::Critical);
        let full = effects_for(StoragePressure::Full);
        assert!(!normal.reduce_prefetch && !normal.preserve_critical_data);
        assert!(elevated.reduce_prefetch && elevated.preserve_critical_data);
        assert!(critical.evict_cache && !critical.reject_new_large_files);
        assert!(full.reject_new_large_files);
    }
}
