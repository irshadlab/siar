//! §106-§108: third-party extensions cannot bypass power policy.
//!
//! An extension *declares* (energy class, priority, durable/ephemeral). The
//! governor turns that into a [`PowerAwareWork`] the normal policy engine then
//! evaluates — so the extension never receives anything the policy would not
//! give a built-in feature. On top of that it enforces, per extension:
//!
//! * **claims are clamped** — no `Critical` priority, no `user_initiated`, no
//!   `aged` (those are the runtime's to grant, §96, §76);
//! * **a bounded queue** (§106 "bounded queues");
//! * **an energy-credit bucket** that refills in proportion to the *current
//!   CPU budget* (§106 "effective execution budget"), so a hot or low-battery
//!   device slows extensions automatically;
//! * **strikes with backoff** (§108): an extension that keeps asking while
//!   out of credit is suspended for exponentially longer, up to a cap.

use std::collections::HashMap;

use crate::budget::{CpuBudgetClass, PowerBudget};
use crate::work::{EnergyClass, PowerAwareWork, WorkDomain, WorkPriority};

/// Opaque extension identity (assigned by the plugin host).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtensionId(pub u32);

/// §107: all an extension may state about its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionDecl {
    pub energy: EnergyClass,
    pub priority: WorkPriority,
    pub durable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionLimits {
    /// Maximum operations an extension may have admitted-but-unfinished.
    pub max_in_flight: u32,
    /// Credit capacity (a burst allowance).
    pub burst: u32,
    /// Credits per second at `CpuBudgetClass::Normal`.
    pub refill_per_sec: u32,
    /// Consecutive over-budget requests before suspension starts.
    pub strikes_before_suspend: u32,
    pub suspend_base_ms: u64,
    pub suspend_max_ms: u64,
}

impl Default for ExtensionLimits {
    fn default() -> Self {
        Self {
            max_in_flight: 8,
            burst: 120,
            refill_per_sec: 4,
            strikes_before_suspend: 10,
            suspend_base_ms: 5_000,
            suspend_max_ms: 5 * 60_000,
        }
    }
}

