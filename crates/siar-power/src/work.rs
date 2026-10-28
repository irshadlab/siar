//! §24-§26, §75: work descriptors, the five scheduler queues, decisions.

use serde::{Deserialize, Serialize};

/// §75. **Lower is more urgent** (`Critical < Interactive < … < Background`),
/// so sorting ascending gives dispatch order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum WorkPriority {
    Critical,
    Interactive,
    Normal,
    Bulk,
    Background,
}

/// §25. Examples from the spec: delivery ACK = `Tiny`, text send = `Low`,
/// photo upload = `Medium`, 5 GB transfer = `High`, software AV1 transcode =
/// `VeryHigh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EnergyClass {
    Tiny,
    Low,
    Medium,
    High,
    VeryHigh,
}

/// Which feature a piece of work belongs to — what §138's domain wrappers
/// (`power.files()`, `power.dtn()`, `power.media()`) and §140's
/// `effective_limits(WorkDomain)` key on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum WorkDomain {
    Message = 0,
    Receipt = 1,
    Identity = 2,
    Presence = 3,
    Files = 4,
    Dtn = 5,
    Relay = 6,
    Media = 7,
    Discovery = 8,
    Sync = 9,
    Indexing = 10,
    Backup = 11,
    Diagnostics = 12,
}

impl WorkDomain {
    pub const ALL: [Self; 13] = [
        Self::Message,
        Self::Receipt,
        Self::Identity,
        Self::Presence,
        Self::Files,
        Self::Dtn,
        Self::Relay,
        Self::Media,
        Self::Discovery,
        Self::Sync,
        Self::Indexing,
        Self::Backup,
        Self::Diagnostics,
    ];

    /// Domains whose work is large and long-running — the ones the bulk
    /// parallelism budget governs.
    #[must_use]
    pub const fn is_bulk(self) -> bool {
        matches!(
            self,
            Self::Files | Self::Dtn | Self::Relay | Self::Indexing | Self::Backup
        )
    }

    /// §29's "critical reserve": identity/security, small messages, ACKs.
    #[must_use]
    pub const fn in_critical_reserve(self) -> bool {
        matches!(self, Self::Message | Self::Receipt | Self::Identity)
    }

    /// §22 default: is work in this domain normally worth keeping when it
    /// cannot run now?
    #[must_use]
    pub const fn durable_by_default(self) -> bool {
        matches!(
            self,
            Self::Message
                | Self::Receipt
                | Self::Identity
                | Self::Files
                | Self::Dtn
                | Self::Relay
                | Self::Backup
                | Self::Sync
        )
    }
}

/// Tiny fixed set of domains (§71's application policy). `Copy`, no alloc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DomainMask(u16);

impl DomainMask {
    pub const EMPTY: Self = Self(0);

    #[must_use]
    pub const fn with(self, d: WorkDomain) -> Self {
        Self(self.0 | (1 << d as u8))
    }

    #[must_use]
    pub const fn contains(self, d: WorkDomain) -> bool {
        self.0 & (1 << d as u8) != 0
    }
}

/// §24.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerAwareWork {
    pub priority: WorkPriority,
    pub energy_class: EnergyClass,
    /// §21: durable work is *deferred*, never dropped.
    pub durable: bool,
    /// Absolute deadline in runtime-clock milliseconds (§172).
    pub deadline: Option<u64>,
    pub deferrable: bool,
    pub domain: WorkDomain,
    /// Needs the radio. OS network restrictions (§72) only bind these.
    pub uses_network: bool,
    /// §96-§97: the user tapped "Send now" / "Download now" **for this
    /// operation**. Overrides saver deferrals for it alone — it never changes
    /// the runtime's profile.
    pub user_initiated: bool,
    /// Set by the scheduler once a deferral outlived its maximum wait (§76).
    /// Never set by callers.
    pub aged: bool,
}

impl PowerAwareWork {
    /// `durable`, `deferrable` and `uses_network` take sensible defaults
    /// from the domain; override with the `with_*` methods.
    #[must_use]
    pub const fn new(
        domain: WorkDomain,
        priority: WorkPriority,
        energy_class: EnergyClass,
    ) -> Self {
        Self {
            priority,
            energy_class,
            durable: domain.durable_by_default(),
            deadline: None,
            deferrable: !matches!(priority, WorkPriority::Critical),
            domain,
            uses_network: !matches!(domain, WorkDomain::Indexing | WorkDomain::Backup),
            user_initiated: false,
            aged: false,
        }
    }

