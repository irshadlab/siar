//! # siar-ui-sync
//!
//! Lets a window mirror a core that lives in another process, **without**
//! serializing UI structs and without a second source of truth.
//!
//! * [`wire`] — explicit, versioned section types carrying *domain* data.
//! * [`engine`] — daemon side: capture → hash → delta, chunked and bounded.
//! * [`apply`] — window side: replay sections into the window's `Slices`,
//!   leaving selection/composer/filter/banner alone; gap detection.
//! * [`commands`] — the validated wire form of the commands the window sends.

pub mod apply;
pub mod client;
pub mod commands;
pub mod engine;
pub mod server;
pub mod wire;

#[cfg(test)]
mod tests;

pub use apply::{apply_delta, apply_snapshot, ApplyError, ClientCursor, Verdict};
pub use client::run_remote;
pub use commands::{WireCommand, WireCommandError};
pub use engine::{capture, SyncServer};
pub use server::UiSyncService;
pub use wire::{Section, SectionKey, SectionUpdate, SyncFrame, SYNC_TOPIC};
