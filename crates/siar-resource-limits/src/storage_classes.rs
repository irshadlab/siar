//! §45 storage classes (and the eviction half of §47).
//!
//! Four classes with *separate* quotas, charged against one shared
//! ceiling (`quota − critical reserve`), so no class can crowd out the
//! others and none can touch the critical reserve:
//!
//! | class   | what                         | reclaimed by                 |
//! |---------|------------------------------|------------------------------|
//! | Durable | messages, events, identities | never evicted by policy      |
//! | Cache   | thumbnails, previews, indexes| evicted first, any time      |
//! | Staging | partial downloads/uploads    | expiry / transfer completion |
//! | Relay   | DTN bundles for others       | evicted lowest-priority-first|
//!
//! A charge that exceeds its own class quota is refused outright. A charge
//! within its class quota that would overflow the shared ceiling is
//! answered with an **eviction plan** (cache first, then relay) when
//! evicting could make room, so callers reclaim space in a defined order
//! instead of failing a durable write.

use crate::profile::ResourceConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageClass {
    Durable,
    Cache,
    Staging,
    Relay,
}

impl StorageClass {
    const COUNT: usize = 4;
    const ALL: [StorageClass; Self::COUNT] =
        [Self::Durable, Self::Cache, Self::Staging, Self::Relay];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageClassQuotas {
    pub durable: u64,
    pub cache: u64,
    pub staging: u64,
    pub relay: u64,
}

impl StorageClassQuotas {
    /// Cache gets an eighth of the quota and staging a quarter (matching
    /// [`FeatureLimits`](crate::feature_limits::FeatureLimits)); relay is
    /// the configured relay quota; durable may use everything the other
    /// classes don't have a claim on.
    pub fn for_config(cfg: &ResourceConfig) -> Self {
        let ceiling = cfg
            .storage
            .quota_bytes
            .saturating_sub(cfg.storage.critical_reserve_bytes);
        Self {
            durable: ceiling,
            cache: cfg.storage.quota_bytes / 8,
            staging: cfg.storage.quota_bytes / 4,
            relay: cfg.storage.relay_quota_bytes,
        }
    }

