//! # siar-app-core
//!
//! The application core, extracted from `apps/desktop` so it no longer drags
//! a GUI toolkit along. It takes an `Arc<StateHub>` (from `siar-ui-model`)
//! and a command channel; *who* drives those — a Slint window, a daemon, an
//! end-to-end test — is the host's business.

mod attach;
pub mod bootstrap;
pub mod core;
mod history;
mod host;
mod link_flow;
pub mod linking;
pub use link_flow::{LinkOfferView, PendingLinkView};
mod peers;
pub use peers::conversation_for;
pub mod power_gate;
mod read_marks;
mod security_log;
mod verification;

#[cfg(test)]
mod e2e_test;
