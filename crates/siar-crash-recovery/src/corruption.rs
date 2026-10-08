//! §51 "Corruption Detection", §52 "Corruption Scope", §53 "Single
//! Blob Corruption", §54 "Transfer Journal Corruption", §55 "Event
//! Stream Corruption", §56 "Secure Store Failure".
//!
//! §52's instruction is "do not treat all corruption the same":
//! [`response_for`] maps each of the five scopes to its own outcome
//! rather than one blanket policy. Detection itself (SQLite integrity
//! check, hashing, signature verification) belongs to the crates that
//! own those formats; this module names *which* check applies
//! ([`mechanism_for`]) and decides what recovery does with a result.

use crate::recovery_mode::RecoverySeverity;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §51
// ---------------------------------------------------------------------

/// What is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataDomain {
    RelationalDb,
    EventRecord,
    BlobContent,
    Manifest,
    Snapshot,
    IdentityState,
}

/// §51's six mechanisms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectionMechanism {
    SqliteIntegrityCheck,
    EventChecksum,
    BlobHash,
    ManifestAuthentication,
    SnapshotChecksum,
    IdentitySignature,
}

/// §51: "use domain-appropriate checks."
pub const fn mechanism_for(d: DataDomain) -> DetectionMechanism {
    match d {
        DataDomain::RelationalDb => DetectionMechanism::SqliteIntegrityCheck,
        DataDomain::EventRecord => DetectionMechanism::EventChecksum,
        DataDomain::BlobContent => DetectionMechanism::BlobHash,
        DataDomain::Manifest => DetectionMechanism::ManifestAuthentication,
        DataDomain::Snapshot => DetectionMechanism::SnapshotChecksum,
        DataDomain::IdentityState => DetectionMechanism::IdentitySignature,
    }
}

