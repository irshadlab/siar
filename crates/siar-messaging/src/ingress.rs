//! Ingress admission (Part 28 §60-§62, §65): the cheap checks that run
//! *before* a frame is decrypted, stored or acknowledged.
//!
//! [`IngressGuard`] puts `siar_abuse::Gate` in front of the receive path.
//!
//! ## Design decisions
//!
//! * **Keyed on the authenticated transport identity, never on the envelope.**
//!   `Envelope::sender` is attacker-controlled text inside an unauthenticated
//!   frame; keying quotas on it would let a stranger burn a contact's budget
//!   by claiming to be them. The caller passes the QUIC-authenticated
//!   endpoint id (`IncomingFrame::from`), which cannot be forged.
//! * **Salted, one-way peer keys (§35).** The gate only ever sees a 16-byte
//!   keyed BLAKE3 of the endpoint id, with a per-process random salt, so
//!   nothing it logs or exposes is a reusable identifier.
//! * **Standing is supplied by the caller on every call** (contact / verified
//!   / unknown come from trust state this crate does not own). The guard only
//!   forwards a change when it differs from what the gate already holds,
//!   because `Gate::set_standing` resets a peer's buckets.
//! * **No duplicate suppression here.** `handle_incoming` is idempotent
//!   (`insert_if_new`) and a retransmit must still be ACKed or the sender
//!   retries forever. Rejecting a "duplicate" at the gate would break that,
//!   so the gate is never given an id.
//! * **A rejected frame is dropped without an ACK.** The sender's outbox
//!   retries with backoff, so a rate-limited contact recovers by itself.
//!
//! Defaults are `siar_abuse::GateConfig::default()` — reasoned, not tuned.

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use siar_abuse::{Gate, GateConfig, GateStats, Incoming, PeerKey, QuotaKind, Reject, Standing};
use siar_protocol::v1::EnvelopeKind;
use siar_protocol::WireMessage;

/// Why a frame was refused before any expensive work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IngressReject {
    #[error("frame is larger than the limit for its kind")]
    TooLong,
    #[error("unsupported protocol version")]
    BadVersion,
    #[error("sender is blocked")]
    Blocked,
    #[error("rate limited; retry in {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("too many strangers at once; retry in {retry_after_ms} ms")]
    GlobalLimited { retry_after_ms: u64 },
}

impl IngressReject {
    /// `true` for the rejections that say nothing about the sender's intent
    /// (a legitimate contact that is merely fast). Callers log these quietly.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::GlobalLimited { .. })
    }
}

/// Which §62 quota an envelope draws from.
///
/// Session-opening and call-control frames count as handshakes (small, rare,
/// and the most expensive per byte to process); everything that carries or
/// acknowledges chat content counts as a message.
#[must_use]
pub const fn quota_for(kind: &EnvelopeKind) -> QuotaKind {
    match kind {
        EnvelopeKind::PairwiseWelcome | EnvelopeKind::CallSignal(_) => QuotaKind::Handshake,
        EnvelopeKind::Text
        | EnvelopeKind::Attachment
        | EnvelopeKind::GroupEvent
        | EnvelopeKind::GroupMlsCommit
        | EnvelopeKind::GroupMlsWelcome
        | EnvelopeKind::GroupMlsApplication
        | EnvelopeKind::DeliveryAck { .. }
        | EnvelopeKind::ReadReceipt { .. }
        | EnvelopeKind::PairwiseMls => QuotaKind::MessageRequest,
    }
}

/// Which quota a *wire* message draws from, how many bytes of it can be large,
/// and its protocol version — what a relay (which sees every `WireMessage`
/// kind from strangers) needs before doing any work on a frame.
///
/// * Bundles and mailbox deposits are `DtnBundle` (a stranger's relay traffic).
/// * A signed mailbox check-in is `SignatureVerify`: verifying the signature is
///   the expensive part, so that is what is rationed.
/// * Anonymous check-ins and route advertisements are cheap, small and
///   periodic; they share the bundle budget rather than the (much tighter,
///   chat-shaped) handshake one.
/// * A chat envelope keeps the classification [`quota_for`] gives it.
#[must_use]
pub fn quota_for_wire(msg: &WireMessage) -> (QuotaKind, usize, u16) {
    match msg {
        WireMessage::V1(e) => (quota_for(&e.kind), e.payload.len(), e.version.0),
        WireMessage::Mesh(m) => (QuotaKind::DtnBundle, m.ciphertext.len(), 1),
        WireMessage::TokenMailboxDeposit(t) => (QuotaKind::DtnBundle, t.ciphertext.len(), 1),
        WireMessage::MailboxCheckIn(_) => (QuotaKind::SignatureVerify, 256, 1),
        // Rare, small, and the most security-sensitive thing a stranger can
        // send: it shares the (tight) handshake budget.
        WireMessage::Link(_) => (QuotaKind::Handshake, 1024, 1),
        WireMessage::AnonymousMailboxCheckIn(_) | WireMessage::RouteAdvertisement(_) => {
            (QuotaKind::DtnBundle, 128, 1)
        }
    }
}

