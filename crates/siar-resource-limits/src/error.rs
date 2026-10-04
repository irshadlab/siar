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

/// §170's four classes, distinct from [`Retryability`] (which says *when*
/// an automatic retry makes sense): this says *who* has to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Capacity or rate will come back; retrying is reasonable.
    Retryable,
    /// Retrying won't help until a person (free up space) or a policy
    /// (raise a quota) changes something.
    UserActionOrPolicy,
    /// This exact request can never succeed.
    PermanentForRequest,
    /// The operation was cancelled; do not resume it implicitly.
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    // ---- §169's nine named variants (the first block) ----
    #[error("hard limit exceeded")]
    HardLimitExceeded,
    #[error("busy")]
    Busy,
    #[error("deferred until resources free up")]
    Deferred,
    #[error("storage full")]
    StorageFull,
    #[error("memory pressure")]
    MemoryPressure,
    #[error("too many connections")]
    TooManyConnections,
    #[error("too many streams")]
    TooManyStreams,
    // `QuotaExceeded` and `Cancelled` are the other two named variants;
    // they live below beside the finer-grained ones.
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
            Self::HardLimitExceeded | Self::StorageFull => Retryability::Never,
            Self::Busy | Self::Deferred | Self::TooManyConnections | Self::TooManyStreams => {
                Retryability::WhenCapacityFrees
            }
            Self::MemoryPressure => Retryability::AfterBackoff,
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

impl ResourceError {
    /// §170 classification.
    pub fn retry_class(&self) -> RetryClass {
        match self {
            Self::Busy
            | Self::Deferred
            | Self::MemoryPressure
            | Self::TooManyConnections
            | Self::TooManyStreams
            | Self::BudgetExhausted(_)
            | Self::QuotaExceeded(_)
            | Self::RateLimited
            | Self::Overloaded(_) => RetryClass::Retryable,
            Self::StorageFull => RetryClass::UserActionOrPolicy,
            Self::HardLimitExceeded
            | Self::DeadlineExpired
            | Self::MalformedRequest(_)
            | Self::Unsafe(_) => RetryClass::PermanentForRequest,
            Self::Cancelled => RetryClass::Terminal,
        }
    }
}

impl From<crate::admission_pipeline::ConnectionReject> for ResourceError {
    fn from(e: crate::admission_pipeline::ConnectionReject) -> Self {
        use crate::admission_pipeline::ConnectionReject as C;
        match e {
            C::NoSlot(_) => Self::TooManyConnections,
            C::HandshakeBudget(_) => Self::Busy,
            C::SourceRate | C::Storm(_) => Self::RateLimited,
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
    fn spec_170_classification() {
        assert_eq!(ResourceError::Busy.retry_class(), RetryClass::Retryable);
        assert_eq!(
            ResourceError::StorageFull.retry_class(),
            RetryClass::UserActionOrPolicy
        );
        assert_eq!(
            ResourceError::HardLimitExceeded.retry_class(),
            RetryClass::PermanentForRequest
        );
        assert_eq!(ResourceError::Cancelled.retry_class(), RetryClass::Terminal);
    }

    #[test]
    fn the_two_classifications_agree() {
        // A "retryable" error must have some automatic-retry story; a
        // permanent/terminal/user-action one must never ask for one.
        let all = [
            ResourceError::HardLimitExceeded,
            ResourceError::Busy,
            ResourceError::Deferred,
            ResourceError::StorageFull,
            ResourceError::MemoryPressure,
            ResourceError::TooManyConnections,
            ResourceError::TooManyStreams,
            ResourceError::BudgetExhausted(ResourceKind::Memory),
            ResourceError::QuotaExceeded(ResourceKind::Storage),
            ResourceError::RateLimited,
            ResourceError::Overloaded(OverloadMode::Shedding),
            ResourceError::DeadlineExpired,
            ResourceError::Cancelled,
            ResourceError::MalformedRequest(ResourceKind::Memory),
            ResourceError::Unsafe(SafetyError::TooDeep(1)),
        ];
        for e in all {
            match e.retry_class() {
                RetryClass::Retryable => assert_ne!(e.retryability(), Retryability::Never, "{e:?}"),
                _ => assert_eq!(e.retryability(), Retryability::Never, "{e:?}"),
            }
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn connection_rejections_map_to_resource_errors() {
        use crate::admission_pipeline::ConnectionReject as C;
        use crate::connections::LimitDenied;
        assert_eq!(
            ResourceError::from(C::NoSlot(LimitDenied::Global)),
            ResourceError::TooManyConnections
        );
        assert_eq!(
            ResourceError::from(C::SourceRate),
            ResourceError::RateLimited
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
