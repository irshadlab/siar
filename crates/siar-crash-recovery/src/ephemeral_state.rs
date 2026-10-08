//! §106 "Discovery Recovery", §107 "Capability Dynamic State
//! Recovery", §108 "Resource Pressure Recovery".
//!
//! Three kinds of state that describe *the world right now*: nearby
//! peers, battery/thermal/hardware, memory/disk pressure. None of it
//! is recovered; it is re-observed. Only long-lived trusted peer
//! endpoints survive a restart.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------
// §106
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveredEntry {
    /// A trusted peer endpoint worth keeping across restarts, as
    /// opposed to something merely seen nearby.
    pub trusted_long_lived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryRecovery {
    pub keep: Vec<usize>,
    pub discard: Vec<usize>,
    /// Always true: discovery is rescanned after every restart.
    pub rescan_required: bool,
}

pub fn recover_discovery(entries: &[DiscoveredEntry]) -> DiscoveryRecovery {
    let (keep, discard): (Vec<usize>, Vec<usize>) =
        (0..entries.len()).partition(|&i| entries[i].trusted_long_lived);
    DiscoveryRecovery {
        keep,
        discard,
        rescan_required: true,
    }
}

// ---------------------------------------------------------------------
// §107
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DynamicCapability {
    Battery,
    Thermal,
    Hardware,
    NetworkInterfaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRecovery {
    /// Reset and ask the platform again.
    RequeryPlatform,
}

pub const fn recover_dynamic_capability(_c: DynamicCapability) -> CapabilityRecovery {
    CapabilityRecovery::RequeryPlatform
}

// ---------------------------------------------------------------------
// §108
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PressureKind {
    Memory,
    Storage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureSource {
    /// Read from the live environment.
    CurrentEnvironment,
    /// Derived from actual disk state.
    DiskState,
}

pub const fn pressure_source(k: PressureKind) -> PressureSource {
    match k {
        PressureKind::Memory => PressureSource::CurrentEnvironment,
        PressureKind::Storage => PressureSource::DiskState,
    }
}

/// §108: "memory pressure currently critical" is never persisted as
/// authoritative — nor is any other pressure reading.
pub const fn persisted_as_authoritative(_k: PressureKind) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PressureLevel {
    Normal,
    Elevated,
    Critical,
}

/// Thresholds as free space in permille of total (e.g. 100 = 10%).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageThresholds {
    pub elevated_below_permille: u32,
    pub critical_below_permille: u32,
}

/// Storage pressure from the numbers the filesystem reports. A
/// zero-size or inconsistent report (`free > total`) is treated as
/// `Critical`: if we can't trust the figure, assume the worst.
pub const fn storage_pressure(free: u64, total: u64, t: StorageThresholds) -> PressureLevel {
    if total == 0 || free > total {
        return PressureLevel::Critical;
    }
    // free/total < p/1000  ⇔  free*1000 < p*total  (u128: no overflow)
    let f = free as u128 * 1000;
    if f < t.critical_below_permille as u128 * total as u128 {
        PressureLevel::Critical
    } else if f < t.elevated_below_permille as u128 * total as u128 {
        PressureLevel::Elevated
    } else {
        PressureLevel::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_trusted_long_lived_peers_survive_and_rescan_is_always_required() {
        let e = [
            DiscoveredEntry {
                trusted_long_lived: false,
            },
            DiscoveredEntry {
                trusted_long_lived: true,
            },
            DiscoveredEntry {
                trusted_long_lived: false,
            },
        ];
        let r = recover_discovery(&e);
        assert_eq!(r.keep, vec![1]);
        assert_eq!(r.discard, vec![0, 2]);
        assert!(r.rescan_required);
        let empty = recover_discovery(&[]);
        assert!(empty.rescan_required && empty.keep.is_empty());
    }

    #[test]
    fn every_dynamic_capability_is_requeried() {
        for c in [
            DynamicCapability::Battery,
            DynamicCapability::Thermal,
            DynamicCapability::Hardware,
            DynamicCapability::NetworkInterfaces,
        ] {
            assert_eq!(
                recover_dynamic_capability(c),
                CapabilityRecovery::RequeryPlatform
            );
        }
    }

    #[test]
    fn pressure_is_never_authoritative_and_has_the_right_source() {
        assert!(!persisted_as_authoritative(PressureKind::Memory));
        assert!(!persisted_as_authoritative(PressureKind::Storage));
        assert_eq!(
            pressure_source(PressureKind::Memory),
            PressureSource::CurrentEnvironment
        );
        assert_eq!(
            pressure_source(PressureKind::Storage),
            PressureSource::DiskState
        );
    }

    const T: StorageThresholds = StorageThresholds {
        elevated_below_permille: 150,
        critical_below_permille: 50,
    };

    #[test]
    fn storage_pressure_levels_from_disk_numbers() {
        assert_eq!(storage_pressure(500, 1_000, T), PressureLevel::Normal);
        assert_eq!(storage_pressure(149, 1_000, T), PressureLevel::Elevated);
        assert_eq!(storage_pressure(150, 1_000, T), PressureLevel::Normal);
        assert_eq!(storage_pressure(49, 1_000, T), PressureLevel::Critical);
        assert_eq!(storage_pressure(0, 1_000, T), PressureLevel::Critical);
    }

    #[test]
    fn untrustworthy_disk_figures_are_critical() {
        assert_eq!(storage_pressure(0, 0, T), PressureLevel::Critical);
        assert_eq!(storage_pressure(10, 5, T), PressureLevel::Critical);
    }

    #[test]
    fn huge_disks_do_not_overflow() {
        assert_eq!(
            storage_pressure(u64::MAX / 2, u64::MAX, T),
            PressureLevel::Normal
        );
    }
}