/// Coarse relative cost (§98: classes, not joules).
#[must_use]
pub const fn credit_cost(e: EnergyClass) -> u32 {
    match e {
        EnergyClass::Tiny => 1,
        EnergyClass::Low => 3,
        EnergyClass::Medium => 12,
        EnergyClass::High => 60,
        EnergyClass::VeryHigh => 240,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionVerdict {
    /// Hand this to the normal policy engine.
    Admit(PowerAwareWork),
    /// Not enough credit right now.
    Throttled {
        retry_after_ms: u64,
    },
    TooManyInFlight,
    /// Misbehaving: refused until this runtime-clock millisecond.
    Suspended {
        until_ms: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    credits: f64,
    last_ms: u64,
    in_flight: u32,
    strikes: u32,
    suspensions: u32,
    suspended_until: u64,
}

#[derive(Debug, Default)]
pub struct ExtensionGovernor {
    limits: ExtensionLimits,
    buckets: HashMap<ExtensionId, Bucket>,
}

/// Credit refill scales with the CPU the device can currently afford.
fn cpu_factor(c: CpuBudgetClass) -> f64 {
    match c {
        CpuBudgetClass::Minimal => 0.0,
        CpuBudgetClass::Low => 0.25,
        CpuBudgetClass::Normal => 1.0,
        CpuBudgetClass::High => 2.0,
    }
}

impl ExtensionGovernor {
    #[must_use]
    pub fn new(limits: ExtensionLimits) -> Self {
        Self {
            limits,
            buckets: HashMap::new(),
        }
    }

    fn bucket(&mut self, id: ExtensionId, now: u64) -> &mut Bucket {
        let burst = f64::from(self.limits.burst);
        self.buckets.entry(id).or_insert(Bucket {
            credits: burst,
            last_ms: now,
            in_flight: 0,
            strikes: 0,
            suspensions: 0,
            suspended_until: 0,
        })
    }

    pub fn request(
        &mut self,
        id: ExtensionId,
        decl: ExtensionDecl,
        budget: &PowerBudget,
        now: u64,
    ) -> ExtensionVerdict {
        let limits = self.limits;
        let factor = cpu_factor(budget.cpu_class);
        let b = self.bucket(id, now);

        if now < b.suspended_until {
            return ExtensionVerdict::Suspended {
                until_ms: b.suspended_until,
            };
        }
        // Refill (monotonic-safe).
        let dt = now.saturating_sub(b.last_ms) as f64 / 1000.0;
        b.last_ms = b.last_ms.max(now);
        b.credits = (b.credits + dt * f64::from(limits.refill_per_sec) * factor)
            .min(f64::from(limits.burst));

        if b.in_flight >= limits.max_in_flight {
            return ExtensionVerdict::TooManyInFlight;
        }
        let cost = f64::from(credit_cost(decl.energy));
        if b.credits < cost {
            b.strikes += 1;
            if b.strikes >= limits.strikes_before_suspend {
                // §108: continuous over-asking ⇒ exponential suspension.
                let span = limits
                    .suspend_base_ms
                    .saturating_mul(1u64 << b.suspensions.min(20))
                    .min(limits.suspend_max_ms);
                b.suspensions = b.suspensions.saturating_add(1);
                b.strikes = 0;
                b.suspended_until = now.saturating_add(span);
                return ExtensionVerdict::Suspended {
                    until_ms: b.suspended_until,
                };
            }
            let rate = f64::from(limits.refill_per_sec) * factor;
            let retry = if rate <= 0.0 {
                60_000
            } else {
                (((cost - b.credits) / rate) * 1000.0).ceil() as u64
            };
            return ExtensionVerdict::Throttled {
                retry_after_ms: retry.clamp(100, 60_000),
            };
        }
        b.credits -= cost;
        b.strikes = 0;
        b.in_flight += 1;

        // Clamp: an extension can never claim what only the runtime may grant.
        let priority = decl.priority.max(WorkPriority::Interactive);
        let mut work = PowerAwareWork::new(WorkDomain::Extension, priority, decl.energy)
            .with_durable(decl.durable);
        work.user_initiated = false;
        work.aged = false;
        ExtensionVerdict::Admit(work)
    }

    /// The host calls this when an admitted operation finishes or is cancelled.
    pub fn finished(&mut self, id: ExtensionId) {
        if let Some(b) = self.buckets.get_mut(&id) {
            b.in_flight = b.in_flight.saturating_sub(1);
        }
    }

    #[must_use]
    pub fn in_flight(&self, id: ExtensionId) -> u32 {
        self.buckets.get(&id).map_or(0, |b| b.in_flight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{PowerPolicyConfig, PowerPolicyProfile};
    use crate::state::*;
    use proptest::prelude::*;

    fn budget(profile: PowerPolicyProfile, s: PowerState) -> PowerBudget {
        PowerBudget::derive(profile, &s, &PowerPolicyConfig::default())
    }
    fn plugged() -> PowerBudget {
        budget(
            PowerPolicyProfile::Balanced,
            PowerState {
                charging: ChargingState::Full,
                ..Default::default()
            },
        )
    }
    fn decl(e: EnergyClass) -> ExtensionDecl {
        ExtensionDecl {
            energy: e,
            priority: WorkPriority::Normal,
            durable: true,
        }
    }

    #[test]
    fn an_extension_cannot_claim_critical_or_user_initiated() {
        let mut g = ExtensionGovernor::default();
        let d = ExtensionDecl {
            energy: EnergyClass::Tiny,
            priority: WorkPriority::Critical,
            durable: true,
        };
        let ExtensionVerdict::Admit(w) = g.request(ExtensionId(1), d, &plugged(), 0) else {
            panic!()
        };
        assert_eq!(
            w.priority,
            WorkPriority::Interactive,
            "clamped, never Critical"
        );
        assert!(!w.user_initiated && !w.aged);
        assert_eq!(w.domain, WorkDomain::Extension);
    }

    #[test]
    fn heavy_requests_drain_credit_and_get_throttled() {
        let mut g = ExtensionGovernor::new(ExtensionLimits {
            max_in_flight: 100,
            ..Default::default()
        });
        let b = plugged();
        let mut admitted = 0;
        for _ in 0..10 {
            if matches!(
                g.request(ExtensionId(1), decl(EnergyClass::High), &b, 0),
                ExtensionVerdict::Admit(_)
            ) {
                admitted += 1;
            }
        }
        assert_eq!(admitted, 2, "burst of 120 credits buys two 60-credit jobs");
        assert!(matches!(
            g.request(ExtensionId(1), decl(EnergyClass::High), &b, 0),
            ExtensionVerdict::Throttled { .. } | ExtensionVerdict::Suspended { .. }
        ));
    }

    #[test]
    fn credit_refills_with_time_but_slower_when_the_device_is_hot() {
        let mk = |thermal| {
            let mut g = ExtensionGovernor::new(ExtensionLimits {
                max_in_flight: 100,
                ..Default::default()
            });
            let b = budget(
                PowerPolicyProfile::Balanced,
                PowerState {
                    thermal,
                    charging: ChargingState::Full,
                    ..Default::default()
                },
            );
            for _ in 0..2 {
                g.request(ExtensionId(1), decl(EnergyClass::High), &b, 0);
            }
            // After 20 s: Nominal refills 80 credits (a High job fits); Serious only 20.
            matches!(
                g.request(ExtensionId(1), decl(EnergyClass::High), &b, 20_000),
                ExtensionVerdict::Admit(_)
            )
        };
        assert!(mk(ThermalState::Nominal));
        assert!(!mk(ThermalState::Serious));
    }

    #[test]
    fn queue_is_bounded_until_the_host_reports_completion() {
        let mut g = ExtensionGovernor::new(ExtensionLimits {
            max_in_flight: 2,
            ..Default::default()
        });
        let b = plugged();
        assert!(matches!(
            g.request(ExtensionId(1), decl(EnergyClass::Tiny), &b, 0),
            ExtensionVerdict::Admit(_)
        ));
        assert!(matches!(
            g.request(ExtensionId(1), decl(EnergyClass::Tiny), &b, 0),
            ExtensionVerdict::Admit(_)
        ));
        assert_eq!(
            g.request(ExtensionId(1), decl(EnergyClass::Tiny), &b, 0),
            ExtensionVerdict::TooManyInFlight
        );
        g.finished(ExtensionId(1));
        assert!(matches!(
            g.request(ExtensionId(1), decl(EnergyClass::Tiny), &b, 0),
            ExtensionVerdict::Admit(_)
        ));
    }

    #[test]
    fn a_hammering_extension_is_suspended_with_growing_backoff() {
        let mut g = ExtensionGovernor::new(ExtensionLimits {
            max_in_flight: 1_000,
            strikes_before_suspend: 3,
            suspend_base_ms: 1_000,
            ..Default::default()
        });
        let b = plugged();
        let id = ExtensionId(7);
        for _ in 0..2 {
            g.request(id, decl(EnergyClass::High), &b, 0); // drain
        }
        let mut first = None;
        for t in 0..10 {
            if let ExtensionVerdict::Suspended { until_ms } =
                g.request(id, decl(EnergyClass::High), &b, t)
            {
                first = Some(until_ms);
                break;
            }
        }
        let first = first.expect("must be suspended after repeated over-asking");
        assert!(
            matches!(
                g.request(id, decl(EnergyClass::Tiny), &b, first - 1),
                ExtensionVerdict::Suspended { .. }
            ),
            "even tiny work is refused while suspended"
        );
        // A second offence at the same cost earns a longer sentence.
        let mut second = None;
        for t in 0..20u64 {
            if let ExtensionVerdict::Suspended { until_ms } =
                g.request(id, decl(EnergyClass::VeryHigh), &b, first + t)
            {
                second = Some((until_ms, first + t));
                break;
            }
        }
        let (until2, at2) = second.expect("suspended again");
        assert!(
            until2 - at2 > first,
            "backoff grows: {} vs {}",
            until2 - at2,
            first
        );
        // A different, well-behaved extension is unaffected.
        assert!(matches!(
            g.request(ExtensionId(8), decl(EnergyClass::Tiny), &b, 5),
            ExtensionVerdict::Admit(_)
        ));
    }

    #[test]
    fn minimal_cpu_budget_stops_refill_entirely() {
        let mut g = ExtensionGovernor::new(ExtensionLimits {
            max_in_flight: 100,
            ..Default::default()
        });
        let melt = budget(
            PowerPolicyProfile::Balanced,
            PowerState {
                thermal: ThermalState::Critical,
                charging: ChargingState::Full,
                ..Default::default()
            },
        );
        for _ in 0..2 {
            g.request(ExtensionId(1), decl(EnergyClass::High), &melt, 0);
        }
        assert!(!matches!(
            g.request(ExtensionId(1), decl(EnergyClass::High), &melt, 3_600_000),
            ExtensionVerdict::Admit(_)
        ));
    }

    proptest! {
        /// Whatever an extension declares, the descriptor it gets back never
        /// carries runtime-only privileges.
        #[test]
        fn clamping_holds_for_every_declaration(e in 0u8..5, p in 0u8..5, durable in any::<bool>()) {
            let energy = [EnergyClass::Tiny, EnergyClass::Low, EnergyClass::Medium, EnergyClass::High, EnergyClass::VeryHigh][e as usize];
            let prio = [WorkPriority::Critical, WorkPriority::Interactive, WorkPriority::Normal, WorkPriority::Bulk, WorkPriority::Background][p as usize];
            let mut g = ExtensionGovernor::default();
            if let ExtensionVerdict::Admit(w) = g.request(ExtensionId(1), ExtensionDecl { energy, priority: prio, durable }, &plugged(), 0) {
                prop_assert!(w.priority >= WorkPriority::Interactive);
                prop_assert!(!w.user_initiated && !w.aged);
                prop_assert_eq!(w.durable, durable);
            }
        }
    }
}
