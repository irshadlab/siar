//! §181 "Recovery Error Model".
//!
//! [`RecoveryFailure`] is the spec's ten-variant enum, verbatim, and
//! is what every subsystem reports when its recovery fails (it
//! replaced the five-variant `RecoveryErrorKind` the coordinator
//! started with, which couldn't express half of §181). Variants are
//! codes, not messages, so an error can't carry a secret.
//!
//! Each failure has a *default* severity; a subsystem that knows
//! better (e.g. an integrity failure confined to a derived store)
//! overrides it in the `RecoveryError` it returns. Conversions from
//! this crate's other rejection types keep the mapping in one place.

use crate::backup_restore::BackupRejection;
use crate::corruption::CorruptionScope;
use crate::migration_recovery::{DowngradeRejected, OpenCompatibility};
use crate::recovery_mode::RecoverySeverity;
use serde::{Deserialize, Serialize};

/// §181, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
pub enum RecoveryFailure {
    #[error("storage unavailable")]
    StorageUnavailable,
    #[error("integrity failure")]
    IntegrityFailure,
    #[error("migration failure")]
    MigrationFailure,
    #[error("identity failure")]
    IdentityFailure,
    #[error("projection failure")]
    ProjectionFailure,
    #[error("blob failure")]
    BlobFailure,
    #[error("DTN failure")]
    DtnFailure,
    #[error("secure store failure")]
    SecureStoreFailure,
    #[error("unsupported version")]
    UnsupportedVersion,
    #[error("fatal invariant violated")]
    FatalInvariant,
}

impl RecoveryFailure {
    pub const ALL: [RecoveryFailure; 10] = [
        RecoveryFailure::StorageUnavailable,
        RecoveryFailure::IntegrityFailure,
        RecoveryFailure::MigrationFailure,
        RecoveryFailure::IdentityFailure,
        RecoveryFailure::ProjectionFailure,
        RecoveryFailure::BlobFailure,
        RecoveryFailure::DtnFailure,
        RecoveryFailure::SecureStoreFailure,
        RecoveryFailure::UnsupportedVersion,
        RecoveryFailure::FatalInvariant,
    ];

    /// Severity when nothing more specific is known.
    ///
    /// * Authoritative or cannot-proceed failures stop the app:
    ///   storage gone, unsupported version, a violated invariant →
    ///   `Fatal`.
    /// * Integrity, migration, identity and secure-store failures
    ///   stop *writes and trusted communication* but leave local data
    ///   viewable → `ReadOnly` (matching `corruption::response_for`).
    /// * Derived or peripheral failures (projection, blob, DTN) never
    ///   take the app down → `Degraded`.
    pub const fn default_severity(self) -> RecoverySeverity {
        match self {
            RecoveryFailure::StorageUnavailable
            | RecoveryFailure::UnsupportedVersion
            | RecoveryFailure::FatalInvariant => RecoverySeverity::Fatal,
            RecoveryFailure::IntegrityFailure
            | RecoveryFailure::MigrationFailure
            | RecoveryFailure::IdentityFailure
            | RecoveryFailure::SecureStoreFailure => RecoverySeverity::ReadOnly,
            RecoveryFailure::ProjectionFailure
            | RecoveryFailure::BlobFailure
            | RecoveryFailure::DtnFailure => RecoverySeverity::Degraded,
        }
    }
}

impl From<DowngradeRejected> for RecoveryFailure {
    fn from(_: DowngradeRejected) -> Self {
        RecoveryFailure::UnsupportedVersion
    }
}

impl From<BackupRejection> for RecoveryFailure {
    fn from(r: BackupRejection) -> Self {
        match r {
            BackupRejection::UnsupportedVersion(_) => RecoveryFailure::UnsupportedVersion,
            BackupRejection::Unauthenticated | BackupRejection::IntegrityFailed => {
                RecoveryFailure::IntegrityFailure
            }
        }
    }
}

impl From<CorruptionScope> for RecoveryFailure {
    fn from(s: CorruptionScope) -> Self {
        match s {
            CorruptionScope::SingleBlob { .. } => RecoveryFailure::BlobFailure,
            CorruptionScope::OneProjection => RecoveryFailure::ProjectionFailure,
            CorruptionScope::OneEventStream | CorruptionScope::EntireDb => {
                RecoveryFailure::IntegrityFailure
            }
            CorruptionScope::SecureStore => RecoveryFailure::SecureStoreFailure,
        }
    }
}

