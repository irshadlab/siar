//! §147 "Recovery Security", §148 "Rollback Attack Recovery", §149
//! "Secure Monotonic Anchor", §150 "Recovery and Encryption Keys".
//!
//! Repair paths are security-sensitive: a peer "helping" with repair
//! is also the easiest way to inject state. Everything here fails
//! closed and reports *why* in a fixed order.

use crate::recovery_mode::RecoverySeverity;

// ---------------------------------------------------------------------
// §147
// ---------------------------------------------------------------------

/// What recovery has established about a peer-supplied repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairOffer {
    /// The peer is who it claims to be (Part 02 identity check).
    pub identity_verified: bool,
    /// That identity may supply *this kind* of repair.
    pub authorized_for_this_repair: bool,
    /// The supplied data matches its hashes/signatures.
    pub integrity_verified: bool,
    /// Format/schema version is one we understand and accept.
    pub version_ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RepairRejection {
    #[error("peer identity not verified")]
    Identity,
    #[error("peer not authorized to supply this repair")]
    Authorization,
    #[error("repair data failed integrity verification")]
    Integrity,
    #[error("repair data version not acceptable")]
    Version,
}

/// All four of §147's conditions are required; the first one missing
/// (in the spec's order) is the reason reported.
pub const fn accept_repair(o: &RepairOffer) -> Result<(), RepairRejection> {
    if !o.identity_verified {
        Err(RepairRejection::Identity)
    } else if !o.authorized_for_this_repair {
        Err(RepairRejection::Authorization)
    } else if !o.integrity_verified {
        Err(RepairRejection::Integrity)
    } else if !o.version_ok {
        Err(RepairRejection::Version)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------
// §148
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackVerdict {
    Consistent,
    /// The local database is older than something we have trusted
    /// evidence of — a restored old backup, or lost writes.
    StaleState {
        local: u64,
        newest_known: u64,
    },
}

impl RollbackVerdict {
    pub const fn allows_trusted_operations(&self) -> bool {
        matches!(self, RollbackVerdict::Consistent)
    }

    /// Severity to feed the recovery accumulator: stale identity state
    /// fails closed for trusted operations.
    pub const fn severity(&self) -> RecoverySeverity {
        match self {
            RollbackVerdict::Consistent => RecoverySeverity::Clean,
            RollbackVerdict::StaleState { .. } => RecoverySeverity::ReadOnly,
        }
    }
}

/// Compares the local account generation with every independent
/// witness available: the secure anchor and anything newer a trusted
/// peer has shown. Either alone is enough to detect a rollback.
pub fn detect_rollback(
    local_generation: u64,
    secure_anchor: Option<u64>,
    newest_trusted_peer_generation: Option<u64>,
) -> RollbackVerdict {
    let newest = secure_anchor
        .into_iter()
        .chain(newest_trusted_peer_generation)
        .max();
    match newest {
        Some(n) if n > local_generation => RollbackVerdict::StaleState {
            local: local_generation,
            newest_known: n,
        },
        _ => RollbackVerdict::Consistent,
    }
}

// ---------------------------------------------------------------------
// §149
// ---------------------------------------------------------------------

/// Platform secure storage holding one integer. Implemented per
/// platform (Keystore, Keychain, TPM…); optional.
pub trait AnchorStorage {
    fn load(&self) -> Option<u64>;
    fn store(&mut self, value: u64);
}

/// Wraps an [`AnchorStorage`] so the stored value can only go **up**:
/// an attacker who can restore an old database but not rewrite secure
/// storage cannot make the anchor fall back. Generic, so no dynamic
/// dispatch.
#[derive(Debug)]
pub struct MonotonicAnchor<S: AnchorStorage> {
    storage: S,
}

impl<S: AnchorStorage> MonotonicAnchor<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }

    pub fn current(&self) -> Option<u64> {
        self.storage.load()
    }

    /// Records `generation` if it is higher than what is stored.
    /// Returns whether the anchor moved.
    pub fn raise(&mut self, generation: u64) -> bool {
        match self.storage.load() {
            Some(cur) if cur >= generation => false,
            _ => {
                self.storage.store(generation);
                true
            }
        }
    }

    pub fn check(&self, local_generation: u64, peer: Option<u64>) -> RollbackVerdict {
        detect_rollback(local_generation, self.current(), peer)
    }
}

/// In-memory implementation for tests and platforms without secure
/// storage.
#[derive(Debug, Default)]
pub struct MemoryAnchor(Option<u64>);

impl AnchorStorage for MemoryAnchor {
    fn load(&self) -> Option<u64> {
        self.0
    }
    fn store(&mut self, value: u64) {
        self.0 = Some(value);
    }
}

// ---------------------------------------------------------------------
// §150
// ---------------------------------------------------------------------

/// "Data exists" and "keys exist" are separate facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectFacts {
    pub data_present: bool,
    pub key_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectUsability {
    Usable,
    /// Ciphertext is there, key is not: not usable.
    EncryptedKeyMissing,
    DataMissing,
}

