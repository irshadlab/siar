//! §34 "Session Recovery", §35 "Session Resumption Tokens", §36
//! "Capability Recovery".
//!
//! One theme: nothing negotiated or authenticated in a previous
//! process lifetime is trusted by the next one. §34 is enforced
//! structurally with an epoch; §35 is a validation gate that defaults
//! to *off* ("start without complex resumption if unnecessary"); §36
//! is a wrapper that can show a cached hint but cannot hand back an
//! authoritative contract.

use crate::shutdown_marker::RuntimeGeneration;
use serde::{Deserialize, Serialize};
use siar_domain::DeviceId;
use std::collections::HashSet;
use std::fmt;

// ---------------------------------------------------------------------
// §34
// ---------------------------------------------------------------------

/// Handle to a live authenticated session. It carries the runtime
/// generation that created it, so after a crash (new generation) every
/// handle from the old process is invalid *by construction* — there is
/// no table to forget to clear. Deliberately not `Serialize`: §34 says
/// do not persist raw live session objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionHandle {
    epoch: RuntimeGeneration,
    slot: u64,
}

impl SessionHandle {
    pub const fn new(epoch: RuntimeGeneration, slot: u64) -> Self {
        Self { epoch, slot }
    }

    pub const fn slot(&self) -> u64 {
        self.slot
    }

    /// True only for handles minted by the *current* runtime lifetime.
    #[inline]
    pub const fn is_valid_in(&self, current: RuntimeGeneration) -> bool {
        self.epoch.0 == current.0
    }
}

// ---------------------------------------------------------------------
// §35
// ---------------------------------------------------------------------

/// §35's policy knob. Default is [`ResumptionPolicy::Disabled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumptionPolicy {
    #[default]
    Disabled,
    Enabled,
}

/// Persisted resumption material with the four §35 attributes:
/// expiry, revocation binding, account-generation binding, and a
/// record of whether it came from secure storage. `secret` is opaque
/// and never printed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumptionTicket {
    pub peer: DeviceId,
    pub expires_at_millis: u64,
    pub account_generation: u64,
    pub loaded_from_secure_store: bool,
    secret: Vec<u8>,
}

impl ResumptionTicket {
    pub fn new(
        peer: DeviceId,
        expires_at_millis: u64,
        account_generation: u64,
        loaded_from_secure_store: bool,
        secret: Vec<u8>,
    ) -> Self {
        Self {
            peer,
            expires_at_millis,
            account_generation,
            loaded_from_secure_store,
            secret,
        }
    }

    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
}

impl fmt::Debug for ResumptionTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResumptionTicket")
            .field("peer", &self.peer)
            .field("expires_at_millis", &self.expires_at_millis)
            .field("account_generation", &self.account_generation)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TicketRejection {
    #[error("session resumption is disabled")]
    Disabled,
    #[error("ticket expired")]
    Expired,
    #[error("peer device has been revoked")]
    PeerRevoked,
    #[error("ticket bound to a different account generation")]
    GenerationMismatch,
    #[error("ticket material was not loaded from secure storage")]
    NotFromSecureStorage,
}

pub struct ResumptionContext<'a> {
    pub policy: ResumptionPolicy,
    pub now_millis: u64,
    pub current_account_generation: u64,
    pub revoked: &'a HashSet<DeviceId>,
}

/// Every §35 condition must hold; any failure means a full fresh
/// handshake. Check order puts the cheapest/most fundamental first.
pub fn validate_ticket(
    t: &ResumptionTicket,
    ctx: &ResumptionContext<'_>,
) -> Result<(), TicketRejection> {
    if ctx.policy == ResumptionPolicy::Disabled {
        return Err(TicketRejection::Disabled);
    }
    if !t.loaded_from_secure_store {
        return Err(TicketRejection::NotFromSecureStorage);
    }
    if ctx.revoked.contains(&t.peer) {
        return Err(TicketRejection::PeerRevoked);
    }
    if t.account_generation != ctx.current_account_generation {
        return Err(TicketRejection::GenerationMismatch);
    }
    if ctx.now_millis >= t.expires_at_millis {
        return Err(TicketRejection::Expired);
    }
    Ok(())
}

// ---------------------------------------------------------------------
// §36
// ---------------------------------------------------------------------

/// A cached peer-capability record. It exposes the data for display
/// ([`CapabilityHint::for_display`]) and nothing that could be
/// mistaken for a negotiated contract: there is deliberately no
/// conversion into an authoritative value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityHint<C> {
    peer: DeviceId,
    cached: C,
}

impl<C> CapabilityHint<C> {
    pub fn new(peer: DeviceId, cached: C) -> Self {
        Self { peer, cached }
    }

    pub fn peer(&self) -> DeviceId {
        self.peer
    }

    /// §36: "cache may help UI".
    pub fn for_display(&self) -> &C {
        &self.cached
    }

    /// §36: "new session must renegotiate" — always true, for every
    /// session created after the hint was written.
    pub const fn requires_renegotiation(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> ResumptionTicket {
        ResumptionTicket::new(DeviceId::new(), 1_000, 7, true, vec![1, 2, 3])
    }

    fn ctx<'a>(revoked: &'a HashSet<DeviceId>) -> ResumptionContext<'a> {
        ResumptionContext {
            policy: ResumptionPolicy::Enabled,
            now_millis: 500,
            current_account_generation: 7,
            revoked,
        }
    }

    #[test]
    fn pre_crash_sessions_are_invalid_after_restart() {
        let before = RuntimeGeneration(3);
        let h = SessionHandle::new(before, 42);
        assert!(h.is_valid_in(before));
        assert!(!h.is_valid_in(RuntimeGeneration(4)));
    }

    #[test]
    fn resumption_is_off_by_default() {
        assert_eq!(ResumptionPolicy::default(), ResumptionPolicy::Disabled);
        let rev = HashSet::new();
        let mut c = ctx(&rev);
        c.policy = ResumptionPolicy::default();
        assert_eq!(
            validate_ticket(&ticket(), &c),
            Err(TicketRejection::Disabled)
        );
    }

    #[test]
    fn a_fully_valid_ticket_is_accepted() {
        let rev = HashSet::new();
        assert_eq!(validate_ticket(&ticket(), &ctx(&rev)), Ok(()));
    }

    #[test]
    fn each_binding_failure_is_rejected() {
        let rev = HashSet::new();
        let mut t = ticket();
        t.loaded_from_secure_store = false;
        assert_eq!(
            validate_ticket(&t, &ctx(&rev)),
            Err(TicketRejection::NotFromSecureStorage)
        );

        let t = ticket();
        let revoked: HashSet<_> = [t.peer].into();
        assert_eq!(
            validate_ticket(&t, &ctx(&revoked)),
            Err(TicketRejection::PeerRevoked)
        );

        let mut c = ctx(&rev);
        c.current_account_generation = 8;
        assert_eq!(
            validate_ticket(&ticket(), &c),
            Err(TicketRejection::GenerationMismatch)
        );

        let mut c = ctx(&rev);
        c.now_millis = 1_000; // expiry is exclusive of validity
        assert_eq!(
            validate_ticket(&ticket(), &c),
            Err(TicketRejection::Expired)
        );
    }

    #[test]
    fn ticket_debug_never_leaks_the_secret() {
        let s = format!("{:?}", ticket());
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("[1, 2, 3]"));
    }

    #[test]
    fn capability_hint_displays_but_always_requires_renegotiation() {
        let h = CapabilityHint::new(DeviceId::new(), vec!["calls", "files"]);
        assert_eq!(h.for_display().len(), 2);
        assert!(h.requires_renegotiation());
    }
}
