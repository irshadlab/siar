//! Error type for this crate. Policy evaluation itself never fails — an
//! unusable input degrades to the conservative answer — so errors only exist
//! at the edges: persistence and platform adapters.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum PowerError {
    /// A persisted deferral record could not be understood. The record is
    /// skipped, never guessed at: re-evaluation will recreate it if the
    /// owning subsystem still has the operation.
    #[error("persisted deferral record is invalid: {0}")]
    InvalidRecord(&'static str),
    /// The platform could not provide a reading (no sensor, permission).
    #[error("power source unavailable: {0}")]
    SourceUnavailable(String),
    /// The scheduler's bounded deferred set is full.
    #[error("deferred-work set is full ({0} entries)")]
    DeferredFull(usize),
    /// An operation id was submitted twice.
    #[error("operation already tracked")]
    Duplicate,
}

pub type Result<T> = core::result::Result<T, PowerError>;