/// Thread-safe admission guard. Cheap to share behind an `Arc`.
pub struct IngressGuard {
    gate: Mutex<Gate>,
    salt: [u8; 32],
    started: Instant,
}

impl IngressGuard {
    /// A guard with a fresh random salt.
    #[must_use]
    pub fn new(cfg: GateConfig) -> Self {
        let mut salt = [0u8; 32];
        // `uuid` is already a dependency and draws from the OS CSPRNG.
        salt[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        salt[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self::with_salt(cfg, salt)
    }

    /// Fixed salt, for tests.
    #[must_use]
    pub fn with_salt(cfg: GateConfig, salt: [u8; 32]) -> Self {
        let started = Instant::now();
        Self {
            gate: Mutex::new(Gate::new(cfg, 0)),
            salt,
            started,
        }
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn peer_key(&self, transport_id: &[u8; 32]) -> PeerKey {
        let h = blake3::keyed_hash(&self.salt, transport_id);
        let mut k = [0u8; 16];
        k.copy_from_slice(&h.as_bytes()[..16]);
        PeerKey(k)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Decide whether a frame may be processed.
    ///
    /// `transport_id` is the authenticated endpoint id the frame arrived
    /// from. `payload_len` is the size of the envelope payload (the only part
    /// of the frame that can be large). `standing` is the caller's current
    /// view of that peer.
    pub fn admit(
        &self,
        transport_id: &[u8; 32],
        standing: Standing,
        kind: &EnvelopeKind,
        payload_len: usize,
        version: u16,
    ) -> Result<(), IngressReject> {
        self.admit_quota(
            transport_id,
            standing,
            quota_for(kind),
            payload_len,
            version,
        )
    }

    /// [`admit`](Self::admit) for any wire message (relay path).
    pub fn admit_wire(
        &self,
        transport_id: &[u8; 32],
        standing: Standing,
        msg: &WireMessage,
    ) -> Result<(), IngressReject> {
        let (quota, len, version) = quota_for_wire(msg);
        self.admit_quota(transport_id, standing, quota, len, version)
    }

    fn admit_quota(
        &self,
        transport_id: &[u8; 32],
        standing: Standing,
        quota: QuotaKind,
        payload_len: usize,
        version: u16,
    ) -> Result<(), IngressReject> {
        let peer = self.peer_key(transport_id);
        let now = self.now_ms();
        let mut gate = self.lock();
        // Never un-block a peer just because the caller's trust view says
        // "unknown" — blocking is an explicit action (`block`/`unblock`).
        if standing != Standing::Unknown
            && standing != Standing::Blocked
            && gate.standing(peer) != standing
            && !gate.is_blocked(peer)
        {
            gate.set_standing(peer, standing);
        } else if standing == Standing::Unknown
            && gate.standing(peer) != Standing::Unknown
            && !gate.is_blocked(peer)
        {
            // Trust was withdrawn (contact removed): drop back to stranger limits.
            gate.set_standing(peer, Standing::Unknown);
        }
        let inc = Incoming {
            peer,
            kind: quota,
            len: payload_len,
            version: u8::try_from(version).unwrap_or(u8::MAX),
            id: None,
        };
        match gate.admit(&inc, now) {
            Ok(_) => Ok(()),
            Err(Reject::TooLong) => Err(IngressReject::TooLong),
            Err(Reject::BadVersion) => Err(IngressReject::BadVersion),
            Err(Reject::Blocked) => Err(IngressReject::Blocked),
            Err(Reject::RateLimited { retry_after_ms }) => {
                Err(IngressReject::RateLimited { retry_after_ms })
            }
            Err(Reject::GlobalLimited { retry_after_ms }) => {
                Err(IngressReject::GlobalLimited { retry_after_ms })
            }
            // Pairing cooldown / duplicates are not produced for these kinds
            // (`id` is `None`, no `PairingAttempt` quota is used here); if the
            // gate ever grows a new reason, refuse conservatively.
            Err(_) => Err(IngressReject::RateLimited {
                retry_after_ms: 1_000,
            }),
        }
    }

    /// Block a transport identity (§65: before any crypto).
    pub fn block(&self, transport_id: &[u8; 32]) -> bool {
        let peer = self.peer_key(transport_id);
        self.lock().block(peer)
    }

    /// Lift a block. Never restores trust by itself (§65).
    pub fn unblock(&self, transport_id: &[u8; 32]) {
        let peer = self.peer_key(transport_id);
        self.lock().unblock(peer);
    }

    #[must_use]
    pub fn is_blocked(&self, transport_id: &[u8; 32]) -> bool {
        let peer = self.peer_key(transport_id);
        self.lock().is_blocked(peer)
    }

    #[must_use]
    pub fn stats(&self) -> GateStats {
        self.lock().stats()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_domain::MessageId;

    const A: [u8; 32] = [1; 32];
    const B: [u8; 32] = [2; 32];

    fn guard() -> IngressGuard {
        IngressGuard::with_salt(GateConfig::default(), [7; 32])
    }

    fn text(g: &IngressGuard, who: &[u8; 32], standing: Standing) -> Result<(), IngressReject> {
        g.admit(who, standing, &EnvelopeKind::Text, 100, 1)
    }

    #[test]
    fn a_stranger_is_limited_but_a_contact_is_not() {
        let g = guard();
        // Default stranger allowance: 5 message requests, then refused.
        for _ in 0..5 {
            assert!(text(&g, &A, Standing::Unknown).is_ok());
        }
        assert!(matches!(
            text(&g, &A, Standing::Unknown),
            Err(IngressReject::RateLimited { .. })
        ));
        // A contact has no message cap.
        for _ in 0..1_000 {
            assert!(text(&g, &B, Standing::Contact).is_ok());
        }
    }

    #[test]
    fn promoting_a_peer_to_contact_lifts_its_limit() {
        let g = guard();
        for _ in 0..5 {
            text(&g, &A, Standing::Unknown).unwrap();
        }
        assert!(text(&g, &A, Standing::Unknown).is_err());
        assert!(text(&g, &A, Standing::Contact).is_ok());
        // Staying a contact must not reset anything or fail.
        for _ in 0..50 {
            assert!(text(&g, &A, Standing::Contact).is_ok());
        }
    }

    #[test]
    fn withdrawing_trust_returns_to_stranger_limits() {
        let g = guard();
        for _ in 0..20 {
            text(&g, &A, Standing::Contact).unwrap();
        }
        // Back to unknown: a fresh stranger bucket, so 5 more then refused.
        let mut ok = 0;
        while text(&g, &A, Standing::Unknown).is_ok() {
            ok += 1;
            assert!(ok <= 5, "stranger limit must apply again");
        }
        assert_eq!(ok, 5);
    }

    #[test]
    fn a_blocked_identity_is_refused_whatever_the_caller_says() {
        let g = guard();
        assert!(g.block(&A));
        assert!(g.is_blocked(&A));
        assert_eq!(
            text(&g, &A, Standing::Verified),
            Err(IngressReject::Blocked)
        );
        // Another identity is unaffected.
        assert!(text(&g, &B, Standing::Contact).is_ok());
        g.unblock(&A);
        assert!(text(&g, &A, Standing::Contact).is_ok());
    }

    #[test]
    fn oversize_and_bad_version_are_refused_before_any_state() {
        let g = guard();
        assert_eq!(
            g.admit(
                &A,
                Standing::Contact,
                &EnvelopeKind::Text,
                10 * 1024 * 1024,
                1
            ),
            Err(IngressReject::TooLong)
        );
        assert_eq!(
            g.admit(&A, Standing::Contact, &EnvelopeKind::Text, 10, 2),
            Err(IngressReject::BadVersion)
        );
        // A version that does not fit in a u8 must not wrap into "1".
        assert_eq!(
            g.admit(&A, Standing::Contact, &EnvelopeKind::Text, 10, 256 + 1),
            Err(IngressReject::BadVersion)
        );
    }

    #[test]
    fn all_strangers_together_share_a_bound() {
        let g = guard();
        let mut admitted = 0u32;
        for i in 0..2_000u32 {
            let mut id = [0u8; 32];
            id[..4].copy_from_slice(&i.to_le_bytes());
            if text(&g, &id, Standing::Unknown).is_ok() {
                admitted += 1;
            }
        }
        // Global burst for message requests is 100 (default config): 2 000
        // fresh identities get no more than that, plus a sliver of refill.
        assert!((100..=110).contains(&admitted), "admitted {admitted}");
    }

    #[test]
    fn spoofing_an_envelope_sender_cannot_spend_someone_elses_budget() {
        // The guard never sees `Envelope::sender`; two different transport
        // ids stay independent no matter what either claims inside a frame.
        let g = guard();
        for _ in 0..5 {
            text(&g, &A, Standing::Unknown).unwrap();
        }
        assert!(text(&g, &A, Standing::Unknown).is_err());
        assert!(text(&g, &B, Standing::Unknown).is_ok());
    }

    #[test]
    fn kinds_map_to_the_intended_quota() {
        use siar_domain::CallControlEvent;
        assert_eq!(quota_for(&EnvelopeKind::Text), QuotaKind::MessageRequest);
        assert_eq!(
            quota_for(&EnvelopeKind::DeliveryAck {
                acked_message: MessageId::new()
            }),
            QuotaKind::MessageRequest
        );
        assert_eq!(
            quota_for(&EnvelopeKind::PairwiseWelcome),
            QuotaKind::Handshake
        );
        let _ = CallControlEvent::Hangup;
    }

    #[test]
    fn the_peer_key_depends_on_the_salt() {
        let g1 = IngressGuard::with_salt(GateConfig::default(), [1; 32]);
        let g2 = IngressGuard::with_salt(GateConfig::default(), [2; 32]);
        assert_ne!(g1.peer_key(&A), g2.peer_key(&A));
        assert_ne!(g1.peer_key(&A), g1.peer_key(&B));
    }

    #[test]
    fn wire_messages_map_to_the_relay_quotas() {
        use siar_domain::{DeviceId, MessagePriority};
        use siar_protocol::MeshEnvelope;
        let mesh = WireMessage::Mesh(MeshEnvelope {
            id: MessageId::new(),
            destination: DeviceId::new(),
            created_at: 0,
            expires_at: 10,
            hop_limit: 3,
            priority: MessagePriority::Normal,
            payload_hash: [0; 32],
            ciphertext: vec![0; 1234],
        });
        let (q, len, v) = quota_for_wire(&mesh);
        assert_eq!((q, len, v), (QuotaKind::DtnBundle, 1234, 1));
    }

    #[test]
    fn a_stranger_flooding_bundles_is_bounded_but_a_known_device_is_not() {
        use siar_domain::{DeviceId, MessagePriority};
        use siar_protocol::MeshEnvelope;
        let g = guard();
        let bundle = || {
            WireMessage::Mesh(MeshEnvelope {
                id: MessageId::new(),
                destination: DeviceId::new(),
                created_at: 0,
                expires_at: 10,
                hop_limit: 3,
                priority: MessagePriority::Normal,
                payload_hash: [0; 32],
                ciphertext: vec![0; 100],
            })
        };
        let stranger = (0..500)
            .filter(|_| g.admit_wire(&A, Standing::Unknown, &bundle()).is_ok())
            .count();
        // Per-peer stranger burst for bundles is 20 (default config).
        assert!((20..=22).contains(&stranger), "admitted {stranger}");
        let known = (0..150)
            .filter(|_| g.admit_wire(&B, Standing::Contact, &bundle()).is_ok())
            .count();
        assert_eq!(known, 150);
    }

    #[test]
    fn device_link_messages_share_the_tight_handshake_budget() {
        use siar_protocol::link::LinkMessage;
        let (q, _, _) = quota_for_wire(&WireMessage::Link(LinkMessage::Denied));
        assert_eq!(q, QuotaKind::Handshake);
        // A stranger spraying link requests is cut off after a few.
        let g = guard();
        let admitted = (0..50)
            .filter(|_| {
                g.admit_wire(
                    &A,
                    Standing::Unknown,
                    &WireMessage::Link(LinkMessage::Denied),
                )
                .is_ok()
            })
            .count();
        assert!(admitted <= 4, "admitted {admitted}");
    }
}
