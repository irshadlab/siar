//! §105 required-capability failure, §106 security failure, §107
//! extension isolation — the *consequence* policy for a failed
//! negotiation.
//!
//! The distinction the spec insists on:
//! - a **security** failure (a required security capability, a downgrade,
//!   a transcript mismatch, an unsupported core) ends the trusted session;
//! - any other failure is **local to the extension** it happened in:
//!   that extension is disabled, its operations fail cleanly, and the
//!   session, messaging and every other extension carry on.

use crate::error::CapabilityNegotiationError as E;
use crate::id::{CapabilityId, CapabilityNamespace};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiationStage {
    /// The core session advertisement.
    Core,
    /// A lazily negotiated extension.
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    /// Close the trusted session (§106).
    TerminateSession,
    /// Mark just this extension unusable; keep the session (§105, §107).
    DisableExtension,
    /// Nothing to do (the caller cancelled).
    Ignore,
}

fn is_session_critical(id: &CapabilityId) -> bool {
    matches!(
        id.namespace,
        CapabilityNamespace::Security | CapabilityNamespace::Core
    )
}

pub fn classify(err: &E, stage: NegotiationStage) -> FailureAction {
    match err {
        E::UnsupportedCore | E::DowngradeSuspected | E::TranscriptMismatch => {
            FailureAction::TerminateSession
        }
        E::SecurityPolicy(_) => FailureAction::TerminateSession,
        E::MissingRequired(id) | E::VersionMismatch(id) => {
            if is_session_critical(id) {
                FailureAction::TerminateSession
            } else {
                FailureAction::DisableExtension
            }
        }
        E::DependencyViolation {
            capability,
            missing,
        } => {
            if is_session_critical(capability) || is_session_critical(missing) {
                FailureAction::TerminateSession
            } else {
                FailureAction::DisableExtension
            }
        }
        E::Conflict(a, b) => {
            if is_session_critical(a) || is_session_critical(b) {
                FailureAction::TerminateSession
            } else {
                FailureAction::DisableExtension
            }
        }
        // A malformed *core* advertisement means we cannot trust the
        // peer's framing at all; a malformed extension set only spoils
        // that extension.
        E::Malformed => match stage {
            NegotiationStage::Core => FailureAction::TerminateSession,
            NegotiationStage::Extension => FailureAction::DisableExtension,
        },
        E::LimitExceeded => FailureAction::DisableExtension,
        E::Cancelled => FailureAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use NegotiationStage::*;

    fn cap(ns: CapabilityNamespace) -> CapabilityId {
        CapabilityId::new(ns, 1)
    }

    #[test]
    fn security_failures_end_the_session() {
        for e in [
            E::UnsupportedCore,
            E::DowngradeSuspected,
            E::TranscriptMismatch,
            E::SecurityPolicy(cap(CapabilityNamespace::Files)),
        ] {
            for st in [Core, Extension] {
                assert_eq!(classify(&e, st), FailureAction::TerminateSession, "{e:?}");
            }
        }
    }

    #[test]
    fn a_missing_required_security_or_core_capability_is_fatal_but_a_files_one_is_not() {
        assert_eq!(
            classify(
                &E::MissingRequired(cap(CapabilityNamespace::Security)),
                Extension
            ),
            FailureAction::TerminateSession
        );
        assert_eq!(
            classify(
                &E::VersionMismatch(cap(CapabilityNamespace::Core)),
                Extension
            ),
            FailureAction::TerminateSession
        );
        assert_eq!(
            classify(
                &E::MissingRequired(cap(CapabilityNamespace::Files)),
                Extension
            ),
            FailureAction::DisableExtension
        );
        assert_eq!(
            classify(
                &E::VersionMismatch(cap(CapabilityNamespace::Dtn)),
                Extension
            ),
            FailureAction::DisableExtension
        );
    }

    #[test]
    fn dependency_and_conflict_follow_the_most_critical_party() {
        let dep = |c, m| E::DependencyViolation {
            capability: cap(c),
            missing: cap(m),
        };
        assert_eq!(
            classify(
                &dep(CapabilityNamespace::Files, CapabilityNamespace::Files),
                Extension
            ),
            FailureAction::DisableExtension
        );
        assert_eq!(
            classify(
                &dep(CapabilityNamespace::Files, CapabilityNamespace::Security),
                Extension
            ),
            FailureAction::TerminateSession
        );
        assert_eq!(
            classify(
                &E::Conflict(
                    cap(CapabilityNamespace::Files),
                    cap(CapabilityNamespace::Dtn)
                ),
                Extension
            ),
            FailureAction::DisableExtension
        );
        assert_eq!(
            classify(
                &E::Conflict(
                    cap(CapabilityNamespace::Security),
                    cap(CapabilityNamespace::Dtn)
                ),
                Extension
            ),
            FailureAction::TerminateSession
        );
    }

    #[test]
    fn malformed_depends_on_where_it_happened() {
        assert_eq!(
            classify(&E::Malformed, Core),
            FailureAction::TerminateSession
        );
        assert_eq!(
            classify(&E::Malformed, Extension),
            FailureAction::DisableExtension
        );
    }

    #[test]
    fn limits_and_cancellation_never_kill_the_session() {
        assert_eq!(
            classify(&E::LimitExceeded, Core),
            FailureAction::DisableExtension
        );
        assert_eq!(classify(&E::Cancelled, Extension), FailureAction::Ignore);
    }
}
