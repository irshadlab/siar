//! Admission control in front of `MessageService::handle_incoming`
//! (Part 28 §60-§62, §65): untrusted inbound frames are shed cheaply and
//! boundedly *before* any decryption or storage work happens.
//!
//! This wires `siar_abuse::Gate` to this crate's types. What it deliberately
//! does **not** do is deduplicate messages: a retransmission of a message whose
//! first processing failed (a crash before it was stored) must still be
//! accepted, and duplicates are already harmless because storage is
//! idempotent (`insert_if_new`). Duplicate suppression here would turn a
//! recoverable crash into a lost message.

use std::sync::{Arc, Mutex, PoisonError};

use siar_abuse::{Admit, Gate, GateConfig, Incoming, PeerKey, QuotaKind, Reject, Standing};
use siar_protocol::EnvelopeKind;

use crate::ticket::PeerTicket;

/// How the host classifies a peer (saved contact? verified?).
pub type StandingFn = Arc<dyn Fn(&PeerTicket) -> Standing + Send + Sync>;
pub type ClockFn = Arc<dyn Fn() -> u64 + Send + Sync>;

pub struct IngressGuard {
    gate: Mutex<Gate>,
    standing_of: StandingFn,
    clock: ClockFn,
}

/// A caller-scoped, domain-separated key (§35): never the raw transport id.
#[must_use]
pub fn peer_key(peer: &PeerTicket) -> PeerKey {
    let mut h = blake3::Hasher::new();
    h.update(b"siar/abuse/peer/v1");
    h.update(&peer.ed25519_verifying);
    let mut k = [0u8; 16];
    k.copy_from_slice(&h.finalize().as_bytes()[..16]);
    PeerKey(k)
}

/// Which quota an envelope draws from.
#[must_use]
pub const fn quota_for(kind: &EnvelopeKind) -> QuotaKind {
    match kind {
        EnvelopeKind::Text | EnvelopeKind::Attachment => QuotaKind::MessageRequest,
        // Call signalling and every group frame are session-setup style
        // traffic, the cheapest thing for a stranger to spam.
        _ => QuotaKind::Handshake,
    }
}

impl IngressGuard {
    #[must_use]
    pub fn new(cfg: GateConfig, standing_of: StandingFn, clock: ClockFn) -> Self {
        let now = clock();
        Self { gate: Mutex::new(Gate::new(cfg, now)), standing_of, clock }
    }

    /// Run the cheap-first pipeline for one envelope.
    pub fn check(&self, peer: &PeerTicket, kind: &EnvelopeKind, payload_len: usize, version: u16) -> Result<Admit, Reject> {
        let key = peer_key(peer);
        let standing = (self.standing_of)(peer);
        let now = (self.clock)();
        let mut gate = self.gate.lock().unwrap_or_else(PoisonError::into_inner);
        // Keep the gate's view of this peer in step with the host's.
        if gate.standing(key) != standing && standing != Standing::Blocked {
            gate.set_standing(key, standing);
        }
        gate.admit(
            &Incoming {
                peer: key,
                kind: quota_for(kind),
                len: payload_len,
                version: u8::try_from(version).unwrap_or(u8::MAX),
                id: None,
            },
            now,
        )
    }

    /// The user blocked this peer.
    pub fn block(&self, peer: &PeerTicket) -> bool {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner).block(peer_key(peer))
    }

    pub fn unblock(&self, peer: &PeerTicket) {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner).unblock(peer_key(peer));
    }

    #[must_use]
    pub fn stats(&self) -> siar_abuse::GateStats {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner).stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn ticket(n: u8) -> PeerTicket {
        let secret = iroh::SecretKey::from_str(&format!("{:02x}", n.max(1)).repeat(32)).unwrap();
        PeerTicket { endpoint_addr: iroh::EndpointAddr::new(secret.public()), x25519_public: [n; 32], ed25519_verifying: [n; 32] }
    }

    fn guard(standing: Standing) -> (IngressGuard, Arc<AtomicU64>) {
        let t = Arc::new(AtomicU64::new(0));
        let t2 = t.clone();
        (IngressGuard::new(GateConfig::default(), Arc::new(move |_| standing), Arc::new(move || t2.load(Ordering::SeqCst))), t)
    }

    #[test]
    fn contacts_are_not_throttled_but_strangers_are() {
        let (contact, _) = guard(Standing::Contact);
        let (stranger, _) = guard(Standing::Unknown);
        let p = ticket(1);
        let run = |g: &IngressGuard| (0..300).filter(|_| g.check(&p, &EnvelopeKind::Text, 100, 1).is_ok()).count();
        assert_eq!(run(&contact), 300, "message requests are unlimited for saved contacts");
        assert!(run(&stranger) <= 5, "an unknown peer gets a handful per hour");
    }

    #[test]
    fn many_fresh_strangers_share_one_budget() {
        let (g, _) = guard(Standing::Unknown);
        let admitted = (1..=200u8).filter(|n| g.check(&ticket(*n), &EnvelopeKind::Text, 100, 1).is_ok()).count();
        assert!(admitted <= 100, "{admitted}");
    }

    #[test]
    fn oversize_and_wrong_version_are_rejected_before_budget_is_spent() {
        let (g, _) = guard(Standing::Unknown);
        let p = ticket(2);
        assert_eq!(g.check(&p, &EnvelopeKind::Text, 10_000_000, 1), Err(Reject::TooLong));
        assert_eq!(g.check(&p, &EnvelopeKind::Text, 10, 999), Err(Reject::BadVersion));
        assert_eq!(g.check(&p, &EnvelopeKind::Text, 10, 1).unwrap().standing, Standing::Unknown);
    }

    #[test]
    fn blocking_wins_and_retransmissions_are_never_treated_as_duplicates() {
        let (g, _) = guard(Standing::Contact);
        let p = ticket(3);
        // The same message arriving twice (a retransmission) is admitted both times.
        assert!(g.check(&p, &EnvelopeKind::Text, 10, 1).is_ok());
        assert!(g.check(&p, &EnvelopeKind::Text, 10, 1).is_ok());
        g.block(&p);
        assert_eq!(g.check(&p, &EnvelopeKind::Text, 10, 1), Err(Reject::Blocked));
        g.unblock(&p);
        assert!(g.check(&p, &EnvelopeKind::Text, 10, 1).is_ok());
    }

    #[test]
    fn peer_keys_are_domain_separated_and_distinct() {
        assert_ne!(peer_key(&ticket(1)), peer_key(&ticket(2)));
        assert_eq!(peer_key(&ticket(1)), peer_key(&ticket(1)));
        assert_ne!(peer_key(&ticket(1)).0, [1u8; 16], "never the raw identifier");
    }

    #[test]
    fn envelope_kinds_map_to_the_right_quota() {
        assert_eq!(quota_for(&EnvelopeKind::Text), QuotaKind::MessageRequest);
        assert_eq!(quota_for(&EnvelopeKind::GroupEvent), QuotaKind::Handshake);
    }
}
