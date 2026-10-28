//! §205.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The runtime's error model. Serializable: it crosses the IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RuntimeError {
    #[error("runtime is not ready")]
    NotReady,
    #[error("runtime is read-only")]
    ReadOnly,
    #[error("unauthorized")]
    Unauthorized,
    #[error("unsupported")]
    Unsupported,
    #[error("busy")]
    Busy,
    #[error("storage error")]
    Storage,
    #[error("identity error")]
    Identity,
    #[error("network error")]
    Network,
    #[error("ipc error: {0}")]
    Ipc(String),
    #[error("invalid command: {0}")]
    InvalidCommand(String),
    #[error("cancelled")]
    Cancelled,
}
