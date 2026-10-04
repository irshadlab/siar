//! §80 resource snapshot.

use crate::os_memory::budget_pressure;
use crate::storage::PressureState;

fn permille(used: u64, limit: u64) -> u64 {
    if limit == 0 {
        return if used > 0 { 1000 } else { 0 };
    }
    (u128::from(used) * 1000 / u128::from(limit)).min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceUsage {
    pub used: u64,
    pub limit: u64,
    pub peak: u64,
}

impl ResourceUsage {
    pub fn new(used: u64, limit: u64, peak: u64) -> Self {
        // A peak below the current value is a caller bug; repair it.
        Self {
            used,
            limit,
            peak: peak.max(used),
        }
    }

    pub fn ratio_permille(&self) -> u64 {
        permille(self.used, self.limit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageUsage {
    pub used: u64,
    pub reserved: u64,
    pub quota: u64,
    pub critical_reserve: u64,
}

impl StorageUsage {
    /// Fraction of quota that is spoken for (used + reserved).
    pub fn ratio_permille(&self) -> u64 {
        permille(self.used.saturating_add(self.reserved), self.quota)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CountUsage {
    pub used: u32,
    pub limit: u32,
}

impl CountUsage {
    pub fn ratio_permille(&self) -> u64 {
        permille(u64::from(self.used), u64::from(self.limit))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetworkUsage {
    pub bulk_bytes_per_sec: u64,
    pub bulk_limit_bytes_per_sec: u64,
    pub realtime_reserved_bytes_per_sec: u64,
}

impl NetworkUsage {
    pub fn ratio_permille(&self) -> u64 {
        permille(self.bulk_bytes_per_sec, self.bulk_limit_bytes_per_sec)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResourceSnapshot {
    pub memory: ResourceUsage,
    pub storage: StorageUsage,
    pub connections: CountUsage,
    pub streams: CountUsage,
    pub network: NetworkUsage,
    /// As reported by the pressure controller (may include OS signals the
    /// numbers above can't show).
    pub pressure: PressureState,
}

impl ResourceSnapshot {
    pub fn worst_ratio_permille(&self) -> u64 {
        [
            self.memory.ratio_permille(),
            self.storage.ratio_permille(),
            self.connections.ratio_permille(),
            self.streams.ratio_permille(),
            self.network.ratio_permille(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0)
    }

    /// Pressure implied by the numbers themselves.
    pub fn derived_pressure(&self) -> PressureState {
        budget_pressure(self.worst_ratio_permille(), 1000)
    }

    /// The reported pressure, never lower than what the numbers imply.
    pub fn effective_pressure(&self) -> PressureState {
        self.pressure.max(self.derived_pressure())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_handle_zero_limits_and_huge_values() {
        assert_eq!(ResourceUsage::new(0, 0, 0).ratio_permille(), 0);
        assert_eq!(ResourceUsage::new(1, 0, 1).ratio_permille(), 1000);
        assert_eq!(ResourceUsage::new(72, 128, 80).ratio_permille(), 562);
        assert_eq!(
            ResourceUsage::new(u64::MAX, u64::MAX, 0).ratio_permille(),
            1000
        );
    }

    #[test]
    fn peak_is_repaired_to_at_least_current() {
        assert_eq!(ResourceUsage::new(10, 100, 3).peak, 10);
    }

    #[test]
    fn storage_counts_reservations_as_spoken_for() {
        let s = StorageUsage {
            used: 40,
            reserved: 50,
            quota: 100,
            critical_reserve: 10,
        };
        assert_eq!(s.ratio_permille(), 900);
    }

    #[test]
    fn snapshot_pressure_is_never_lower_than_the_numbers_imply() {
        let mut s = ResourceSnapshot {
            streams: CountUsage {
                used: 32,
                limit: 32,
            },
            ..Default::default()
        };
        assert_eq!(s.derived_pressure(), PressureState::Exhausted);
        s.pressure = PressureState::Normal; // stale controller reading
        assert_eq!(s.effective_pressure(), PressureState::Exhausted);
        s.streams = CountUsage { used: 1, limit: 32 };
        s.pressure = PressureState::Critical; // OS signal the numbers can't see
        assert_eq!(s.effective_pressure(), PressureState::Critical);
    }

    #[test]
    fn worst_dimension_wins() {
        let s = ResourceSnapshot {
            memory: ResourceUsage::new(10, 100, 10),
            connections: CountUsage { used: 9, limit: 10 },
            ..Default::default()
        };
        assert_eq!(s.worst_ratio_permille(), 900);
        assert_eq!(s.derived_pressure(), PressureState::Critical);
    }
}
