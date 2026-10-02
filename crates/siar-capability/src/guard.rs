//! §54-55 per-operation requirements, §113 operation guard, §114
//! fallback suggestions.
//!
//! Extension presence never implies every feature (§54): each operation
//! lists the exact capabilities (with minimum versions) it needs, and the
//! guard checks the *negotiated* set. Fail-closed: an operation nobody
//! registered is denied, not allowed.

use std::collections::BTreeMap;

use crate::id::CapabilityId;
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub u32);

/// §114.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackSuggestion {
    /// Do a degraded version of the same thing (e.g. send edit as a new message).
    UseOperation(OperationId),
    /// Keep it queued; retry when the peer's capabilities change (§52).
    QueueUntilCapable,
    /// Nothing sensible: disable the control (§53, §56).
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRequirement {
    pub needs: Vec<(CapabilityId, CapabilityVersion)>,
    pub fallback: FallbackSuggestion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    Allowed,
    Denied {
        missing: Vec<CapabilityId>,
        fallback: FallbackSuggestion,
    },
    /// No requirement registered for this operation. Treated as a denial.
    UnknownOperation,
}

impl GuardVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}

#[derive(Debug, Default)]
pub struct OperationGuard {
    ops: BTreeMap<OperationId, OperationRequirement>,
}

impl OperationGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, op: OperationId, req: OperationRequirement) {
        self.ops.insert(op, req);
    }

    /// Same major, and at least the required minor.
    fn satisfied(negotiated: &CapabilitySet, id: &CapabilityId, min: CapabilityVersion) -> bool {
        negotiated
            .get(id)
            .is_some_and(|d| d.version.major == min.major && d.version.minor >= min.minor)
    }

    pub fn check(&self, op: OperationId, negotiated: &CapabilitySet) -> GuardVerdict {
        let Some(req) = self.ops.get(&op) else {
            return GuardVerdict::UnknownOperation;
        };
        let missing: Vec<CapabilityId> = req
            .needs
            .iter()
            .filter(|(id, min)| !Self::satisfied(negotiated, id, *min))
            .map(|(id, _)| *id)
            .collect();
        if missing.is_empty() {
            GuardVerdict::Allowed
        } else {
            GuardVerdict::Denied {
                missing,
                fallback: req.fallback,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityNamespace;

    const EDIT: OperationId = OperationId(1);
    const SEND: OperationId = OperationId(2);

    fn cap(code: u32) -> CapabilityId {
        CapabilityId::new(CapabilityNamespace::Messaging, code)
    }

    fn set(entries: &[(u32, u16, u16)]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for &(c, maj, min) in entries {
            s.insert(CapabilityDescriptor::new(
                cap(c),
                CapabilityVersion::new(maj, min),
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        }
        s
    }

    fn guard() -> OperationGuard {
        let mut g = OperationGuard::new();
        g.register(
            EDIT,
            OperationRequirement {
                needs: vec![(cap(10), CapabilityVersion::new(1, 2))],
                fallback: FallbackSuggestion::UseOperation(SEND),
            },
        );
        g
    }

    #[test]
    fn allowed_when_version_is_sufficient() {
        assert!(guard().check(EDIT, &set(&[(10, 1, 2)])).is_allowed());
        assert!(guard().check(EDIT, &set(&[(10, 1, 9)])).is_allowed());
    }

    #[test]
    fn denied_with_fallback_when_missing_or_too_old_or_wrong_major() {
        for s in [set(&[]), set(&[(10, 1, 1)]), set(&[(10, 2, 5)])] {
            assert_eq!(
                guard().check(EDIT, &s),
                GuardVerdict::Denied {
                    missing: vec![cap(10)],
                    fallback: FallbackSuggestion::UseOperation(SEND)
                }
            );
        }
    }

    #[test]
    fn unregistered_operation_fails_closed() {
        let v = guard().check(OperationId(99), &set(&[(10, 1, 2)]));
        assert_eq!(v, GuardVerdict::UnknownOperation);
        assert!(!v.is_allowed());
    }
}