    #[must_use]
    pub const fn with_deadline(mut self, deadline_ms: u64) -> Self {
        self.deadline = Some(deadline_ms);
        self
    }

    #[must_use]
    pub const fn with_durable(mut self, durable: bool) -> Self {
        self.durable = durable;
        self
    }

    #[must_use]
    pub const fn with_deferrable(mut self, deferrable: bool) -> Self {
        self.deferrable = deferrable;
        self
    }

    #[must_use]
    pub const fn with_network(mut self, uses_network: bool) -> Self {
        self.uses_network = uses_network;
        self
    }

    #[must_use]
    pub const fn user_initiated(mut self) -> Self {
        self.user_initiated = true;
        self
    }

    /// Bulk-like for the metered-network rule: large by domain or by priority.
    #[must_use]
    pub const fn is_bulk_like(&self) -> bool {
        self.domain.is_bulk()
            || matches!(self.priority, WorkPriority::Bulk | WorkPriority::Background)
    }
}

/// §26.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerDecision {
    Run,
    RunThrottled,
    Defer,
    Drop,
    Reject,
}

impl PowerDecision {
    #[must_use]
    pub const fn runs(self) -> bool {
        matches!(self, Self::Run | Self::RunThrottled)
    }
}

/// What a deferred operation is waiting for (§77-§80). Persisted as the
/// "next eligibility hint" (§110) — a *hint*: the scheduler always
/// re-evaluates before running anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WaitFor {
    Charging,
    UnmeteredNetwork,
    Foreground,
    ThermalRecovery,
    BatteryRecovery,
    ResourceRelief,
    /// Not before this runtime-clock millisecond.
    Time(u64),
}

/// Why a decision came out the way it did (§115 diagnostics, §85 metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DecisionReason {
    Unrestricted,
    CriticalOverride,
    SmallMessageImmediate,
    UserInitiated,
    OsRestriction,
    ThermalSafety,
    CriticalBatteryReserve,
    BackgroundBudget,
    MeteredNetwork,
    AppPolicy,
    ResourcePressure,
    ThermalThrottle,
    BatteryThrottle,
    DeadlineNear,
    Aged,
    Ephemeral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evaluation {
    pub decision: PowerDecision,
    pub reason: DecisionReason,
    /// Present for `Defer`.
    pub wait: Option<WaitFor>,
}

impl Evaluation {
    #[must_use]
    pub const fn run(reason: DecisionReason) -> Self {
        Self {
            decision: PowerDecision::Run,
            reason,
            wait: None,
        }
    }

    #[must_use]
    pub const fn throttled(reason: DecisionReason) -> Self {
        Self {
            decision: PowerDecision::RunThrottled,
            reason,
            wait: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_by_urgency() {
        assert!(WorkPriority::Critical < WorkPriority::Interactive);
        assert!(WorkPriority::Bulk < WorkPriority::Background);
    }

    #[test]
    fn domain_mask_round_trip() {
        let m = DomainMask::EMPTY
            .with(WorkDomain::Files)
            .with(WorkDomain::Diagnostics);
        for d in WorkDomain::ALL {
            assert_eq!(
                m.contains(d),
                matches!(d, WorkDomain::Files | WorkDomain::Diagnostics),
                "{d:?}"
            );
        }
    }

    #[test]
    fn defaults_follow_the_domain() {
        let msg = PowerAwareWork::new(WorkDomain::Message, WorkPriority::Normal, EnergyClass::Low);
        assert!(msg.durable && msg.deferrable && msg.uses_network);
        let typing = PowerAwareWork::new(
            WorkDomain::Presence,
            WorkPriority::Background,
            EnergyClass::Tiny,
        );
        assert!(!typing.durable);
        let sos = PowerAwareWork::new(
            WorkDomain::Message,
            WorkPriority::Critical,
            EnergyClass::Tiny,
        );
        assert!(!sos.deferrable);
        let idx = PowerAwareWork::new(
            WorkDomain::Indexing,
            WorkPriority::Background,
            EnergyClass::Medium,
        );
        assert!(!idx.uses_network);
    }
}
