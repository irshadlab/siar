//! §169-171: one typed resource error with an explicit retryability
//! verdict — no generic string errors, and callers never have to guess
//! whether retrying can help.

use crate::pressure::OverloadMode;
use crate::reservation::ReservationError;
use crate::safety::SafetyError;
use crate::types::ResourceKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retryability {
    /// Retrying the same request cannot succeed.
    Never,
    /// Capacity may free up soon; retry when a permit is released.
    WhenCapacityFrees,
    /// Retry only after a delay (rate limits, overload easing).
    AfterBackoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("{0:?} budget exhausted")]
    BudgetExhausted(ResourceKind),
    #[error("{0:?} quota exceeded")]
    QuotaExceeded(ResourceKind),
    #[error("request for {0:?} overflows")]
    MalformedRequest(ResourceKind),
    #[error("rate limit exceeded")]
    RateLimited,
    #[error("runtime overloaded ({0:?})")]
    Overloaded(OverloadMode),
    #[error("deadline expired")]
    DeadlineExpired,
    #[error("cancelled")]
    Cancelled,
    #[error("input rejected: {0}")]
    Unsafe(SafetyError),
}

impl ResourceError {
    pub fn retryability(&self) -> Retryability {
        match self {
            Self::BudgetExhausted(_) => Retryability::WhenCapacityFrees,
            Self::QuotaExceeded(_) | Self::RateLimited => Retryability::AfterBackoff,
            Self::Overloaded(OverloadMode::Normal) => Retryability::WhenCapacityFrees,
            Self::Overloaded(_) => Retryability::AfterBackoff,
            Self::DeadlineExpired
            | Self::Cancelled
            | Self::MalformedRequest(_)
            | Self::Unsafe(_) => Retryability::Never,
        }
    }
}

impl From<ReservationError> for ResourceError {
    fn from(e: ReservationError) -> Self {
        match e {
            ReservationError::Exhausted { kind, .. } => Self::BudgetExhausted(kind),
            // An overflowing request is malformed, not transient.
            ReservationError::Overflow(kind) => Self::MalformedRequest(kind),
        }
    }
}

impl From<SafetyError> for ResourceError {
    fn from(e: SafetyError) -> Self {
        Self::Unsafe(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryability_matches_cause() {
        assert_eq!(
            ResourceError::BudgetExhausted(ResourceKind::Memory).retryability(),
            Retryability::WhenCapacityFrees
        );
        assert_eq!(
            ResourceError::RateLimited.retryability(),
            Retryability::AfterBackoff
        );
        assert_eq!(
            ResourceError::Overloaded(OverloadMode::CriticalOnly).retryability(),
            Retryability::AfterBackoff
        );
        assert_eq!(ResourceError::Cancelled.retryability(), Retryability::Never);
        assert_eq!(
            ResourceError::Unsafe(SafetyError::TooDeep(3)).retryability(),
            Retryability::Never
        );
    }

    #[test]
    fn conversions() {
        let e: ResourceError = ReservationError::Exhausted {
            kind: ResourceKind::Streams,
            requested: 1,
            available: 0,
        }
        .into();
        assert_eq!(e, ResourceError::BudgetExhausted(ResourceKind::Streams));
        let m: ResourceError = ReservationError::Overflow(ResourceKind::Memory).into();
        assert_eq!(m.retryability(), Retryability::Never);
    }
}