/// `None` when the database can be opened (directly or by migrating).
pub const fn failure_for_open(c: OpenCompatibility) -> Option<RecoveryFailure> {
    match c {
        OpenCompatibility::Current | OpenCompatibility::NeedsMigration { .. } => None,
        OpenCompatibility::TooNew | OpenCompatibility::TooOld => {
            Some(RecoveryFailure::UnsupportedVersion)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corruption::response_for;
    use crate::recovery_security::RollbackVerdict;

    #[test]
    fn exactly_the_ten_spec_variants_each_listed_once() {
        let set: std::collections::HashSet<_> = RecoveryFailure::ALL.iter().collect();
        assert_eq!(set.len(), 10);
    }

    #[test]
    fn severities_follow_the_stop_writes_vs_stop_everything_split() {
        use RecoveryFailure::*;
        for f in [StorageUnavailable, UnsupportedVersion, FatalInvariant] {
            assert_eq!(f.default_severity(), RecoverySeverity::Fatal, "{f:?}");
        }
        for f in [
            IntegrityFailure,
            MigrationFailure,
            IdentityFailure,
            SecureStoreFailure,
        ] {
            assert_eq!(f.default_severity(), RecoverySeverity::ReadOnly, "{f:?}");
        }
        for f in [ProjectionFailure, BlobFailure, DtnFailure] {
            assert_eq!(f.default_severity(), RecoverySeverity::Degraded, "{f:?}");
        }
    }

    #[test]
    fn mapping_agrees_with_the_corruption_scope_table() {
        // A scope's severity is the outcome when corruption is
        // *handled*; the failure's default is the outcome when
        // handling *failed*.
        //
        // Authoritative scopes can't be "handled away", so both agree
        // (the scope is at least as severe as the failure default).
        for scope in [
            CorruptionScope::OneEventStream,
            CorruptionScope::EntireDb,
            CorruptionScope::SecureStore,
        ] {
            let f = RecoveryFailure::from(scope);
            assert!(
                response_for(scope).severity >= f.default_severity(),
                "{scope:?} vs {f:?}"
            );
        }
        // Derived scopes are repairable, so handling them is milder
        // than the failure of handling them.
        for scope in [
            CorruptionScope::OneProjection,
            CorruptionScope::SingleBlob { reacquirable: true },
        ] {
            let f = RecoveryFailure::from(scope);
            assert!(
                response_for(scope).severity < f.default_severity(),
                "{scope:?} handled should be milder than failed {f:?}"
            );
        }
        assert_eq!(
            response_for(CorruptionScope::SecureStore).severity,
            RecoveryFailure::SecureStoreFailure.default_severity()
        );
    }

    #[test]
    fn rollback_and_identity_failure_agree() {
        let v = RollbackVerdict::StaleState {
            local: 1,
            newest_known: 2,
        };
        assert_eq!(
            v.severity(),
            RecoveryFailure::IdentityFailure.default_severity()
        );
    }

    #[test]
    fn conversions_from_other_rejections() {
        use crate::backup_restore::BackupRejection as B;
        assert_eq!(
            RecoveryFailure::from(DowngradeRejected {
                binary: 1,
                min_reader: 2
            }),
            RecoveryFailure::UnsupportedVersion
        );
        assert_eq!(
            RecoveryFailure::from(B::Unauthenticated),
            RecoveryFailure::IntegrityFailure
        );
        assert_eq!(
            RecoveryFailure::from(B::IntegrityFailed),
            RecoveryFailure::IntegrityFailure
        );
        assert_eq!(
            RecoveryFailure::from(B::UnsupportedVersion(9)),
            RecoveryFailure::UnsupportedVersion
        );
    }

    #[test]
    fn open_compatibility_only_fails_when_the_version_is_unreadable() {
        assert_eq!(failure_for_open(OpenCompatibility::Current), None);
        assert_eq!(
            failure_for_open(OpenCompatibility::NeedsMigration { from: 3 }),
            None
        );
        assert_eq!(
            failure_for_open(OpenCompatibility::TooNew),
            Some(RecoveryFailure::UnsupportedVersion)
        );
        assert_eq!(
            failure_for_open(OpenCompatibility::TooOld),
            Some(RecoveryFailure::UnsupportedVersion)
        );
    }

    #[test]
    fn errors_are_codes_with_stable_display_text() {
        assert_eq!(
            RecoveryFailure::SecureStoreFailure.to_string(),
            "secure store failure"
        );
        assert_eq!(RecoveryFailure::DtnFailure.to_string(), "DTN failure");
    }
}
