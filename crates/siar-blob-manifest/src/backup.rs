//! §158-161: backup and restore, as policy and verification — not as a
//! transport (§161's second device, §162's object store are just more
//! [`crate::source_trust`]-authorized blob sources behind
//! [`crate::blob_io::BlobStore`]).
//!
//! * §158 [`BackupPolicy`] decides *what* is eligible. The relay cache is
//!   excluded by default: it is other people's ciphertext in transit, not
//!   this user's data.
//! * §159 [`plan_backup`] is incremental because blobs are content
//!   addressed: only [`BlobId`]s the backup does not already hold are
//!   planned.
//! * §160 [`restore_entry`] never trusts backup bytes: the hash and size
//!   are checked against the id first, and references are rebuilt only
//!   from a blob that passed.

use std::collections::HashSet;

use thiserror::Error;

use crate::app_policy::BlobStorageClass;
use crate::ids::BlobId;
use crate::reference_store::{BlobReference, RefOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupPolicy {
    /// Everything in the store, referenced or not (a full disk image).
    pub all_blobs: bool,
    /// Blobs with a live reference in a durable class (the normal choice).
    pub referenced_durable: bool,
    pub thumbnails: bool,
    /// Normally `false`.
    pub relay_cache: bool,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self {
            all_blobs: false,
            referenced_durable: true,
            thumbnails: false,
            relay_cache: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupCandidate {
    pub blob_id: BlobId,
    pub size: u64,
    pub class: BlobStorageClass,
    pub referenced: bool,
    pub is_thumbnail: bool,
}

impl BackupPolicy {
    pub fn includes(&self, c: &BackupCandidate) -> bool {
        if c.class == BlobStorageClass::Relay && !self.relay_cache {
            // Even `all_blobs` leaves other people's relay ciphertext out
            // unless it is asked for by name.
            return false;
        }
        if self.all_blobs {
            return true;
        }
        if c.is_thumbnail {
            return self.thumbnails;
        }
        self.referenced_durable && c.referenced && c.class == BlobStorageClass::Durable
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackupPlan {
    pub upload: Vec<BackupCandidate>,
    pub already_backed_up: usize,
    pub excluded_by_policy: usize,
    pub bytes_to_upload: u64,
}

/// §159: what must be copied, given what the backup already holds.
pub fn plan_backup(
    candidates: &[BackupCandidate],
    policy: &BackupPolicy,
    already_in_backup: &HashSet<BlobId>,
) -> BackupPlan {
    let mut plan = BackupPlan::default();
    let mut seen = HashSet::new();
    for c in candidates {
        if !policy.includes(c) {
            plan.excluded_by_policy += 1;
        } else if already_in_backup.contains(&c.blob_id) || !seen.insert(c.blob_id) {
            plan.already_backed_up += 1;
        } else {
            plan.bytes_to_upload = plan.bytes_to_upload.saturating_add(c.size);
            plan.upload.push(*c);
        }
    }
    plan
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RestoreError {
    #[error("restored blob is {actual} bytes, expected {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("restored bytes do not hash to the blob id")]
    HashMismatch,
}

/// A backed-up blob and the references that must be rebuilt for it.
#[derive(Debug, Clone)]
pub struct RestoreEntry {
    pub blob_id: BlobId,
    pub size: u64,
    pub references: Vec<BlobReference>,
}

/// §160: bytes → verify → references. Returns the reference operations
/// to apply *only if* the bytes are exactly the blob they claim to be.
pub fn restore_entry(entry: &RestoreEntry, bytes: &[u8]) -> Result<Vec<RefOp>, RestoreError> {
    if bytes.len() as u64 != entry.size {
        return Err(RestoreError::SizeMismatch {
            expected: entry.size,
            actual: bytes.len() as u64,
        });
    }
    if BlobId::from_ciphertext(bytes) != entry.blob_id {
        return Err(RestoreError::HashMismatch);
    }
    Ok(entry.references.iter().cloned().map(RefOp::Add).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_store::{BlobRole, ObjectId};
    use siar_event_log::namespace::NamespaceId;

    fn cand(n: u8, class: BlobStorageClass, referenced: bool, thumb: bool) -> BackupCandidate {
        BackupCandidate {
            blob_id: BlobId::from_ciphertext(&[n]),
            size: 100,
            class,
            referenced,
            is_thumbnail: thumb,
        }
    }

    #[test]
    fn the_default_policy_takes_referenced_durable_blobs_only() {
        use BlobStorageClass::*;
        let p = BackupPolicy::default();
        assert!(p.includes(&cand(1, Durable, true, false)));
        assert!(!p.includes(&cand(2, Durable, false, false)), "unreferenced");
        assert!(!p.includes(&cand(3, Cached, true, false)), "cache");
        assert!(!p.includes(&cand(4, Temporary, true, false)));
        assert!(!p.includes(&cand(5, Relay, true, false)), "relay excluded");
        assert!(!p.includes(&cand(6, Durable, true, true)), "thumbnails off");
    }

    #[test]
    fn relay_cache_needs_to_be_asked_for_even_under_all_blobs() {
        use BlobStorageClass::*;
        let all = BackupPolicy {
            all_blobs: true,
            ..BackupPolicy::default()
        };
        assert!(all.includes(&cand(1, Cached, false, false)));
        assert!(!all.includes(&cand(2, Relay, false, false)));
        let with_relay = BackupPolicy {
            relay_cache: true,
            ..all
        };
        assert!(with_relay.includes(&cand(2, Relay, false, false)));
    }

    #[test]
    fn thumbnails_follow_their_own_switch() {
        use BlobStorageClass::*;
        let p = BackupPolicy {
            thumbnails: true,
            ..BackupPolicy::default()
        };
        assert!(p.includes(&cand(1, Cached, false, true)));
    }

    #[test]
    fn an_incremental_plan_uploads_only_what_the_backup_lacks() {
        use BlobStorageClass::*;
        let cs = vec![
            cand(1, Durable, true, false),
            cand(2, Durable, true, false),
            cand(2, Durable, true, false), // the same blob referenced twice
            cand(3, Durable, true, false),
            cand(4, Relay, true, false),
        ];
        let have: HashSet<BlobId> = [BlobId::from_ciphertext(&[1])].into();
        let plan = plan_backup(&cs, &BackupPolicy::default(), &have);
        assert_eq!(plan.upload.len(), 2);
        assert_eq!(plan.bytes_to_upload, 200);
        assert_eq!(plan.already_backed_up, 2, "one held, one duplicate");
        assert_eq!(plan.excluded_by_policy, 1);
        // A second run with everything present plans nothing.
        let all: HashSet<BlobId> = cs.iter().map(|c| c.blob_id).collect();
        assert!(plan_backup(&cs, &BackupPolicy::default(), &all)
            .upload
            .is_empty());
    }

    fn entry(bytes: &[u8]) -> RestoreEntry {
        RestoreEntry {
            blob_id: BlobId::from_ciphertext(bytes),
            size: bytes.len() as u64,
            references: vec![BlobReference {
                owner_namespace: NamespaceId::new("personal").unwrap(),
                object_id: ObjectId::new("msg-1").unwrap(),
                blob_id: BlobId::from_ciphertext(bytes),
                role: BlobRole::MessageAttachment,
            }],
        }
    }

    #[test]
    fn restore_rebuilds_references_only_for_verified_bytes() {
        let good = b"the real ciphertext".to_vec();
        let e = entry(&good);
        let ops = restore_entry(&e, &good).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], RefOp::Add(_)));
    }

    #[test]
    fn restore_never_trusts_backup_bytes() {
        let good = b"the real ciphertext".to_vec();
        let e = entry(&good);
        let mut flipped = good.clone();
        flipped[0] ^= 1;
        assert_eq!(restore_entry(&e, &flipped), Err(RestoreError::HashMismatch));
        assert_eq!(
            restore_entry(&e, &good[..5]),
            Err(RestoreError::SizeMismatch {
                expected: good.len() as u64,
                actual: 5
            })
        );
    }
}
