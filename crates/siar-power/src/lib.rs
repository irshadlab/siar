//! # siar-power — battery-aware scheduling (sys-arch Part 13)
//!
//! One canonical power model for the whole workspace, and one policy engine
//! that every background-capable subsystem asks instead of inventing its own
//! battery logic.
//!
//! ```text
//! platform adapter ──RawPowerReading──▶ PowerTracker ──Transition──▶ subscribers
//!   (sysfs / push)     (hysteresis §81-82, coalescing §84, epoch §83)
//!                                │
//!                       PowerState → profile → PowerBudget
//!                                │
//!        PowerAwareWork ──▶ evaluate_detailed ──▶ Run / RunThrottled / Defer / Drop / Reject
//!                                │                              │
//!                        PowerScheduler (durable deferrals, aging, herd control, restart-safe)
//! ```
//!
//! ## Module map (spec §137)
//!
//! | module | spec | what |
//! |---|---|---|
//! | [`state`] | §4-8, 15, 86-87 | coarse inputs, `PowerState`, `PeerPowerHint` |
//! | [`hysteresis`] | §81-82 | battery / thermal filters |
//! | [`profile`] | §9-14, 69-70 | profiles, config, user prefs |
//! | [`budget`] | §16-19, 140 | `PowerBudget`, `EffectiveLimits` |
//! | [`work`] | §24-26, 75 | work descriptor, decisions, wait hints |
//! | [`policy`] | §20, 26-28, 68, 72-74, 96-97 | the layered engine |
//! | [`tracker`] | §83-84 | raw → state, epoch, triggers |
//! | [`scheduler`] | §21, 75-80, 110-112 | deferral, aging, herd, restart |
//! | [`discovery`] | §34-38, 91 | duty cycle, upgrade cost |
//! | [`wake`] | §30-32 | wake-window batching |
//! | [`domains`] | §39-57, 92-95, 138 | files / DTN / media advice |
//! | [`platform`] | §58-67, 139 | Linux sysfs reader, push source |
//! | [`diagnostics`] | §85, 98, 115 | counters + snapshot |
//!
//! ## Invariants (all property- or scenario-tested)
//!
//! * A restriction always beats a boost: no input grows a budget past its
//!   profile's base.
//! * Durable work is only ever run or deferred — never dropped or rejected.
//! * An OS background restriction, thermal-critical, the critical-battery
//!   reserve and an explicit "no bulk on mobile data" policy cannot be lifted
//!   by aging, deadlines, a user override or an emergency.
//! * A user override affects one operation and never changes the profile.
//! * No exact battery percentage or timestamp survives past the tracker.
//!
//! ## Not done (named, not hidden)
//!
//! Android `BatteryManager`/Doze/Foreground-Service glue and iOS adapters
//! (§58-64) — the Rust side is [`platform::PushPowerSource`]; the Kotlin and
//! Swift producers are not written. Vendor quirk tables (§100-102),
//! empirical tuning (§99), real-device and benchmark suites (§130-136).
//! No subsystem outside the workspace's own tests calls this crate yet except
//! through the bridges documented in `ROADMAP.md`.

#![forbid(unsafe_code)]

pub mod budget;
pub mod diagnostics;
pub mod discovery;
pub mod domains;
pub mod error;
pub mod hysteresis;
pub mod platform;
pub mod policy;
pub mod profile;
pub mod scheduler;
pub mod state;
pub mod tracker;
pub mod wake;
pub mod work;

pub use budget::{
    BackgroundBudgetClass, CpuBudgetClass, EffectiveLimits, PowerBudget, RadioBudgetClass,
};
pub use diagnostics::{PowerDiagnostics, PowerMetrics};
pub use domains::PowerAdvisor;
pub use error::{PowerError, Result};
pub use policy::{DefaultPowerPolicy, PowerPolicyEngine, ResourcePressure, ResourceSnapshot};
pub use profile::{
    select_profile, Deployment, PowerPolicyConfig, PowerPolicyProfile, UserPowerPrefs,
};
pub use scheduler::{
    energy_for_bytes, OpId, PersistedDeferral, PowerScheduler, Release, SchedulerConfig, Submission,
};
pub use state::{
    AppActivityState, BatteryLevelClass, ChargingState, PeerPowerHint, PowerState, ThermalState,
};
pub use tracker::{PowerStateEpoch, PowerTracker, RawPowerReading, Transition};
pub use work::{
    DecisionReason, DomainMask, EnergyClass, Evaluation, PowerAwareWork, PowerDecision, WaitFor,
    WorkDomain, WorkPriority,
};
