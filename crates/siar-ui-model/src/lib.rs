//! # siar-ui-model — renderer-independent UI state
//!
//! `Slices` (everything a UI shows), `StateHub` (shared state + command
//! channel) and the plain-language power description. **No GUI toolkit, no
//! networking, no storage** — so the application core can be hosted by a
//! window (`siar-ui-slint`), a daemon, or a test, and only the window pays
//! for Slint.

pub mod hub;
pub mod power;
pub mod slices;

pub use hub::{StateHub, Wake};
pub use power::{describe as describe_power, PowerStatus};
pub use slices::{
    ContactSafety, EventView, HistoryPaging, LocalIdentity, Notice, NoticeLevel, Slices,
    VerificationState,
};
