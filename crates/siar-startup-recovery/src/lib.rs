#![forbid(unsafe_code)]

//! Two sections of `04-offline-event-log-architecture.md` that turned
//! out to share nothing domain-specific and everything structural:
//! §47 "Startup Recovery" ([`sequence`]) and §48 "Work Queue
//! Reconciliation" ([`reconciliation`]). Neither needed a dependency
//! on `siar-event-log` or any domain crate — see each module's own doc
//! comment for why staying generic was the actual point, not an
//! afterthought.

pub mod reconciliation;
pub mod sequence;

pub use reconciliation::{
    pending_from_history, pending_from_history_with_expiry, reconcile, ReconciliationReport,
};
pub use sequence::{step, StartupFailure, StartupSequence, StartupStep, Step, StepError};
