//! §59 lazy extension negotiation, §60 hierarchical negotiation, and the
//! per-extension isolation of §107.
//!
//! ```text
//! Core session            → each side advertises only a tiny
//!                           (namespace, version) *summary* per extension
//!   ↓
//! extension support       → intersection of summaries, computed locally
//!   ↓
//! feature subset          → full capability set exchanged ON DEMAND,
//!   ↓                       the first time that extension is needed
//! operation parameters    → e.g. codec choice when a call actually starts
//! ```
//! A connection that never uses calls never pays for the codec catalogue.
//! Each extension has its own state, so one failing (or never-needed)
//! extension does not disturb another, and a failure classed as fatal by
//! [`crate::failure::classify`] terminates the whole session.

use std::collections::BTreeMap;

use crate::error::CapabilityNegotiationError;
use crate::extension::ExtensionNegotiator;
use crate::failure::{classify, FailureAction, NegotiationStage};
use crate::id::CapabilityNamespace;
use crate::policy::CapabilityPolicy;
use crate::set::CapabilitySet;
use crate::version::CapabilityVersion;

pub const MAX_EXTENSION_SUMMARIES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionSummary {
    pub namespace: CapabilityNamespace,
    pub version: CapabilityVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CoreAdvertisementError {
    #[error("more than {MAX_EXTENSION_SUMMARIES} extension summaries")]
    TooMany,
    #[error("the same extension is listed twice")]
    Duplicate,
}

/// The per-connection payload of the hierarchical scheme: a handful of
/// bytes per extension, regardless of how large each extension's own
/// capability set is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreAdvertisement {
    summaries: Vec<ExtensionSummary>,
}

impl CoreAdvertisement {
    pub fn new(mut summaries: Vec<ExtensionSummary>) -> Result<Self, CoreAdvertisementError> {
        if summaries.len() > MAX_EXTENSION_SUMMARIES {
            return Err(CoreAdvertisementError::TooMany);
        }
        summaries.sort_by_key(|s| s.namespace);
        if summaries
            .windows(2)
            .any(|w| w[0].namespace == w[1].namespace)
        {
            return Err(CoreAdvertisementError::Duplicate);
        }
        Ok(Self { summaries })
    }

    pub fn summaries(&self) -> &[ExtensionSummary] {
        &self.summaries
    }