    fn of(&self, c: StorageClass) -> u64 {
        match c {
            StorageClass::Durable => self.durable,
            StorageClass::Cache => self.cache,
            StorageClass::Staging => self.staging,
            StorageClass::Relay => self.relay,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChargeOutcome {
    #[error("class quota exceeded")]
    ClassQuotaExceeded,
    /// Would fit its class quota but not the shared ceiling; evicting
    /// these (in order) would make room.
    #[error("storage full; eviction needed")]
    NeedsEviction(Vec<(StorageClass, u64)>),
    #[error("storage full and nothing is evictable")]
    Full,
}

#[derive(Debug)]
pub struct StorageLedger {
    quotas: StorageClassQuotas,
    ceiling: u64,
    used: [u64; StorageClass::COUNT],
}

impl StorageLedger {
    pub fn new(quotas: StorageClassQuotas, shared_ceiling: u64) -> Self {
        Self {
            quotas,
            ceiling: shared_ceiling,
            used: [0; StorageClass::COUNT],
        }
    }

    pub fn from_config(cfg: &ResourceConfig) -> Self {
        Self::new(
            StorageClassQuotas::for_config(cfg),
            cfg.storage
                .quota_bytes
                .saturating_sub(cfg.storage.critical_reserve_bytes),
        )
    }

    pub fn used(&self, c: StorageClass) -> u64 {
        self.used[c as usize]
    }

    pub fn total_used(&self) -> u64 {
        self.used.iter().fold(0u64, |a, b| a.saturating_add(*b))
    }

    pub fn try_charge(&mut self, class: StorageClass, bytes: u64) -> Result<(), ChargeOutcome> {
        let i = class as usize;
        let new_class = self.used[i]
            .checked_add(bytes)
            .ok_or(ChargeOutcome::ClassQuotaExceeded)?;
        if new_class > self.quotas.of(class) {
            return Err(ChargeOutcome::ClassQuotaExceeded);
        }
        let new_total = self
            .total_used()
            .checked_add(bytes)
            .ok_or(ChargeOutcome::Full)?;
        if new_total > self.ceiling {
            let need = new_total - self.ceiling;
            return Err(self.plan_eviction(class, need));
        }
        self.used[i] = new_class;
        Ok(())
    }

    fn plan_eviction(&self, requesting: StorageClass, need: u64) -> ChargeOutcome {
        // Cache first, then relay. A class never evicts itself to satisfy
        // itself (that would just churn), and durable/staging are never
        // policy-evicted.
        let mut plan = Vec::new();
        let mut left = need;
        for c in [StorageClass::Cache, StorageClass::Relay] {
            if c == requesting || left == 0 {
                continue;
            }
            let take = self.used[c as usize].min(left);
            if take > 0 {
                plan.push((c, take));
                left -= take;
            }
        }
        if left == 0 {
            ChargeOutcome::NeedsEviction(plan)
        } else {
            ChargeOutcome::Full
        }
    }

    /// Return bytes (evicted, expired, completed). Saturates.
    pub fn release(&mut self, class: StorageClass, bytes: u64) {
        let u = &mut self.used[class as usize];
        *u = u.saturating_sub(bytes);
    }

    /// Apply an eviction plan returned by [`Self::try_charge`].
    pub fn apply_plan(&mut self, plan: &[(StorageClass, u64)]) {
        for (c, n) in plan {
            self.release(*c, *n);
        }
    }

    pub fn classes() -> [StorageClass; StorageClass::COUNT] {
        StorageClass::ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use StorageClass::*;

    fn ledger() -> StorageLedger {
        // ceiling 1000; class quotas: durable 1000, cache 100, staging 250, relay 300
        StorageLedger::new(
            StorageClassQuotas {
                durable: 1_000,
                cache: 100,
                staging: 250,
                relay: 300,
            },
            1_000,
        )
    }

    #[test]
    fn each_class_has_its_own_quota() {
        let mut l = ledger();
        assert!(l.try_charge(Cache, 100).is_ok());
        assert_eq!(
            l.try_charge(Cache, 1),
            Err(ChargeOutcome::ClassQuotaExceeded)
        );
        assert!(l.try_charge(Staging, 250).is_ok());
        assert_eq!(
            l.try_charge(Staging, 1),
            Err(ChargeOutcome::ClassQuotaExceeded)
        );
        assert!(l.try_charge(Relay, 300).is_ok());
        assert_eq!(l.total_used(), 650);
    }

    #[test]
    fn durable_write_on_a_full_disk_is_answered_with_a_cache_then_relay_eviction_plan() {
        let mut l = ledger();
        l.try_charge(Cache, 100).unwrap();
        l.try_charge(Relay, 300).unwrap();
        l.try_charge(Staging, 250).unwrap();
        l.try_charge(Durable, 350).unwrap(); // total 1000: full
        let err = l.try_charge(Durable, 150).unwrap_err();
        assert_eq!(
            err,
            ChargeOutcome::NeedsEviction(vec![(Cache, 100), (Relay, 50)])
        );
        let ChargeOutcome::NeedsEviction(plan) = err else {
            unreachable!()
        };
        l.apply_plan(&plan);
        assert!(l.try_charge(Durable, 150).is_ok());
        assert_eq!(l.total_used(), 1_000);
    }

    #[test]
    fn durable_and_staging_are_never_evicted_by_policy() {
        let mut l = ledger();
        l.try_charge(Durable, 700).unwrap();
        l.try_charge(Staging, 250).unwrap();
        // 950 used; only 50 free and nothing evictable beyond that
        assert_eq!(l.try_charge(Durable, 100), Err(ChargeOutcome::Full));
        assert_eq!(l.used(Durable), 700, "a refused charge changes nothing");
    }

    #[test]
    fn a_class_does_not_evict_itself() {
        let mut l = ledger();
        l.try_charge(Durable, 950).unwrap();
        l.try_charge(Cache, 50).unwrap(); // total 1000: full
                                          // Within cache's own quota (100) but over the shared ceiling, and the
                                          // only evictable bytes are cache's own: refuse rather than churn.
        assert_eq!(l.try_charge(Cache, 50), Err(ChargeOutcome::Full));
        assert_eq!(l.used(Cache), 50, "nothing changed");
        // Whereas a durable write may evict that cache.
        l.release(Durable, 50);
        assert_eq!(
            l.try_charge(Durable, 100),
            Err(ChargeOutcome::NeedsEviction(vec![(Cache, 50)]))
        );
    }

    #[test]
    fn overflow_and_over_release_are_handled() {
        let mut l = ledger();
        assert_eq!(
            l.try_charge(Durable, u64::MAX),
            Err(ChargeOutcome::ClassQuotaExceeded)
        );
        l.release(Durable, 99);
        assert_eq!(l.used(Durable), 0);
    }

    #[test]
    fn config_derived_quotas_never_touch_the_critical_reserve() {
        use crate::profile::ResourceProfile::*;
        for p in [Embedded, MobileLow, MobileNormal, Desktop, Server] {
            let cfg = ResourceConfig::for_profile(p);
            let q = StorageClassQuotas::for_config(&cfg);
            let ceiling = cfg.storage.quota_bytes - cfg.storage.critical_reserve_bytes;
            for c in StorageLedger::classes() {
                assert!(q.of(c) <= cfg.storage.quota_bytes, "{p:?}/{c:?}");
            }
            let mut l = StorageLedger::from_config(&cfg);
            // Fill the shared ceiling with durable data: the reserve stays free.
            l.try_charge(Durable, ceiling).unwrap();
            assert!(l.try_charge(Durable, 1).is_err());
            assert!(l.total_used() + cfg.storage.critical_reserve_bytes <= cfg.storage.quota_bytes);
            assert_eq!(q.relay, cfg.storage.relay_quota_bytes);
        }
    }
}
