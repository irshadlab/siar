//! §136-138 (and §78): a multi-device capability view for one account.
//! Device selection and fan-out are deterministic — ties break on the
//! device key, never on hash-map iteration order.

use std::collections::BTreeMap;

use crate::id::CapabilityId;
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceKey(pub [u8; 16]);

/// A capability a device must offer, optionally at a minimum version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Need {
    pub id: CapabilityId,
    pub min_version: Option<CapabilityVersion>,
}

pub const MAX_DEVICES_PER_ACCOUNT: usize = 64;

#[derive(Debug, Default)]
pub struct AccountCapabilityView {
    devices: BTreeMap<DeviceKey, CapabilitySet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("account already tracks the maximum of {MAX_DEVICES_PER_ACCOUNT} devices")]
pub struct TooManyDevices;

impl AccountCapabilityView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_device(
        &mut self,
        key: DeviceKey,
        caps: CapabilitySet,
    ) -> Result<(), TooManyDevices> {
        if !self.devices.contains_key(&key) && self.devices.len() >= MAX_DEVICES_PER_ACCOUNT {
            return Err(TooManyDevices);
        }
        self.devices.insert(key, caps);
        Ok(())
    }

    pub fn remove_device(&mut self, key: &DeviceKey) -> bool {
        self.devices.remove(key).is_some()
    }

    fn satisfies(caps: &CapabilitySet, need: &Need) -> bool {
        caps.get(&need.id)
            .is_some_and(|d| need.min_version.is_none_or(|m| d.version >= m))
    }

    /// §137: devices meeting *every* need, in key order.
    pub fn devices_meeting(&self, needs: &[Need]) -> Vec<DeviceKey> {
        self.devices
            .iter()
            .filter(|(_, c)| needs.iter().all(|n| Self::satisfies(c, n)))
            .map(|(k, _)| *k)
            .collect()
    }

    /// §137: best single device — most capabilities overall, then lowest key.
    pub fn select_device(&self, needs: &[Need]) -> Option<DeviceKey> {
        self.devices
            .iter()
            .filter(|(_, c)| needs.iter().all(|n| Self::satisfies(c, n)))
            .max_by(|(ka, a), (kb, b)| a.len().cmp(&b.len()).then(kb.cmp(ka)))
            .map(|(k, _)| *k)
    }

    /// §138: fan-out targets — every device meeting the need (one
    /// message → all capable devices), in key order.
    pub fn fan_out_targets(&self, need: &Need) -> Vec<DeviceKey> {
        self.devices_meeting(std::slice::from_ref(need))
    }

    /// §78: highest version of each capability any device offers.
    pub fn account_union(&self) -> BTreeMap<CapabilityId, CapabilityVersion> {
        let mut out = BTreeMap::new();
        for c in self.devices.values() {
            for d in c.iter() {
                out.entry(d.id)
                    .and_modify(|v: &mut CapabilityVersion| *v = (*v).max(d.version))
                    .or_insert(d.version);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityNamespace;

    fn cap(code: u32) -> CapabilityId {
        CapabilityId::new(CapabilityNamespace::Files, code)
    }

    fn caps(list: &[(u32, u16)]) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        for &(c, v) in list {
            s.insert(CapabilityDescriptor::new(
                cap(c),
                CapabilityVersion::new(v, 0),
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        }
        s
    }

    fn view() -> AccountCapabilityView {
        let mut v = AccountCapabilityView::new();
        v.set_device(DeviceKey([3; 16]), caps(&[(1, 1)])).unwrap();
        v.set_device(DeviceKey([1; 16]), caps(&[(1, 2), (2, 1)]))
            .unwrap();
        v.set_device(DeviceKey([2; 16]), caps(&[(1, 2), (2, 1)]))
            .unwrap();
        v
    }

    #[test]
    fn selection_prefers_most_capable_then_lowest_key() {
        let n = Need {
            id: cap(1),
            min_version: None,
        };
        assert_eq!(view().select_device(&[n]), Some(DeviceKey([1; 16])));
    }

    #[test]
    fn min_version_and_all_needs_are_enforced() {
        let v = view();
        let n = Need {
            id: cap(1),
            min_version: Some(CapabilityVersion::new(2, 0)),
        };
        assert_eq!(v.devices_meeting(&[n]).len(), 2);
        let both = [
            n,
            Need {
                id: cap(2),
                min_version: None,
            },
        ];
        assert_eq!(
            v.devices_meeting(&both),
            vec![DeviceKey([1; 16]), DeviceKey([2; 16])]
        );
        assert!(v
            .select_device(&[Need {
                id: cap(9),
                min_version: None
            }])
            .is_none());
    }

    #[test]
    fn fan_out_and_union() {
        let v = view();
        assert_eq!(
            v.fan_out_targets(&Need {
                id: cap(2),
                min_version: None
            })
            .len(),
            2
        );
        let u = v.account_union();
        assert_eq!(u[&cap(1)], CapabilityVersion::new(2, 0));
        assert_eq!(u.len(), 2);
    }

    #[test]
    fn device_count_is_bounded() {
        let mut v = AccountCapabilityView::new();
        for i in 0..MAX_DEVICES_PER_ACCOUNT as u8 {
            v.set_device(DeviceKey([i; 16]), CapabilitySet::new())
                .unwrap();
        }
        assert_eq!(
            v.set_device(DeviceKey([200; 16]), CapabilitySet::new()),
            Err(TooManyDevices)
        );
        assert!(
            v.set_device(DeviceKey([0; 16]), CapabilitySet::new())
                .is_ok(),
            "update of existing still allowed"
        );
    }
}
