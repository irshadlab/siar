//! §87 Resource Capabilities and §88 Dynamic Resource Limits.
//!
//! Limits are *negotiated ceilings, not promises of capacity* (§87): the
//! effective limit may only ever go down from what was negotiated, in response
//! to storage, battery or memory pressure (§88), and a capability update is
//! worth sending only when the advertised capacity *class* actually changes.
//!
//! The pressure inputs are this crate's own three-level enum so it keeps no
//! dependency on Part 08; a caller maps `siar-resource-limits`' pressure state
//! and `siar-power`'s battery state onto [`Level`].

use serde::{Deserialize, Serialize};

use crate::privacy::ResourceClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_concurrent_transfers: u32,
    pub max_inbound_file_size: u64,
    pub max_relay_bytes: u64,
    pub max_active_streams: u32,
}

impl ResourceLimits {
    /// Field-wise minimum: each side's ceiling binds. Symmetric, so both ends
    /// compute the same result (§72).
    pub fn negotiate(local: Self, remote: Self) -> Self {
        Self {
            max_concurrent_transfers: local
                .max_concurrent_transfers
                .min(remote.max_concurrent_transfers),
            max_inbound_file_size: local
                .max_inbound_file_size
                .min(remote.max_inbound_file_size),
            max_relay_bytes: local.max_relay_bytes.min(remote.max_relay_bytes),
            max_active_streams: local.max_active_streams.min(remote.max_active_streams),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    #[default]
    Normal,
    Elevated,
    Critical,
}

impl Level {
    /// Elevated halves, Critical keeps an eighth.
    const fn shift(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Elevated => 1,
            Self::Critical => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimePressure {
    pub storage: Level,
    pub battery: Level,
    pub memory: Level,
}

/// What each kind of pressure squeezes: storage → bytes held; memory → how much
/// runs at once; battery → background transfer and relay work.
pub fn effective_limits(negotiated: ResourceLimits, p: RuntimePressure) -> ResourceLimits {
    let transfers = p.memory.max(p.battery).shift();
    let streams = p.memory.shift();
    let inbound = p.storage.shift();
    let relay = p.storage.max(p.battery).shift();
    // A non-zero count never drops to zero: slowed, not switched off.
    let count = |v: u32, s: u32| if v == 0 { 0 } else { (v >> s).max(1) };
    ResourceLimits {
        max_concurrent_transfers: count(negotiated.max_concurrent_transfers, transfers),
        max_inbound_file_size: negotiated.max_inbound_file_size >> inbound,
        max_relay_bytes: negotiated.max_relay_bytes >> relay,
        max_active_streams: count(negotiated.max_active_streams, streams),
    }
}

/// §88: "Capability update may advertise changed capacity class" — only when the
/// class changes, so a limit wobbling inside one class sends nothing.
#[derive(Debug, Clone, Copy)]
pub struct CapacityAdvertiser {
    last: ResourceClass,
}

impl CapacityAdvertiser {
    pub fn new(initial: &ResourceLimits) -> Self {
        Self {
            last: Self::class_of(initial),
        }
    }

    fn class_of(l: &ResourceLimits) -> ResourceClass {
        ResourceClass::from_bytes(l.max_inbound_file_size)
    }

    /// `Some(new_class)` exactly when it differs from the last advertised one.
    pub fn update(&mut self, effective: &ResourceLimits) -> Option<ResourceClass> {
        let now = Self::class_of(effective);
        (now != self.last).then(|| {
            self.last = now;
            now
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const BASE: ResourceLimits = ResourceLimits {
        max_concurrent_transfers: 8,
        max_inbound_file_size: 1 << 30,
        max_relay_bytes: 1 << 28,
        max_active_streams: 32,
    };

    fn level() -> impl Strategy<Value = Level> {
        prop_oneof![
            Just(Level::Normal),
            Just(Level::Elevated),
            Just(Level::Critical)
        ]
    }

    fn pressure() -> impl Strategy<Value = RuntimePressure> {
        (level(), level(), level()).prop_map(|(storage, battery, memory)| RuntimePressure {
            storage,
            battery,
            memory,
        })
    }

    #[test]
    fn negotiation_takes_the_smaller_ceiling_on_each_axis_symmetrically() {
        let small = ResourceLimits {
            max_concurrent_transfers: 2,
            max_inbound_file_size: 1 << 40,
            max_relay_bytes: 0,
            max_active_streams: 64,
        };
        let n = ResourceLimits::negotiate(BASE, small);
        assert_eq!(n.max_concurrent_transfers, 2);
        assert_eq!(n.max_inbound_file_size, 1 << 30);
        assert_eq!(n.max_relay_bytes, 0);
        assert_eq!(n.max_active_streams, 32);
        assert_eq!(n, ResourceLimits::negotiate(small, BASE));
    }

    #[test]
    fn no_pressure_leaves_the_negotiated_limits_untouched() {
        assert_eq!(effective_limits(BASE, RuntimePressure::default()), BASE);
    }

    #[test]
    fn each_pressure_squeezes_only_what_it_should() {
        let storage = effective_limits(
            BASE,
            RuntimePressure {
                storage: Level::Elevated,
                ..Default::default()
            },
        );
        assert_eq!(storage.max_inbound_file_size, 1 << 29);
        assert_eq!(storage.max_relay_bytes, 1 << 27);
        assert_eq!(storage.max_active_streams, 32);

        let memory = effective_limits(
            BASE,
            RuntimePressure {
                memory: Level::Critical,
                ..Default::default()
            },
        );
        assert_eq!(memory.max_active_streams, 4);
        assert_eq!(memory.max_concurrent_transfers, 1);
        assert_eq!(memory.max_inbound_file_size, 1 << 30);
    }

    #[test]
    fn counts_are_slowed_never_switched_off_and_zero_stays_zero() {
        let tiny = ResourceLimits {
            max_concurrent_transfers: 1,
            max_active_streams: 2,
            ..BASE
        };
        let p = RuntimePressure {
            memory: Level::Critical,
            battery: Level::Critical,
            storage: Level::Critical,
        };
        let e = effective_limits(tiny, p);
        assert_eq!((e.max_concurrent_transfers, e.max_active_streams), (1, 1));
        let none = ResourceLimits {
            max_concurrent_transfers: 0,
            ..BASE
        };
        assert_eq!(effective_limits(none, p).max_concurrent_transfers, 0);
    }

    #[test]
    fn a_capability_update_is_advertised_only_when_the_class_changes() {
        let mut adv = CapacityAdvertiser::new(&BASE);
        let squeeze = |s| {
            effective_limits(
                BASE,
                RuntimePressure {
                    storage: s,
                    ..Default::default()
                },
            )
        };
        assert_eq!(adv.update(&squeeze(Level::Normal)), None);
        // 1 GiB → 512 MiB: still Large.
        assert_eq!(adv.update(&squeeze(Level::Elevated)), None);
        // 1 GiB → 128 MiB: Medium.
        assert_eq!(
            adv.update(&squeeze(Level::Critical)),
            Some(ResourceClass::Medium)
        );
        assert_eq!(
            adv.update(&squeeze(Level::Critical)),
            None,
            "no repeat for the same class"
        );
        assert_eq!(
            adv.update(&squeeze(Level::Normal)),
            Some(ResourceClass::Large)
        );
    }

    proptest! {
        #[test]
        fn effective_never_exceeds_negotiated_and_worse_pressure_never_raises_a_limit(
            t in 0u32..1000, f in 0u64..u64::MAX, r in 0u64..u64::MAX, s in 0u32..1000,
            a in pressure(), b in pressure(),
        ) {
            let n = ResourceLimits { max_concurrent_transfers: t, max_inbound_file_size: f, max_relay_bytes: r, max_active_streams: s };
            let ea = effective_limits(n, a);
            prop_assert!(ea.max_concurrent_transfers <= t && ea.max_inbound_file_size <= f
                && ea.max_relay_bytes <= r && ea.max_active_streams <= s);
            let worse = RuntimePressure {
                storage: a.storage.max(b.storage),
                battery: a.battery.max(b.battery),
                memory: a.memory.max(b.memory),
            };
            let ew = effective_limits(n, worse);
            prop_assert!(ew.max_concurrent_transfers <= ea.max_concurrent_transfers
                && ew.max_inbound_file_size <= ea.max_inbound_file_size
                && ew.max_relay_bytes <= ea.max_relay_bytes
                && ew.max_active_streams <= ea.max_active_streams);
        }
    }
}
