//! Who each 1:1 conversation talks to.
//!
//! The Dioxus shell had one global `ActivePeer`, a fresh `ConversationId`
//! per incoming message, and dropped any frame while no peer was active.
//! Here every peer gets exactly one conversation, with an id derived
//! deterministically from the peer's transport identity — so it is stable
//! across restarts and across "pair" / "use contact" / "message arrived"
//! paths, and an incoming frame is routed by *who sent it*, not by whoever
//! happens to be selected.

use std::collections::HashMap;

use siar_domain::ConversationId;
use siar_messaging::PeerTicket;
use uuid::Uuid;

/// Stable per-peer conversation id: the first 16 bytes of the peer's
/// 32-byte transport public key. Collisions would need a 64-bit birthday
/// attack on keys the peer cannot choose freely, and the id only labels
/// local state, never authenticates anything.
pub fn conversation_for(ticket: &PeerTicket) -> ConversationId {
    let key = ticket.endpoint_addr.id.as_bytes();
    let mut b = [0u8; 16];
    b.copy_from_slice(&key[..16]);
    ConversationId::from_uuid(Uuid::from_bytes(b))
}

#[derive(Default)]
pub struct PeerRegistry {
    by_conversation: HashMap<ConversationId, PeerTicket>,
    by_endpoint: HashMap<iroh::EndpointId, ConversationId>,
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers (or refreshes the address of) a peer; returns its
    /// conversation. A refreshed ticket replaces the old one, so a peer
    /// whose relay/addresses changed keeps its conversation.
    pub fn register(&mut self, ticket: PeerTicket) -> ConversationId {
        let conversation = conversation_for(&ticket);
        self.by_endpoint
            .insert(ticket.endpoint_addr.id, conversation);
        self.by_conversation.insert(conversation, ticket);
        conversation
    }

    pub fn ticket(&self, conversation: ConversationId) -> Option<&PeerTicket> {
        self.by_conversation.get(&conversation)
    }

    pub fn conversation_of(&self, endpoint: &iroh::EndpointId) -> Option<ConversationId> {
        self.by_endpoint.get(endpoint).copied()
    }

    pub fn ticket_of_endpoint(&self, endpoint: &iroh::EndpointId) -> Option<&PeerTicket> {
        self.conversation_of(endpoint).and_then(|c| self.ticket(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(seed: u8) -> PeerTicket {
        let secret = iroh::SecretKey::from_bytes(&[seed; 32]);
        PeerTicket {
            endpoint_addr: iroh::EndpointAddr::new(secret.public()),
            x25519_public: [seed; 32],
            ed25519_verifying: [seed; 32],
        }
    }

    #[test]
    fn conversation_id_is_stable_and_distinct_per_peer() {
        assert_eq!(conversation_for(&ticket(1)), conversation_for(&ticket(1)));
        assert_ne!(conversation_for(&ticket(1)), conversation_for(&ticket(2)));
    }

    #[test]
    fn frames_route_by_sender_not_by_selection() {
        let mut reg = PeerRegistry::new();
        let (a, b) = (ticket(1), ticket(2));
        let (ca, cb) = (reg.register(a.clone()), reg.register(b.clone()));
        assert_ne!(ca, cb);
        assert_eq!(reg.conversation_of(&a.endpoint_addr.id), Some(ca));
        assert_eq!(reg.conversation_of(&b.endpoint_addr.id), Some(cb));
        assert_eq!(reg.conversation_of(&ticket(3).endpoint_addr.id), None);
    }

    #[test]
    fn re_registering_keeps_the_conversation_and_refreshes_the_ticket() {
        let mut reg = PeerRegistry::new();
        let mut t = ticket(7);
        let c1 = reg.register(t.clone());
        t.x25519_public = [9; 32];
        let c2 = reg.register(t.clone());
        assert_eq!(c1, c2);
        assert_eq!(reg.ticket(c1).unwrap().x25519_public, [9; 32]);
    }
}
