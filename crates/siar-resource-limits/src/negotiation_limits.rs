//! §71 capability negotiation limits.
//!
//! Bounds the three things the spec names — capability count, parameter
//! bytes, concurrent negotiations — so "a peer cannot hold unlimited
//! negotiation sessions" and cannot send an oversized offer.

use crate::connections::{LimitDenied, PeerHold, PeerKey, PeerLimiter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NegotiationLimits {
    pub max_capabilities: usize,
    pub max_param_bytes_each: usize,
    pub max_param_bytes_total: usize,
    pub max_concurrent: u32,
    pub max_concurrent_per_peer: u32,
}

impl Default for NegotiationLimits {
    fn default() -> Self {
        Self {
            max_capabilities: 256,
            max_param_bytes_each: 256,
            max_param_bytes_total: 16 * 1024,
            max_concurrent: 64,
            max_concurrent_per_peer: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NegotiationReject {
    #[error("offer lists {count} capabilities, limit {max}")]
    TooManyCapabilities { count: usize, max: usize },
    #[error("a parameter of {bytes} bytes exceeds {max}")]
    ParameterTooLarge { bytes: usize, max: usize },
    #[error("offer parameters total {bytes} bytes, limit {max}")]
    ParametersTooLarge { bytes: usize, max: usize },
    #[error("too many concurrent negotiations")]
    Busy,
    #[error("this peer already has the maximum negotiations open")]
    PeerBusy,
}

#[derive(Debug, Clone)]
pub struct NegotiationGate {
    limits: NegotiationLimits,
    sessions: PeerLimiter,
}

#[derive(Debug)]
#[must_use = "dropping the permit closes the negotiation slot"]
pub struct NegotiationPermit(#[allow(dead_code)] PeerHold);

impl NegotiationGate {
    pub fn new(limits: NegotiationLimits) -> Self {
        Self {
            limits,
            sessions: PeerLimiter::new(limits.max_concurrent, limits.max_concurrent_per_peer),
        }
    }

    /// Check the offer's *shape* before allocating a session for it, and
    /// before any per-capability work.
    pub fn check_offer(&self, param_sizes: &[usize]) -> Result<(), NegotiationReject> {
        let l = &self.limits;
        if param_sizes.len() > l.max_capabilities {
            return Err(NegotiationReject::TooManyCapabilities {
                count: param_sizes.len(),
                max: l.max_capabilities,
            });
        }
        let mut total = 0usize;
        for &b in param_sizes {
            if b > l.max_param_bytes_each {
                return Err(NegotiationReject::ParameterTooLarge {
                    bytes: b,
                    max: l.max_param_bytes_each,
                });
            }
            total = total.saturating_add(b);
        }
        if total > l.max_param_bytes_total {
            return Err(NegotiationReject::ParametersTooLarge {
                bytes: total,
                max: l.max_param_bytes_total,
            });
        }
        Ok(())
    }

    /// Validate, then take a negotiation slot.
    pub fn begin(
        &self,
        peer: PeerKey,
        param_sizes: &[usize],
    ) -> Result<NegotiationPermit, NegotiationReject> {
        self.check_offer(param_sizes)?;
        self.sessions
            .try_acquire(peer, self.limits.max_concurrent)
            .map(NegotiationPermit)
            .map_err(|d| match d {
                LimitDenied::Global => NegotiationReject::Busy,
                LimitDenied::PerPeer => NegotiationReject::PeerBusy,
            })
    }

    pub fn open(&self) -> u32 {
        self.sessions.active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PeerKey = [7; 32];

    #[test]
    fn offer_shape_limits() {
        let g = NegotiationGate::new(NegotiationLimits::default());
        assert!(g.check_offer(&[10; 10]).is_ok());
        assert!(matches!(
            g.check_offer(&[0; 257]),
            Err(NegotiationReject::TooManyCapabilities { count: 257, .. })
        ));
        assert!(matches!(
            g.check_offer(&[257]),
            Err(NegotiationReject::ParameterTooLarge { bytes: 257, .. })
        ));
        // 100 params of 200 bytes each: individually fine, 20000 total is not
        assert!(matches!(
            g.check_offer(&[200; 100]),
            Err(NegotiationReject::ParametersTooLarge { .. })
        ));
    }

    #[test]
    fn a_peer_cannot_hold_unlimited_sessions() {
        let g = NegotiationGate::new(NegotiationLimits::default());
        let a = g.begin(P, &[1]).unwrap();
        let _b = g.begin(P, &[1]).unwrap();
        assert_eq!(g.begin(P, &[1]).unwrap_err(), NegotiationReject::PeerBusy);
        drop(a);
        assert!(g.begin(P, &[1]).is_ok());
    }

    #[test]
    fn global_cap_and_malformed_offers_do_not_consume_slots() {
        let g = NegotiationGate::new(NegotiationLimits {
            max_concurrent: 1,
            ..Default::default()
        });
        let _a = g.begin([1; 32], &[1]).unwrap();
        assert_eq!(g.begin([2; 32], &[1]).unwrap_err(), NegotiationReject::Busy);
        assert!(g.begin([3; 32], &[9_999]).is_err());
        assert_eq!(g.open(), 1);
    }
}
