//! §62 messaging backpressure, §63 inbound limits.

use crate::peer_quota::TrustClass;
use crate::token_bucket::TokenBucket;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundLimits {
    pub max_message_bytes: u64,
    pub burst: u64,
    pub msgs_per_sec: u64,
    pub max_pending: u32,
}

impl InboundLimits {
    /// §63 "Unknown peers stricter". Numbers are this crate's own
    /// reasoned defaults; the ordering (never looser for less trust) is
    /// the tested invariant.
    pub fn for_trust(t: TrustClass) -> Self {
        let (kib, burst, rate, pending) = match t {
            TrustClass::Unknown => (16, 5, 1, 8),
            TrustClass::Known => (64, 20, 5, 32),
            TrustClass::Verified => (128, 40, 10, 64),
            TrustClass::Organization | TrustClass::Authority => (256, 60, 15, 128),
            TrustClass::LocalOwnDevice => (1024, 200, 50, 512),
        };
        Self {
            max_message_bytes: kib * 1024,
            burst,
            msgs_per_sec: rate,
            max_pending: pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InboundReject {
    #[error("message of {size} bytes exceeds {max}")]
    TooLarge { size: u64, max: u64 },
    #[error("message rate exceeded")]
    RateLimited,
    #[error("too many unprocessed messages from this peer")]
    TooManyPending,
}

/// Per-peer inbound gate. Cheapest checks first (size, then pending, then
/// the rate bucket), so a rejected message costs no token and no work.
#[derive(Debug)]
pub struct InboundGate {
    limits: InboundLimits,
    bucket: TokenBucket,
    pending: u32,
}

impl InboundGate {
    pub fn new(limits: InboundLimits, now_millis: u64) -> Self {
        Self {
            limits,
            bucket: TokenBucket::new(limits.burst, limits.msgs_per_sec, now_millis),
            pending: 0,
        }
    }

    /// On `Ok`, the caller owes one [`Self::completed`] when processing ends.
    pub fn admit(&mut self, size_bytes: u64, now_millis: u64) -> Result<(), InboundReject> {
        if size_bytes > self.limits.max_message_bytes {
            return Err(InboundReject::TooLarge {
                size: size_bytes,
                max: self.limits.max_message_bytes,
            });
        }
        if self.pending >= self.limits.max_pending {
            return Err(InboundReject::TooManyPending);
        }
        if !self.bucket.try_consume(1, now_millis) {
            return Err(InboundReject::RateLimited);
        }
        self.pending += 1;
        Ok(())
    }

    pub fn completed(&mut self) {
        self.pending = self.pending.saturating_sub(1);
    }

    pub fn pending(&self) -> u32 {
        self.pending
    }

    /// Trust changed (e.g. peer verified): adopt new limits, keep the
    /// pending count so in-flight work is still accounted.
    pub fn retune(&mut self, limits: InboundLimits, now_millis: u64) {
        self.limits = limits;
        self.bucket = TokenBucket::new(limits.burst, limits.msgs_per_sec, now_millis);
    }
}

/// §62: what to do with an outbound message when the send queue is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundAction {
    EnqueueNow,
    /// Persisted; the UI shows "Waiting to send". Never lost.
    PersistDeferred,
    /// Ephemeral (typing, presence): not worth keeping.
    Discard,
}

pub fn outbound_action(queue_has_room: bool, durable: bool) -> OutboundAction {
    match (queue_has_room, durable) {
        (true, _) => OutboundAction::EnqueueNow,
        (false, true) => OutboundAction::PersistDeferred,
        (false, false) => OutboundAction::Discard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn less_trust_is_never_looser() {
        let order = [
            TrustClass::Unknown,
            TrustClass::Known,
            TrustClass::Verified,
            TrustClass::Organization,
            TrustClass::LocalOwnDevice,
        ];
        for w in order.windows(2) {
            let (a, b) = (
                InboundLimits::for_trust(w[0]),
                InboundLimits::for_trust(w[1]),
            );
            assert!(a.max_message_bytes <= b.max_message_bytes);
            assert!(a.burst <= b.burst && a.msgs_per_sec <= b.msgs_per_sec);
            assert!(a.max_pending <= b.max_pending);
        }
    }

    #[test]
    fn oversize_is_rejected_without_spending_a_token() {
        let mut g = InboundGate::new(InboundLimits::for_trust(TrustClass::Unknown), 0);
        for _ in 0..50 {
            assert!(matches!(
                g.admit(10_000_000, 0),
                Err(InboundReject::TooLarge { .. })
            ));
        }
        assert!(
            g.admit(10, 0).is_ok(),
            "bucket untouched by rejected oversize messages"
        );
    }

    #[test]
    fn pending_cap_and_completion() {
        let limits = InboundLimits {
            max_message_bytes: 100,
            burst: 100,
            msgs_per_sec: 100,
            max_pending: 2,
        };
        let mut g = InboundGate::new(limits, 0);
        g.admit(1, 0).unwrap();
        g.admit(1, 0).unwrap();
        assert_eq!(g.admit(1, 0), Err(InboundReject::TooManyPending));
        g.completed();
        assert!(g.admit(1, 0).is_ok());
        g.completed();
        g.completed();
        g.completed();
        assert_eq!(g.pending(), 0, "extra completions saturate");
    }

    #[test]
    fn rate_limit_and_refill() {
        let limits = InboundLimits {
            max_message_bytes: 100,
            burst: 2,
            msgs_per_sec: 1,
            max_pending: 100,
        };
        let mut g = InboundGate::new(limits, 0);
        g.admit(1, 0).unwrap();
        g.admit(1, 0).unwrap();
        assert_eq!(g.admit(1, 0), Err(InboundReject::RateLimited));
        assert!(g.admit(1, 1_000).is_ok());
    }

    #[test]
    fn durable_outbound_is_never_discarded() {
        assert_eq!(outbound_action(true, true), OutboundAction::EnqueueNow);
        assert_eq!(
            outbound_action(false, true),
            OutboundAction::PersistDeferred
        );
        assert_eq!(outbound_action(false, false), OutboundAction::Discard);
    }
}
