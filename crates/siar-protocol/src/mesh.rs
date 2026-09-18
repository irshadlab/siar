//! next.md §29's `MeshEnvelope` — the routing-header wrapper Phase 7 of
//! the next.md rebuild found missing, and the reason
//! `apps/emergency-node` could log an unrecognized peer's frame but
//! couldn't do anything useful with it.
//!
//! [`v1::Envelope`] (this module's sibling) needs a live session —
//! pre-shared X25519 keys from a `PeerTicket` — before
//! `MessageService::handle_incoming` can decrypt anything at all. A
//! relay/DTN node forwarding for a stranger it's never been introduced
//! to can't meet that requirement, and per next.md §74 it shouldn't
//! need to: "a completely unknown phone may be trusted to carry
//! ciphertext... without being trusted to read messages."
//!
//! [`MeshEnvelope`] is the fix: `ciphertext` is typically an already-
//! encoded `WireMessage::V1` from a real session between the *original*
//! sender and the *real* destination — this module never looks inside
//! it, same "never looks inside `payload`" rule `v1::Envelope` already
//! documents for itself. A relay only ever needs `destination` (to
//! decide whether/where to forward) and `hop_limit`/`expires_at` (to
//! decide whether to bother at all) — it never needs a session, and
//! never gets one.

use serde::{Deserialize, Serialize};
use siar_domain::{DeviceId, MessageId, MessagePriority};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshEnvelope {
    pub id: MessageId,
    /// next.md §71 wants this to be an opaque, short-lived routing tag
    /// rather than a stable device identity ("avoid carrying obvious
    /// `recipient_phone_number`... instead use opaque routing
    /// identifiers"). This is a raw `DeviceId` for now, not that.
    /// `DeviceId` is already an opaque random identifier — not a phone
    /// number or display name — which is real progress over the doc's
    /// worst-case example, but it's still a *stable* identifier a relay
    /// could correlate across multiple mesh encounters over time,
    /// which §71's rotating-tag design exists specifically to prevent.
    /// A genuine rotating tag needs `siar-crypto` work (HKDF over a
    /// short-lived epoch secret, next.md §72) this pass doesn't
    /// attempt — flagged as a follow-up, not silently downgraded to
    /// "good enough."
    pub destination: DeviceId,
    /// Opaque `u64` ticks, caller supplies `now`, no wall clock
    /// read in this crate (for reproducible testing and monotonic timestamps).
    pub created_at: u64,
    pub expires_at: u64,
    pub hop_limit: u8,
    pub priority: MessagePriority,
    /// A hash of `ciphertext` — corruption detection for a possibly-
    /// lossy multi-hop path, the whole-bundle-level counterpart to
    /// `siar_transport_ble::fragment`'s per-fragment `checksum16`. This
    /// crate deliberately doesn't compute or verify it (no hashing
    /// dependency added here) — same "wire format carries bytes, a
    /// higher layer decides what they mean" boundary `v1::Envelope`'s
    /// own `payload` field already draws; whoever handles a received
    /// `MeshEnvelope` is responsible for actually checking this against
    /// `ciphertext` before trusting it.
    pub payload_hash: [u8; 32],
    /// Usually an encoded `WireMessage::V1(Envelope)` from a real
    /// end-to-end session between the original sender and
    /// `destination` — see this module's top doc comment.
    pub ciphertext: Vec<u8>,
}

impl MeshEnvelope {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    /// next.md §30: every forward decrements the hop count; at zero,
    /// drop. Contract mirrors the DTN forwarding rule (`None` means "stop,
    /// don't forward further").
    pub fn forwarded(mut self) -> Option<Self> {
        if self.hop_limit == 0 {
            return None;
        }
        self.hop_limit -= 1;
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(hop_limit: u8) -> MeshEnvelope {
        MeshEnvelope {
            id: MessageId::new(),
            destination: DeviceId::new(),
            created_at: 0,
            expires_at: 100,
            hop_limit,
            priority: MessagePriority::Normal,
            payload_hash: [0u8; 32],
            ciphertext: vec![1, 2, 3],
        }
    }

    #[test]
    fn is_expired_true_once_now_reaches_expires_at() {
        let envelope = envelope(4);
        assert!(!envelope.is_expired(99));
        assert!(envelope.is_expired(100));
    }

    #[test]
    fn forwarded_decrements_until_zero_then_stops() {
        let envelope = envelope(1);
        let envelope = envelope
            .forwarded()
            .expect("hop_limit 1 -> 0 should still forward");
        assert_eq!(envelope.hop_limit, 0);
        assert!(envelope.forwarded().is_none());
    }
}
