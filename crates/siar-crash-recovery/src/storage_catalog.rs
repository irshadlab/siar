//! §134 "Authoritative vs Reconstructible Data" and §135 "Storage
//! Catalog".
//!
//! One table: every data set, its class, where it can be rebuilt
//! from, retention, backup, encryption and recovery method. The point
//! (§135) is to stop accidental misuse — e.g. treating a cache as
//! truth or deleting truth as a cache — so the table is `const` and
//! its structural rules are enforced by tests: adding a data set
//! without a catalog row, or a row that breaks a rule, fails the
//! build's tests.

use crate::derived_recovery::StoreClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataSet {
    // Authoritative (§134)
    EventLog,
    IdentityState,
    UserBlobs,
    DtnBundleStore,
    // Reconstructible (§134 plus the derived stores earlier sections name)
    SearchIndex,
    ConversationSummary,
    RouteCache,
    CapabilityCache,
    ThumbnailCache,
    Outbox,
    Projections,
    TransferJournal,
}

impl DataSet {
    pub const ALL: [DataSet; 12] = [
        DataSet::EventLog,
        DataSet::IdentityState,
        DataSet::UserBlobs,
        DataSet::DtnBundleStore,
        DataSet::SearchIndex,
        DataSet::ConversationSummary,
        DataSet::RouteCache,
        DataSet::CapabilityCache,
        DataSet::ThumbnailCache,
        DataSet::Outbox,
        DataSet::Projections,
        DataSet::TransferJournal,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClass {
    Authoritative,
    Reconstructible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildSource {
    /// Cannot be rebuilt locally; peers may *help* (§133) but are
    /// never required.
    NotRebuildable,
    EventLog,
    BlobStore,
    ChunkVerification,
    Rediscovery,
    Renegotiation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    Forever,
    UntilExpiry,
    Bounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMethod {
    /// Verify; on corruption quarantine / halt, never auto-delete.
    VerifyThenQuarantineOrHalt,
    DropAndRebuild,
    ReverifyChunks,
}

/// §135's six questions, one row per data set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    pub data_set: DataSet,
    pub class: DataClass,
    pub rebuild_source: RebuildSource,
    pub retention: Retention,
    pub in_backup: bool,
    pub encrypted_at_rest: bool,
    pub recovery_method: RecoveryMethod,
}

use DataClass::*;
use DataSet as D;
use RebuildSource as R;
use RecoveryMethod as M;
use Retention as T;

const fn row(
    data_set: DataSet,
    class: DataClass,
    rebuild_source: RebuildSource,
    retention: Retention,
    in_backup: bool,
    encrypted_at_rest: bool,
    recovery_method: RecoveryMethod,
) -> CatalogEntry {
    CatalogEntry {
        data_set,
        class,
        rebuild_source,
        retention,
        in_backup,
        encrypted_at_rest,
        recovery_method,
    }
}

/// The catalog. Order matches [`DataSet::ALL`].
pub const CATALOG: [CatalogEntry; 12] = [
    row(
        D::EventLog,
        Authoritative,
        R::NotRebuildable,
        T::Forever,
        true,
        true,
        M::VerifyThenQuarantineOrHalt,
    ),
    row(
        D::IdentityState,
        Authoritative,
        R::NotRebuildable,
        T::Forever,
        true,
        true,
        M::VerifyThenQuarantineOrHalt,
    ),
    row(
        D::UserBlobs,
        Authoritative,
        R::NotRebuildable,
        T::Forever,
        true,
        true,
        M::VerifyThenQuarantineOrHalt,
    ),
    // Carried for others and bounded by bundle expiry; DTN redundancy
    // tolerates losing it, so it is excluded from backups.
    row(
        D::DtnBundleStore,
        Authoritative,
        R::NotRebuildable,
        T::UntilExpiry,
        false,
        true,
        M::VerifyThenQuarantineOrHalt,
    ),
    row(
        D::SearchIndex,
        Reconstructible,
        R::EventLog,
        T::Forever,
        false,
        true,
        M::DropAndRebuild,
    ),
    row(
        D::ConversationSummary,
        Reconstructible,
        R::EventLog,
        T::Forever,
        false,
        true,
        M::DropAndRebuild,
    ),
    row(
        D::RouteCache,
        Reconstructible,
        R::Rediscovery,
        T::Bounded,
        false,
        false,
        M::DropAndRebuild,
    ),
    row(
        D::CapabilityCache,
        Reconstructible,
        R::Renegotiation,
        T::Bounded,
        false,
        false,
        M::DropAndRebuild,
    ),
    row(
        D::ThumbnailCache,
        Reconstructible,
        R::BlobStore,
        T::Bounded,
        false,
        true,
        M::DropAndRebuild,
    ),
    row(
        D::Outbox,
        Reconstructible,
        R::EventLog,
        T::UntilExpiry,
        false,
        true,
        M::DropAndRebuild,
    ),
    row(
        D::Projections,
        Reconstructible,
        R::EventLog,
        T::Forever,
        false,
        true,
        M::DropAndRebuild,
    ),
    row(
        D::TransferJournal,
        Reconstructible,
        R::ChunkVerification,
        T::UntilExpiry,
        false,
        true,
        M::ReverifyChunks,
    ),
];

pub fn entry_for(d: DataSet) -> &'static CatalogEntry {
    CATALOG
        .iter()
        .find(|e| e.data_set == d)
        .expect("every DataSet has a catalog row (enforced by tests)")
}

/// Bridges §134's class to §42's corruption response.
pub fn store_class(d: DataSet) -> StoreClass {
    match entry_for(d).class {
        DataClass::Authoritative => StoreClass::Authoritative,
        DataClass::Reconstructible => StoreClass::DerivedNoncritical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derived_recovery::{respond_to_corruption, CorruptionResponse};
    use std::collections::HashSet;

    #[test]
    fn every_data_set_has_exactly_one_row_in_order() {
        let sets: HashSet<_> = CATALOG.iter().map(|e| e.data_set).collect();
        assert_eq!(sets.len(), CATALOG.len(), "no duplicates");
        for (i, d) in DataSet::ALL.iter().enumerate() {
            assert_eq!(CATALOG[i].data_set, *d, "row {i} out of order");
            assert_eq!(entry_for(*d).data_set, *d);
        }
    }

    #[test]
    fn spec_lists_are_classified_as_the_spec_says() {
        for d in [D::EventLog, D::IdentityState, D::UserBlobs] {
            assert_eq!(entry_for(d).class, Authoritative, "{d:?}");
        }
        for d in [
            D::SearchIndex,
            D::ConversationSummary,
            D::RouteCache,
            D::CapabilityCache,
            D::ThumbnailCache,
        ] {
            assert_eq!(entry_for(d).class, Reconstructible, "{d:?}");
        }
    }

    #[test]
    fn authoritative_data_is_encrypted_never_rebuilt_and_verified_not_dropped() {
        for e in CATALOG.iter().filter(|e| e.class == Authoritative) {
            assert!(e.encrypted_at_rest, "{:?}", e.data_set);
            assert_eq!(e.rebuild_source, R::NotRebuildable, "{:?}", e.data_set);
            assert_eq!(
                e.recovery_method,
                M::VerifyThenQuarantineOrHalt,
                "{:?}",
                e.data_set
            );
        }
    }

    #[test]
    fn reconstructible_data_names_a_source_and_is_never_backed_up() {
        for e in CATALOG.iter().filter(|e| e.class == Reconstructible) {
            assert_ne!(e.rebuild_source, R::NotRebuildable, "{:?}", e.data_set);
            assert!(
                !e.in_backup,
                "{:?}: rebuildable data wastes backup space",
                e.data_set
            );
            assert_ne!(
                e.recovery_method,
                M::VerifyThenQuarantineOrHalt,
                "{:?}",
                e.data_set
            );
        }
    }

    #[test]
    fn user_owned_authoritative_data_is_backed_up() {
        for d in [D::EventLog, D::IdentityState, D::UserBlobs] {
            assert!(entry_for(d).in_backup, "{d:?}");
        }
    }

    #[test]
    fn rebuild_sources_are_authoritative_or_live_not_other_caches() {
        // A projection must come from the event log, never from
        // another derived store (which could itself be stale).
        for d in [
            D::Projections,
            D::SearchIndex,
            D::ConversationSummary,
            D::Outbox,
        ] {
            assert_eq!(entry_for(d).rebuild_source, R::EventLog, "{d:?}");
        }
        assert_eq!(entry_for(D::ThumbnailCache).rebuild_source, R::BlobStore);
    }

    #[test]
    fn class_drives_the_corruption_response() {
        for e in &CATALOG {
            let r = respond_to_corruption(store_class(e.data_set));
            match e.class {
                Authoritative => assert_eq!(r, CorruptionResponse::Halt, "{:?}", e.data_set),
                Reconstructible => {
                    assert_eq!(r, CorruptionResponse::DropAndRebuild, "{:?}", e.data_set)
                }
            }
        }
    }

    #[test]
    fn bounded_retention_is_only_for_caches() {
        for e in CATALOG.iter().filter(|e| e.retention == T::Bounded) {
            assert_eq!(e.class, Reconstructible, "{:?}", e.data_set);
        }
    }
}