// ---------------------------------------------------------------------
// §52
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CorruptionScope {
    SingleBlob { reacquirable: bool },
    OneProjection,
    OneEventStream,
    EntireDb,
    SecureStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorruptionResponse {
    pub severity: RecoverySeverity,
    /// Whether trusted communication may continue.
    pub trusted_comms_allowed: bool,
    pub action: CorruptionAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptionAction {
    /// §53: mark corrupt, remove final trust, redownload.
    MarkCorruptAndReacquire,
    /// Blob can't be re-fetched: stays marked corrupt and isolated;
    /// the rest of the application is unaffected.
    MarkCorruptIsolate,
    /// §42: derived data — drop and rebuild.
    DropAndRebuild,
    /// §55: quarantine the stream, try backup/peer restore.
    QuarantineAndRestore,
    /// Whole database: stop, restore from backup (§57) or repair.
    HaltAndRestoreFromBackup,
    /// §56: keys unavailable.
    DisableNetworkIdentity,
}

/// §52's five-way classification → outcome.
///
/// Note on §49 vs §56: §49 lists "identity key store inaccessible" as
/// a *fatal* example while §56 says local data can still be viewed.
/// Both are honoured: the severity is `ReadOnly` (local viewing
/// allowed, no durable writes, no network) with trusted communication
/// explicitly disallowed. If the database is *also* unreadable the
/// caller records `EntireDb` too and the accumulated worst severity
/// becomes `Fatal`.
pub const fn response_for(scope: CorruptionScope) -> CorruptionResponse {
    use CorruptionAction::*;
    match scope {
        CorruptionScope::SingleBlob { reacquirable: true } => CorruptionResponse {
            severity: RecoverySeverity::Repaired,
            trusted_comms_allowed: true,
            action: MarkCorruptAndReacquire,
        },
        CorruptionScope::SingleBlob {
            reacquirable: false,
        } => CorruptionResponse {
            severity: RecoverySeverity::Degraded,
            trusted_comms_allowed: true,
            action: MarkCorruptIsolate,
        },
        CorruptionScope::OneProjection => CorruptionResponse {
            severity: RecoverySeverity::Repaired,
            trusted_comms_allowed: true,
            action: DropAndRebuild,
        },
        CorruptionScope::OneEventStream => CorruptionResponse {
            severity: RecoverySeverity::ReadOnly,
            trusted_comms_allowed: false,
            action: QuarantineAndRestore,
        },
        CorruptionScope::EntireDb => CorruptionResponse {
            severity: RecoverySeverity::Fatal,
            trusted_comms_allowed: false,
            action: HaltAndRestoreFromBackup,
        },
        CorruptionScope::SecureStore => CorruptionResponse {
            severity: RecoverySeverity::ReadOnly,
            trusted_comms_allowed: false,
            action: DisableNetworkIdentity,
        },
    }
}

// ---------------------------------------------------------------------
// §53
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobTrust {
    Trusted,
    /// Final trust removed; must not be served or opened as valid.
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobCorruptionPlan {
    pub new_trust: BlobTrust,
    pub redownload: bool,
}

pub const fn plan_blob_corruption(reacquirable: bool) -> BlobCorruptionPlan {
    BlobCorruptionPlan {
        new_trust: BlobTrust::Corrupt,
        redownload: reacquirable,
    }
}

// ---------------------------------------------------------------------
// §54
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkVerification {
    Verified,
    Mismatch,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRebuild {
    /// Chunk indexes proven good by re-verification.
    pub verified: Vec<u32>,
    /// Chunk indexes that must be fetched again.
    pub to_fetch: Vec<u32>,
}

/// §54: with the journal lost, progress is rebuilt purely from what
/// can be verified on disk — slower, but never optimistic. Only
/// `Verified` chunks count as progress.
pub fn rebuild_journal_progress(chunks: &[ChunkVerification]) -> JournalRebuild {
    let mut r = JournalRebuild {
        verified: Vec::new(),
        to_fetch: Vec::new(),
    };
    for (i, c) in chunks.iter().enumerate() {
        let i = i as u32;
        match c {
            ChunkVerification::Verified => r.verified.push(i),
            ChunkVerification::Mismatch | ChunkVerification::Missing => r.to_fetch.push(i),
        }
    }
    r
}

// ---------------------------------------------------------------------
// §55
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("event stream gap: expected sequence {expected}, found {found}")]
pub struct SequenceGap {
    pub expected: u64,
    pub found: u64,
}

/// §55: "do not silently skip events." Verifies a stream's sequence
/// numbers are contiguous starting at `first`; the first hole is an
/// error the caller must quarantine on, never skip past.
pub fn verify_contiguous(
    first: u64,
    seqs: impl IntoIterator<Item = u64>,
) -> Result<u64, SequenceGap> {
    let mut expected = first;
    for s in seqs {
        if s != expected {
            return Err(SequenceGap { expected, found: s });
        }
        expected += 1;
    }
    Ok(expected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreSource {
    Backup,
    Peer,
}

/// Restore sources to try, backup first (local, authenticated) then
/// peers. Empty means the stream stays quarantined.
pub fn restore_sources(backup_available: bool, peer_reachable: bool) -> Vec<RestoreSource> {
    let mut v = Vec::with_capacity(2);
    if backup_available {
        v.push(RestoreSource::Backup);
    }
    if peer_reachable {
        v.push(RestoreSource::Peer);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_domain_has_its_own_mechanism() {
        use DataDomain::*;
        use DetectionMechanism::*;
        assert_eq!(mechanism_for(RelationalDb), SqliteIntegrityCheck);
        assert_eq!(mechanism_for(EventRecord), EventChecksum);
        assert_eq!(mechanism_for(BlobContent), BlobHash);
        assert_eq!(mechanism_for(Manifest), ManifestAuthentication);
        assert_eq!(mechanism_for(Snapshot), SnapshotChecksum);
        assert_eq!(mechanism_for(IdentityState), IdentitySignature);
    }

    #[test]
    fn scopes_are_not_all_treated_the_same() {
        let scopes = [
            CorruptionScope::SingleBlob { reacquirable: true },
            CorruptionScope::SingleBlob {
                reacquirable: false,
            },
            CorruptionScope::OneProjection,
            CorruptionScope::OneEventStream,
            CorruptionScope::EntireDb,
            CorruptionScope::SecureStore,
        ];
        let actions: std::collections::HashSet<_> = scopes
            .iter()
            .map(|s| format!("{:?}", response_for(*s).action))
            .collect();
        assert_eq!(actions.len(), scopes.len());
    }

    #[test]
    fn small_corruption_never_poisons_the_whole_app() {
        for s in [
            CorruptionScope::SingleBlob { reacquirable: true },
            CorruptionScope::SingleBlob {
                reacquirable: false,
            },
            CorruptionScope::OneProjection,
        ] {
            let r = response_for(s);
            assert!(r.severity < RecoverySeverity::ReadOnly, "{s:?}");
            assert!(r.trusted_comms_allowed);
        }
    }

    #[test]
    fn authoritative_and_key_failures_stop_trusted_comms() {
        for s in [
            CorruptionScope::OneEventStream,
            CorruptionScope::EntireDb,
            CorruptionScope::SecureStore,
        ] {
            assert!(!response_for(s).trusted_comms_allowed, "{s:?}");
        }
        assert_eq!(
            response_for(CorruptionScope::EntireDb).severity,
            RecoverySeverity::Fatal
        );
    }

    #[test]
    fn secure_store_failure_still_allows_local_viewing() {
        use crate::recovery_mode::{gate, Operation};
        let sev = response_for(CorruptionScope::SecureStore).severity;
        assert!(gate(sev, Operation::ViewMessages).is_ok());
        assert!(gate(sev, Operation::StartNetwork).is_err());
    }

    #[test]
    fn blob_is_always_untrusted_and_redownloaded_only_if_possible() {
        let p = plan_blob_corruption(true);
        assert_eq!((p.new_trust, p.redownload), (BlobTrust::Corrupt, true));
        let p = plan_blob_corruption(false);
        assert_eq!((p.new_trust, p.redownload), (BlobTrust::Corrupt, false));
    }

    #[test]
    fn journal_rebuild_counts_only_verified_chunks() {
        use ChunkVerification::*;
        let r = rebuild_journal_progress(&[Verified, Mismatch, Verified, Missing]);
        assert_eq!(r.verified, vec![0, 2]);
        assert_eq!(r.to_fetch, vec![1, 3]);
        assert!(rebuild_journal_progress(&[]).verified.is_empty());
    }

    #[test]
    fn event_stream_gap_is_an_error_not_a_skip() {
        assert_eq!(verify_contiguous(1, [1, 2, 3]), Ok(4));
        assert_eq!(
            verify_contiguous(1, [1, 2, 4]),
            Err(SequenceGap {
                expected: 3,
                found: 4
            })
        );
        assert_eq!(
            verify_contiguous(5, [4]),
            Err(SequenceGap {
                expected: 5,
                found: 4
            })
        );
        assert_eq!(verify_contiguous(1, []), Ok(1));
    }

    #[test]
    fn restore_prefers_local_backup_then_peers() {
        assert_eq!(
            restore_sources(true, true),
            vec![RestoreSource::Backup, RestoreSource::Peer]
        );
        assert!(restore_sources(false, false).is_empty());
    }
}
