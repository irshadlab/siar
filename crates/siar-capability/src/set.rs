//! §27 "Canonical Encoding" and §61 "Capability Advertisement Size".

use crate::descriptor::CapabilityDescriptor;
use crate::id::CapabilityId;

/// §61: "Bound: max capabilities, max parameters, max bytes. A peer
/// must not force huge negotiation payloads." This crate owns only the
/// count bound (`max capabilities`); the parameter/byte bounds are
/// [`crate::descriptor::MAX_PARAMETER_BYTES`] and the future wire
/// advertisement's own size budget respectively. 512 is a deliberately
/// generous ceiling — no real deployment described anywhere in Parts
/// 01-07 approaches it — chosen only to make "unbounded" structurally
/// impossible, not tuned against any measured payload.
pub const MAX_CAPABILITIES_PER_SET: usize = 512;

/// §27: "Capability ordering must be canonical before hashing... Do
/// not hash arbitrary HashMap iteration order." A [`CapabilitySet`] is
/// therefore not a `HashMap` — it's a `Vec` that [`CapabilitySet::insert`]
/// keeps sorted by `(namespace, code, version)` (via [`CapabilityId`]'s
/// and [`crate::version::CapabilityVersion`]'s derived `Ord`) at every
/// mutation, so iteration order is always the canonical order and
/// [`crate::hash::CapabilitySetHash::of`] never needs a separate sort
/// pass that could be forgotten at a call site.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    descriptors: Vec<CapabilityDescriptor>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts, replacing any existing descriptor with the same
    /// [`CapabilityId`] (a set has at most one descriptor per
    /// capability — re-advertising the same id updates it rather than
    /// creating a duplicate entry that canonical ordering would then
    /// need a tie-break rule for).
    pub fn insert(&mut self, descriptor: CapabilityDescriptor) -> Result<(), CapabilitySetError> {
        // Sorted by id and unique per id, so binary search is exact and
        // its `Err` position is the canonical insertion point.
        let pos = match self
            .descriptors
            .binary_search_by_key(&descriptor.id, |d| d.id)
        {
            Ok(i) => {
                self.descriptors[i] = descriptor;
                return Ok(());
            }
            Err(pos) => pos,
        };

        if self.descriptors.len() >= MAX_CAPABILITIES_PER_SET {
            return Err(CapabilitySetError::TooManyCapabilities {
                max: MAX_CAPABILITIES_PER_SET,
            });
        }

        self.descriptors.insert(pos, descriptor);
        Ok(())
    }

    pub fn get(&self, id: &CapabilityId) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .binary_search_by_key(id, |d| d.id)
            .ok()
            .map(|i| &self.descriptors[i])
    }

    /// Removes the descriptor for `id`, returning it if present.
    pub fn remove(&mut self, id: &CapabilityId) -> Option<CapabilityDescriptor> {
        self.descriptors
            .binary_search_by_key(id, |d| d.id)
            .ok()
            .map(|i| self.descriptors.remove(i))
    }

    pub fn contains(&self, id: &CapabilityId) -> bool {
        self.get(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// Always yields descriptors in canonical order (§27).
    pub fn iter(&self) -> impl Iterator<Item = &CapabilityDescriptor> {
        self.descriptors.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilitySetError {
    #[error("capability set exceeds max of {max} capabilities")]
    TooManyCapabilities { max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityParameters, CapabilityRequirement};
    use crate::id::{CapabilityId, CapabilityNamespace};
    use crate::version::CapabilityVersion;

    fn desc(namespace: CapabilityNamespace, code: u32) -> CapabilityDescriptor {
        CapabilityDescriptor::new(
            CapabilityId::new(namespace, code),
            CapabilityVersion::new(1, 0),
            CapabilityRequirement::Optional,
            CapabilityParameters::None,
        )
    }

    #[test]
    fn insert_maintains_canonical_order_regardless_of_insertion_order() {
        let mut set = CapabilitySet::new();
        set.insert(desc(CapabilityNamespace::Files, 5)).unwrap();
        set.insert(desc(CapabilityNamespace::Core, 1)).unwrap();
        set.insert(desc(CapabilityNamespace::Files, 1)).unwrap();

        let ids: Vec<_> = set.iter().map(|d| d.id).collect();
        assert_eq!(
            ids,
            vec![
                CapabilityId::new(CapabilityNamespace::Core, 1),
                CapabilityId::new(CapabilityNamespace::Files, 1),
                CapabilityId::new(CapabilityNamespace::Files, 5),
            ]
        );
    }

    #[test]
    fn insert_replaces_existing_id_instead_of_duplicating() {
        let mut set = CapabilitySet::new();
        set.insert(desc(CapabilityNamespace::Core, 1)).unwrap();
        let updated = CapabilityDescriptor::new(
            CapabilityId::new(CapabilityNamespace::Core, 1),
            CapabilityVersion::new(2, 0),
            CapabilityRequirement::Required,
            CapabilityParameters::None,
        );
        set.insert(updated.clone()).unwrap();

        assert_eq!(set.len(), 1);
        assert_eq!(
            set.get(&CapabilityId::new(CapabilityNamespace::Core, 1)),
            Some(&updated)
        );
    }

    #[test]
    fn insert_rejects_past_max_capacity() {
        let mut set = CapabilitySet::new();
        for code in 0..MAX_CAPABILITIES_PER_SET as u32 {
            set.insert(desc(CapabilityNamespace::Core, code)).unwrap();
        }
        let overflow = set.insert(desc(
            CapabilityNamespace::Core,
            MAX_CAPABILITIES_PER_SET as u32,
        ));
        assert_eq!(
            overflow.unwrap_err(),
            CapabilitySetError::TooManyCapabilities {
                max: MAX_CAPABILITIES_PER_SET
            }
        );
    }
}