pub const fn usability(o: ObjectFacts) -> ObjectUsability {
    match (o.data_present, o.key_present) {
        (true, true) => ObjectUsability::Usable,
        (true, false) => ObjectUsability::EncryptedKeyMissing,
        (false, _) => ObjectUsability::DataMissing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyCompleteness {
    pub usable: usize,
    pub key_missing: usize,
    pub data_missing: usize,
}

impl KeyCompleteness {
    /// §150: never mark fully recovered while cryptographic material
    /// is missing for data that exists.
    pub const fn fully_recovered(&self) -> bool {
        self.key_missing == 0
    }
}

pub fn assess_keys(objects: &[ObjectFacts]) -> KeyCompleteness {
    let mut c = KeyCompleteness::default();
    for o in objects {
        match usability(*o) {
            ObjectUsability::Usable => c.usable += 1,
            ObjectUsability::EncryptedKeyMissing => c.key_missing += 1,
            ObjectUsability::DataMissing => c.data_missing += 1,
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(i: bool, a: bool, g: bool, v: bool) -> RepairOffer {
        RepairOffer {
            identity_verified: i,
            authorized_for_this_repair: a,
            integrity_verified: g,
            version_ok: v,
        }
    }

    #[test]
    fn repair_needs_all_four_conditions() {
        assert_eq!(accept_repair(&offer(true, true, true, true)), Ok(()));
        assert_eq!(
            accept_repair(&offer(false, true, true, true)),
            Err(RepairRejection::Identity)
        );
        assert_eq!(
            accept_repair(&offer(true, false, true, true)),
            Err(RepairRejection::Authorization)
        );
        assert_eq!(
            accept_repair(&offer(true, true, false, true)),
            Err(RepairRejection::Integrity)
        );
        assert_eq!(
            accept_repair(&offer(true, true, true, false)),
            Err(RepairRejection::Version)
        );
    }

    #[test]
    fn rejection_reports_the_first_failure_in_spec_order() {
        assert_eq!(
            accept_repair(&offer(false, false, false, false)),
            Err(RepairRejection::Identity)
        );
        assert_eq!(
            accept_repair(&offer(true, false, false, false)),
            Err(RepairRejection::Authorization)
        );
        assert_eq!(
            accept_repair(&offer(true, true, false, false)),
            Err(RepairRejection::Integrity)
        );
    }

    #[test]
    fn no_witness_means_no_rollback_claim() {
        assert_eq!(detect_rollback(5, None, None), RollbackVerdict::Consistent);
    }

    #[test]
    fn either_witness_alone_exposes_a_restored_old_backup() {
        assert_eq!(
            detect_rollback(5, Some(9), None),
            RollbackVerdict::StaleState {
                local: 5,
                newest_known: 9
            }
        );
        assert_eq!(
            detect_rollback(5, None, Some(7)),
            RollbackVerdict::StaleState {
                local: 5,
                newest_known: 7
            }
        );
        assert_eq!(
            detect_rollback(5, Some(6), Some(8)),
            RollbackVerdict::StaleState {
                local: 5,
                newest_known: 8
            }
        );
    }

    #[test]
    fn equal_or_newer_local_state_is_consistent() {
        assert_eq!(
            detect_rollback(9, Some(9), Some(4)),
            RollbackVerdict::Consistent
        );
        assert_eq!(
            detect_rollback(12, Some(9), Some(10)),
            RollbackVerdict::Consistent
        );
    }

    #[test]
    fn stale_state_blocks_trusted_ops_and_maps_to_readonly() {
        let v = detect_rollback(1, Some(2), None);
        assert!(!v.allows_trusted_operations());
        assert_eq!(v.severity(), RecoverySeverity::ReadOnly);
        assert!(RollbackVerdict::Consistent.allows_trusted_operations());
    }

    #[test]
    fn the_anchor_only_ever_goes_up() {
        let mut a = MonotonicAnchor::new(MemoryAnchor::default());
        assert_eq!(a.current(), None);
        assert!(a.raise(5));
        assert!(!a.raise(3), "lower value ignored");
        assert!(!a.raise(5), "equal value is not a move");
        assert!(a.raise(8));
        assert_eq!(a.current(), Some(8));
    }

    #[test]
    fn anchor_catches_a_rolled_back_database() {
        let mut a = MonotonicAnchor::new(MemoryAnchor::default());
        a.raise(41);
        // attacker restores a DB from generation 30
        assert!(!a.check(30, None).allows_trusted_operations());
        assert!(a.check(41, None).allows_trusted_operations());
    }

    #[test]
    fn absent_anchor_degrades_to_peer_evidence_only() {
        let a = MonotonicAnchor::new(MemoryAnchor::default());
        assert!(a.check(3, None).allows_trusted_operations());
        assert!(!a.check(3, Some(4)).allows_trusted_operations());
    }

    #[test]
    fn encrypted_data_without_a_key_is_not_usable() {
        let f = |d, k| {
            usability(ObjectFacts {
                data_present: d,
                key_present: k,
            })
        };
        assert_eq!(f(true, true), ObjectUsability::Usable);
        assert_eq!(f(true, false), ObjectUsability::EncryptedKeyMissing);
        assert_eq!(f(false, true), ObjectUsability::DataMissing);
        assert_eq!(f(false, false), ObjectUsability::DataMissing);
    }

    #[test]
    fn missing_key_material_prevents_fully_recovered() {
        let objs = [
            ObjectFacts {
                data_present: true,
                key_present: true,
            },
            ObjectFacts {
                data_present: true,
                key_present: false,
            },
            ObjectFacts {
                data_present: false,
                key_present: false,
            },
        ];
        let c = assess_keys(&objs);
        assert_eq!((c.usable, c.key_missing, c.data_missing), (1, 1, 1));
        assert!(!c.fully_recovered());
        assert!(assess_keys(&objs[..1]).fully_recovered());
        assert!(assess_keys(&[]).fully_recovered());
    }

    #[test]
    fn missing_data_alone_does_not_count_as_a_key_problem() {
        let c = assess_keys(&[ObjectFacts {
            data_present: false,
            key_present: false,
        }]);
        assert!(
            c.fully_recovered(),
            "key completeness is about keys; data gaps are §146's concern"
        );
    }
}