    /// Upper bound on the serialized size: 16 bytes per summary.
    pub fn wire_size_bound(&self) -> usize {
        self.summaries.len() * 16
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionState {
    NotNegotiated,
    Negotiating,
    Ready(CapabilitySet),
    /// The peer does not offer this extension (or a compatible version).
    Unsupported,
    Failed(CapabilityNegotiationError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LazyError {
    #[error("the session has been terminated")]
    SessionTerminated,
    #[error("the peer does not support this extension")]
    UnsupportedByPeer,
    #[error("this extension failed earlier; reset it explicitly to retry")]
    PreviouslyFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginOutcome {
    /// Send this extension's capability set now.
    SendAdvertisement,
    AlreadyReady,
    /// Someone already started it; wait for that to finish.
    InProgress,
}

#[derive(Debug)]
pub struct HierarchicalSession {
    terminated: bool,
    supported: BTreeMap<CapabilityNamespace, CapabilityVersion>,
    states: BTreeMap<CapabilityNamespace, ExtensionState>,
}

static NOT_NEGOTIATED: ExtensionState = ExtensionState::NotNegotiated;

impl HierarchicalSession {
    /// Core step: compute which extensions both sides can possibly use.
    pub fn establish(local: &CoreAdvertisement, remote: &CoreAdvertisement) -> Self {
        let mut supported = BTreeMap::new();
        for l in local.summaries() {
            if let Some(r) = remote
                .summaries()
                .iter()
                .find(|r| r.namespace == l.namespace)
            {
                if let Some(v) = l.version.negotiate(r.version) {
                    supported.insert(l.namespace, v);
                }
            }
        }
        Self {
            terminated: false,
            supported,
            states: BTreeMap::new(),
        }
    }

    pub fn supported_extensions(
        &self,
    ) -> impl Iterator<Item = (&CapabilityNamespace, &CapabilityVersion)> {
        self.supported.iter()
    }

    pub fn is_terminated(&self) -> bool {
        self.terminated
    }

    pub fn state(&self, ns: &CapabilityNamespace) -> &ExtensionState {
        self.states.get(ns).unwrap_or(&NOT_NEGOTIATED)
    }

    /// Start (or join) negotiation for an extension the first time it is
    /// needed.
    pub fn begin(&mut self, ns: CapabilityNamespace) -> Result<BeginOutcome, LazyError> {
        if self.terminated {
            return Err(LazyError::SessionTerminated);
        }
        if !self.supported.contains_key(&ns) {
            self.states.insert(ns, ExtensionState::Unsupported);
            return Err(LazyError::UnsupportedByPeer);
        }
        match self.states.get(&ns) {
            Some(ExtensionState::Ready(_)) => Ok(BeginOutcome::AlreadyReady),
            Some(ExtensionState::Negotiating) => Ok(BeginOutcome::InProgress),
            Some(ExtensionState::Failed(_)) => Err(LazyError::PreviouslyFailed),
            Some(ExtensionState::Unsupported) => Err(LazyError::UnsupportedByPeer),
            Some(ExtensionState::NotNegotiated) | None => {
                self.states.insert(ns, ExtensionState::Negotiating);
                Ok(BeginOutcome::SendAdvertisement)
            }
        }
    }

    /// Record the result. A failure only affects `ns` unless
    /// [`classify`] says it is session-fatal.
    pub fn finish(
        &mut self,
        ns: CapabilityNamespace,
        result: Result<CapabilitySet, CapabilityNegotiationError>,
    ) -> FailureAction {
        // A result is only meaningful for a negotiation we started. A
        // stray or late one (an extension never begun, one the peer does
        // not support, one that already finished, or any result after the
        // session ended) must not change state: otherwise an unsupported
        // extension could be made "Ready", or a forged error could
        // terminate a healthy session.
        if self.terminated || !matches!(self.states.get(&ns), Some(ExtensionState::Negotiating)) {
            return FailureAction::Ignore;
        }
        match result {
            Ok(set) => {
                self.states.insert(ns, ExtensionState::Ready(set));
                FailureAction::Ignore
            }
            Err(e) => {
                let action = classify(&e, NegotiationStage::Extension);
                if action == FailureAction::TerminateSession {
                    self.terminated = true;
                }
                if action != FailureAction::Ignore {
                    self.states.insert(ns, ExtensionState::Failed(e));
                } else {
                    // cancelled: back to "not yet negotiated"
                    self.states.insert(ns, ExtensionState::NotNegotiated);
                }
                action
            }
        }
    }

    /// Allow a failed extension to be retried (e.g. after the user
    /// changed a policy).
    pub fn reset(&mut self, ns: &CapabilityNamespace) {
        if matches!(
            self.states.get(ns),
            Some(ExtensionState::Failed(_) | ExtensionState::Unsupported)
        ) && self.supported.contains_key(ns)
        {
            self.states.insert(*ns, ExtensionState::NotNegotiated);
        }
    }

    /// The negotiated set, only while the extension is Ready and the
    /// session is alive.
    pub fn get(&self, ns: &CapabilityNamespace) -> Option<&CapabilitySet> {
        if self.terminated {
            return None;
        }
        match self.states.get(ns) {
            Some(ExtensionState::Ready(s)) => Some(s),
            _ => None,
        }
    }

    pub fn is_usable(&self, ns: &CapabilityNamespace) -> bool {
        self.get(ns).is_some()
    }

    /// Run a whole on-demand negotiation with an [`ExtensionNegotiator`].
    pub fn negotiate_extension<N: ExtensionNegotiator>(
        &mut self,
        ns: CapabilityNamespace,
        negotiator: &N,
        remote: &CapabilitySet,
        policy: &CapabilityPolicy,
    ) -> Result<FailureAction, LazyError> {
        match self.begin(ns)? {
            BeginOutcome::AlreadyReady | BeginOutcome::InProgress => Ok(FailureAction::Ignore),
            BeginOutcome::SendAdvertisement => {
                Ok(self.finish(ns, negotiator.negotiate(remote, policy)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{CapabilityDescriptor, CapabilityParameters, CapabilityRequirement};
    use crate::id::CapabilityId;
    use crate::registry::CapabilityRegistry;

    use CapabilityNamespace::{Dtn, Files, Media, Security};

    fn sum(ns: CapabilityNamespace, major: u16) -> ExtensionSummary {
        ExtensionSummary {
            namespace: ns,
            version: CapabilityVersion::new(major, 0),
        }
    }

    fn session() -> HierarchicalSession {
        let local =
            CoreAdvertisement::new(vec![sum(Files, 1), sum(Media, 1), sum(Dtn, 1)]).unwrap();
        let remote =
            CoreAdvertisement::new(vec![sum(Media, 1), sum(Files, 1), sum(Dtn, 2)]).unwrap();
        HierarchicalSession::establish(&local, &remote)
    }

    struct Fixed {
        advert: CapabilitySet,
        reg: CapabilityRegistry,
    }
    impl ExtensionNegotiator for Fixed {
        fn advertise(&self) -> CapabilitySet {
            self.advert.clone()
        }
        fn registry(&self) -> &CapabilityRegistry {
            &self.reg
        }
    }

    fn set_of(ns: CapabilityNamespace, req: CapabilityRequirement) -> CapabilitySet {
        let mut s = CapabilitySet::new();
        s.insert(CapabilityDescriptor::new(
            CapabilityId::new(ns, 1),
            CapabilityVersion::new(1, 0),
            req,
            CapabilityParameters::None,
        ))
        .unwrap();
        s
    }

    #[test]
    fn core_exchange_is_tiny_and_bounded() {
        let big: Vec<_> = (0..=MAX_EXTENSION_SUMMARIES as u32)
            .map(|i| {
                sum(
                    CapabilityNamespace::Application(crate::id::NamespaceId(i)),
                    1,
                )
            })
            .collect();
        assert_eq!(
            CoreAdvertisement::new(big).unwrap_err(),
            CoreAdvertisementError::TooMany
        );
        assert_eq!(
            CoreAdvertisement::new(vec![sum(Files, 1), sum(Files, 2)]).unwrap_err(),
            CoreAdvertisementError::Duplicate
        );
        let a = CoreAdvertisement::new(vec![sum(Files, 1), sum(Media, 1), sum(Dtn, 1)]).unwrap();
        assert_eq!(
            a.wire_size_bound(),
            48,
            "three extensions cost a few dozen bytes, however big their sets are"
        );
    }

    #[test]
    fn support_is_the_version_compatible_intersection_computed_locally() {
        let s = session();
        let names: Vec<_> = s.supported_extensions().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![Files, Media],
            "dtn majors differ -> unsupported"
        );
        assert_eq!(
            *s.state(&Files),
            ExtensionState::NotNegotiated,
            "nothing negotiated yet: lazy"
        );
    }

    #[test]
    fn nothing_is_negotiated_until_needed_then_exactly_once() {
        let mut s = session();
        assert_eq!(s.begin(Media), Ok(BeginOutcome::SendAdvertisement));
        assert_eq!(
            s.begin(Media),
            Ok(BeginOutcome::InProgress),
            "a second caller joins, it does not re-send"
        );
        assert_eq!(
            s.finish(Media, Ok(set_of(Media, CapabilityRequirement::Optional))),
            FailureAction::Ignore
        );
        assert_eq!(s.begin(Media), Ok(BeginOutcome::AlreadyReady));
        assert!(s.is_usable(&Media));
        assert!(
            !s.is_usable(&Files),
            "untouched extensions stay un-negotiated"
        );
    }

    #[test]
    fn unsupported_extensions_fail_cleanly_without_a_round_trip() {
        let mut s = session();
        assert_eq!(s.begin(Dtn), Err(LazyError::UnsupportedByPeer));
        assert_eq!(*s.state(&Dtn), ExtensionState::Unsupported);
        assert_eq!(
            s.begin(CapabilityNamespace::Presence),
            Err(LazyError::UnsupportedByPeer)
        );
    }

    #[test]
    fn one_failed_extension_does_not_break_the_others() {
        let mut s = session();
        s.begin(Media).unwrap();
        let action = s.finish(
            Media,
            Err(CapabilityNegotiationError::MissingRequired(
                CapabilityId::new(Media, 9),
            )),
        );
        assert_eq!(action, FailureAction::DisableExtension);
        assert!(!s.is_terminated());
        assert!(matches!(s.state(&Media), ExtensionState::Failed(_)));
        s.begin(Files).unwrap();
        s.finish(Files, Ok(set_of(Files, CapabilityRequirement::Optional)));
        assert!(
            s.is_usable(&Files) && !s.is_usable(&Media),
            "calls fail, files work"
        );
        assert_eq!(
            s.begin(Media),
            Err(LazyError::PreviouslyFailed),
            "no retry storm"
        );
        s.reset(&Media);
        assert_eq!(
            s.begin(Media),
            Ok(BeginOutcome::SendAdvertisement),
            "explicit reset allows a retry"
        );
    }

    #[test]
    fn a_security_failure_terminates_the_whole_session() {
        let mut s = session();
        s.begin(Files).unwrap();
        s.finish(Files, Ok(set_of(Files, CapabilityRequirement::Optional)));
        s.begin(Media).unwrap();
        assert_eq!(
            s.finish(Media, Err(CapabilityNegotiationError::DowngradeSuspected)),
            FailureAction::TerminateSession
        );
        assert!(s.is_terminated());
        assert!(
            !s.is_usable(&Files),
            "even already-ready extensions are unusable on a terminated session"
        );
        assert_eq!(s.begin(Files), Err(LazyError::SessionTerminated));
    }

    #[test]
    fn a_result_for_an_extension_that_was_never_begun_is_ignored() {
        let mut s = session();
        // unsupported (dtn: majors differ) and never-begun (files) extensions
        assert_eq!(
            s.finish(Dtn, Ok(set_of(Dtn, CapabilityRequirement::Optional))),
            FailureAction::Ignore
        );
        assert!(
            !s.is_usable(&Dtn),
            "an unsupported extension cannot be made Ready by a stray result"
        );
        assert_eq!(
            s.finish(Files, Ok(set_of(Files, CapabilityRequirement::Optional))),
            FailureAction::Ignore
        );
        assert!(
            !s.is_usable(&Files),
            "nor can one that was never negotiated"
        );
        // a forged fatal error for something we never started cannot kill the session
        assert_eq!(
            s.finish(Files, Err(CapabilityNegotiationError::DowngradeSuspected)),
            FailureAction::Ignore
        );
        assert!(!s.is_terminated());
        assert_eq!(*s.state(&Files), ExtensionState::NotNegotiated);
    }

    #[test]
    fn a_second_result_after_completion_or_after_termination_is_ignored() {
        let mut s = session();
        s.begin(Files).unwrap();
        s.finish(Files, Ok(set_of(Files, CapabilityRequirement::Optional)));
        // duplicate / replayed completion cannot overwrite the negotiated set or fail it
        assert_eq!(
            s.finish(Files, Err(CapabilityNegotiationError::LimitExceeded)),
            FailureAction::Ignore
        );
        assert!(s.is_usable(&Files));
        s.begin(Media).unwrap();
        s.finish(Media, Err(CapabilityNegotiationError::TranscriptMismatch));
        assert!(s.is_terminated());
        assert_eq!(
            s.finish(Media, Ok(set_of(Media, CapabilityRequirement::Optional))),
            FailureAction::Ignore
        );
        assert!(!s.is_usable(&Media) && !s.is_usable(&Files));
    }

    #[test]
    fn cancellation_returns_to_not_negotiated() {
        let mut s = session();
        s.begin(Files).unwrap();
        assert_eq!(
            s.finish(Files, Err(CapabilityNegotiationError::Cancelled)),
            FailureAction::Ignore
        );
        assert_eq!(*s.state(&Files), ExtensionState::NotNegotiated);
        assert_eq!(s.begin(Files), Ok(BeginOutcome::SendAdvertisement));
    }

    #[test]
    fn negotiate_extension_drives_a_real_negotiator_end_to_end() {
        let mut s = session();
        let n = Fixed {
            advert: set_of(Files, CapabilityRequirement::Optional),
            reg: CapabilityRegistry::new(),
        };
        let remote = set_of(Files, CapabilityRequirement::Optional);
        let r = s.negotiate_extension(Files, &n, &remote, &CapabilityPolicy::new());
        assert_eq!(r, Ok(FailureAction::Ignore));
        assert!(s.is_usable(&Files));
        // a required capability the registry has never heard of => extension-local failure
        let strict = Fixed {
            advert: set_of(Media, CapabilityRequirement::Required),
            reg: CapabilityRegistry::new(),
        };
        assert_eq!(
            s.negotiate_extension(
                Media,
                &strict,
                &set_of(Media, CapabilityRequirement::Optional),
                &CapabilityPolicy::new()
            ),
            Ok(FailureAction::DisableExtension)
        );
        assert!(s.is_usable(&Files) && !s.is_usable(&Media));
        assert_eq!(
            s.negotiate_extension(Security, &n, &remote, &CapabilityPolicy::new()),
            Err(LazyError::UnsupportedByPeer)
        );
    }
}
