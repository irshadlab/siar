//! §127 hash/CPU DoS, §128 signature-verification budget, §131 handshake
//! budget.
//!
//! Verification work is attacker-triggered: every object a peer sends
//! costs us hashing and signature checks. So it goes through a bounded
//! queue, each submission is priced in cost units and charged to the
//! sender, and *unknown* senders share one small class-wide bucket and a
//! capped number of queue slots — rotating fake identities buys an
//! attacker nothing, because their budget is the class's, not their own.
//! Known senders get a per-peer bucket (that map is bounded by the number
//! of tracked peers, not by attacker behaviour).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::connections::PeerKey;
use crate::peer_quota::TrustClass;
use crate::permits::BoundedCounter;
use crate::token_bucket::TokenBucket;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyLimits {
    pub queue_slots: usize,
    /// How many of those slots unknown senders may hold at once.
    pub unknown_slots: usize,
    pub unknown_class: (u64, u64), // (burst, units/sec) — shared by ALL unknown peers
    pub max_tracked_peers: usize,
}

impl Default for VerifyLimits {
    fn default() -> Self {
        Self {
            queue_slots: 256,
            unknown_slots: 16,
            unknown_class: (50, 20),
            max_tracked_peers: 1_024,
        }
    }
}

/// Per-peer (burst, units/sec) for non-unknown trust classes.
fn peer_rate(t: TrustClass) -> (u64, u64) {
    match t {
        TrustClass::Unknown => (50, 20), // unused: unknowns use the class bucket
        TrustClass::Known => (200, 100),
        TrustClass::Verified => (400, 200),
        TrustClass::Organization | TrustClass::Authority => (800, 400),
        TrustClass::LocalOwnDevice => (4_000, 2_000),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyReject {
    #[error("verification queue full")]
    QueueFull,
    #[error("unknown senders already hold their share of the queue")]
    UnknownSlotsFull,
    #[error("unknown senders have exhausted their shared CPU budget")]
    UnknownRate,
    #[error("this peer has exhausted its CPU budget")]
    PeerRate,
}

#[derive(Debug)]
struct State {
    unknown_in_flight: usize,
    unknown_bucket: TokenBucket,
    peers: HashMap<PeerKey, TokenBucket>,
}

#[derive(Debug)]
struct Inner {
    limits: VerifyLimits,
    queue: BoundedCounter,
    state: Mutex<State>,
}

#[derive(Debug, Clone)]
pub struct VerificationBudget(Arc<Inner>);

#[derive(Debug)]
#[must_use = "dropping the ticket (verification finished) frees the queue slot"]
pub struct VerifyTicket {
    inner: Arc<Inner>,
    unknown: bool,
}

impl VerificationBudget {
    pub fn new(limits: VerifyLimits, now_millis: u64) -> Self {
        Self(Arc::new(Inner {
            limits,
            queue: BoundedCounter::new(limits.queue_slots as u64),
            state: Mutex::new(State {
                unknown_in_flight: 0,
                unknown_bucket: TokenBucket::new(
                    limits.unknown_class.0,
                    limits.unknown_class.1,
                    now_millis,
                ),
                peers: HashMap::new(),
            }),
        }))
    }

    /// Queue a verification costing `cost_units`. Cheapest rejections
    /// first; tokens are only spent once a slot is certain.
    pub fn submit(
        &self,
        peer: PeerKey,
        trust: TrustClass,
        cost_units: u64,
        now_millis: u64,
    ) -> Result<VerifyTicket, VerifyReject> {
        let unknown = trust == TrustClass::Unknown;
        let mut st = lock(&self.0.state);
        if unknown && st.unknown_in_flight >= self.0.limits.unknown_slots {
            return Err(VerifyReject::UnknownSlotsFull);
        }
        if self.0.queue.available() == 0 {
            return Err(VerifyReject::QueueFull);
        }
        if unknown {
            if !st.unknown_bucket.try_consume(cost_units, now_millis) {
                return Err(VerifyReject::UnknownRate);
            }
        } else {
            if !st.peers.contains_key(&peer) && st.peers.len() >= self.0.limits.max_tracked_peers {
                // Forget the idlest peer (fullest bucket) to stay bounded.
                let idlest = st
                    .peers
                    .iter_mut()
                    .map(|(k, b)| (*k, b.available(now_millis)))
                    .max_by_key(|(_, a)| *a)
                    .map(|(k, _)| k);
                if let Some(k) = idlest {
                    st.peers.remove(&k);
                }
            }
            let (burst, rate) = peer_rate(trust);
            let bucket = st
                .peers
                .entry(peer)
                .or_insert_with(|| TokenBucket::new(burst, rate, now_millis));
            if !bucket.try_consume(cost_units, now_millis) {
                return Err(VerifyReject::PeerRate);
            }
        }
        if !self.0.queue.try_reserve(1) {
            return Err(VerifyReject::QueueFull); // raced with another submitter
        }
        if unknown {
            st.unknown_in_flight += 1;
        }
        drop(st);
        Ok(VerifyTicket {
            inner: Arc::clone(&self.0),
            unknown,
        })
    }

    pub fn in_flight(&self) -> u64 {
        self.0.queue.in_use()
    }

    pub fn tracked_peers(&self) -> usize {
        lock(&self.0.state).peers.len()
    }
}

impl Drop for VerifyTicket {
    fn drop(&mut self) {
        if self.unknown {
            let mut st = lock(&self.inner.state);
            st.unknown_in_flight = st.unknown_in_flight.saturating_sub(1);
        }
        self.inner.queue.release(1);
    }
}

// ---------------------------------------------------------- §131 handshake

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeLimits {
    pub half_open: u64,
    pub authenticated: u64,
    pub active: u64,
}

impl Default for HandshakeLimits {
    fn default() -> Self {
        Self {
            half_open: 16,
            authenticated: 64,
            active: 128,
        }
    }
}

impl HandshakeLimits {
    /// §131: half-open sessions get the smallest budget.
    pub fn is_well_formed(&self) -> bool {
        self.half_open > 0 && self.half_open <= self.authenticated && self.half_open <= self.active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeDenied {
    #[error("half-open session pool full")]
    HalfOpenFull,
    #[error("authenticated session pool full")]
    AuthenticatedFull,
    #[error("active session pool full")]
    ActiveFull,
}

#[derive(Debug)]
struct Pools {
    half_open: BoundedCounter,
    authenticated: BoundedCounter,
    active: BoundedCounter,
}

#[derive(Debug, Clone)]
pub struct HandshakePools(Arc<Pools>);

#[derive(Debug)]
#[must_use]
pub struct HalfOpen(Arc<Pools>);
#[derive(Debug)]
#[must_use]
pub struct Authenticated(Arc<Pools>);
#[derive(Debug)]
#[must_use]
pub struct Active(Arc<Pools>);

impl HandshakePools {
    pub fn new(l: HandshakeLimits) -> Self {
        Self(Arc::new(Pools {
            half_open: BoundedCounter::new(l.half_open),
            authenticated: BoundedCounter::new(l.authenticated),
            active: BoundedCounter::new(l.active),
        }))
    }

    pub fn begin(&self) -> Result<HalfOpen, HandshakeDenied> {
        if self.0.half_open.try_reserve(1) {
            Ok(HalfOpen(Arc::clone(&self.0)))
        } else {
            Err(HandshakeDenied::HalfOpenFull)
        }
    }

    pub fn counts(&self) -> (u64, u64, u64) {
        (
            self.0.half_open.in_use(),
            self.0.authenticated.in_use(),
            self.0.active.in_use(),
        )
    }
}

impl HalfOpen {
    /// Handshake completed. Takes the authenticated slot *before*
    /// releasing the half-open one; on failure the caller gets the
    /// half-open permit back to close the session cleanly.
    pub fn authenticate(self) -> Result<Authenticated, (HalfOpen, HandshakeDenied)> {
        if self.0.authenticated.try_reserve(1) {
            let pools = Arc::clone(&self.0);
            drop(self); // frees the half-open slot
            Ok(Authenticated(pools))
        } else {
            Err((self, HandshakeDenied::AuthenticatedFull))
        }
    }
}

impl Authenticated {
    pub fn activate(self) -> Result<Active, (Authenticated, HandshakeDenied)> {
        if self.0.active.try_reserve(1) {
            let pools = Arc::clone(&self.0);
            drop(self);
            Ok(Active(pools))
        } else {
            Err((self, HandshakeDenied::ActiveFull))
        }
    }
}

impl Drop for HalfOpen {
    fn drop(&mut self) {
        self.0.half_open.release(1);
    }
}
impl Drop for Authenticated {
    fn drop(&mut self) {
        self.0.authenticated.release(1);
    }
}
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.release(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vb() -> VerificationBudget {
        VerificationBudget::new(
            VerifyLimits {
                queue_slots: 8,
                unknown_slots: 2,
                unknown_class: (10, 0),
                max_tracked_peers: 2,
            },
            0,
        )
    }

    #[test]
    fn rotating_unknown_identities_share_one_class_budget() {
        let v = vb();
        let mut accepted = 0;
        for i in 0..200u32 {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_le_bytes());
            if let Ok(t) = v.submit(k, TrustClass::Unknown, 1, 0) {
                accepted += 1;
                drop(t); // finish immediately: only the *rate* limits now
            }
        }
        assert_eq!(
            accepted, 10,
            "burst of 10 units total, regardless of identities"
        );
        assert_eq!(v.tracked_peers(), 0, "unknowns create no per-peer state");
    }

    #[test]
    fn unknown_senders_cannot_hold_more_than_their_slots() {
        let v = VerificationBudget::new(
            VerifyLimits {
                queue_slots: 8,
                unknown_slots: 2,
                unknown_class: (1_000, 0),
                max_tracked_peers: 4,
            },
            0,
        );
        let _a = v.submit([1; 32], TrustClass::Unknown, 1, 0).unwrap();
        let _b = v.submit([2; 32], TrustClass::Unknown, 1, 0).unwrap();
        assert_eq!(
            v.submit([3; 32], TrustClass::Unknown, 1, 0).unwrap_err(),
            VerifyReject::UnknownSlotsFull
        );
        // known peers still get in despite unknown saturation
        assert!(v.submit([9; 32], TrustClass::Known, 1, 0).is_ok());
    }

    #[test]
    fn known_peers_are_charged_individually() {
        let v = vb();
        let (a, b) = ([1; 32], [2; 32]);
        let _t = v.submit(a, TrustClass::Known, 200, 0).unwrap(); // whole burst
        assert_eq!(
            v.submit(a, TrustClass::Known, 1, 0).unwrap_err(),
            VerifyReject::PeerRate
        );
        assert!(
            v.submit(b, TrustClass::Known, 1, 0).is_ok(),
            "b unaffected by a"
        );
    }

    #[test]
    fn rejected_submissions_do_not_burn_tokens_or_slots() {
        let v = VerificationBudget::new(
            VerifyLimits {
                queue_slots: 1,
                unknown_slots: 1,
                unknown_class: (5, 0),
                max_tracked_peers: 4,
            },
            0,
        );
        let _full = v.submit([1; 32], TrustClass::Known, 1, 0).unwrap();
        for _ in 0..50 {
            assert_eq!(
                v.submit([2; 32], TrustClass::Unknown, 1, 0).unwrap_err(),
                VerifyReject::QueueFull
            );
        }
        drop(_full);
        // the unknown bucket still has its full 5 units
        let t = v.submit([2; 32], TrustClass::Unknown, 5, 0);
        assert!(t.is_ok());
    }

    #[test]
    fn queue_slot_frees_on_drop_and_peer_map_is_bounded() {
        let v = vb();
        let t = v.submit([1; 32], TrustClass::Known, 1, 0).unwrap();
        assert_eq!(v.in_flight(), 1);
        drop(t);
        assert_eq!(v.in_flight(), 0);
        for i in 0..10u8 {
            let _ = v.submit([i; 32], TrustClass::Verified, 1, 0).map(drop);
        }
        assert!(v.tracked_peers() <= 2);
    }

    #[test]
    fn handshake_default_gives_half_open_the_smallest_budget() {
        assert!(HandshakeLimits::default().is_well_formed());
        assert!(!HandshakeLimits {
            half_open: 100,
            authenticated: 10,
            active: 10
        }
        .is_well_formed());
        assert!(!HandshakeLimits {
            half_open: 0,
            authenticated: 1,
            active: 1
        }
        .is_well_formed());
    }

    #[test]
    fn a_half_open_flood_cannot_touch_authenticated_capacity() {
        let p = HandshakePools::new(HandshakeLimits {
            half_open: 2,
            authenticated: 2,
            active: 2,
        });
        let h1 = p.begin().unwrap();
        let _h2 = p.begin().unwrap();
        assert_eq!(p.begin().unwrap_err(), HandshakeDenied::HalfOpenFull);
        let a1 = h1.authenticate().unwrap();
        assert_eq!(
            p.counts(),
            (1, 1, 0),
            "promotion moved the slot between pools"
        );
        assert!(p.begin().is_ok(), "half-open slot was freed by promotion");
        drop(a1);
        assert_eq!(p.counts().1, 0);
    }

    #[test]
    fn failed_promotion_hands_the_permit_back_for_a_clean_close() {
        let p = HandshakePools::new(HandshakeLimits {
            half_open: 2,
            authenticated: 1,
            active: 1,
        });
        let a = p.begin().unwrap().authenticate().unwrap();
        let h = p.begin().unwrap();
        let (h, why) = h.authenticate().unwrap_err();
        assert_eq!(why, HandshakeDenied::AuthenticatedFull);
        assert_eq!(p.counts(), (1, 1, 0), "no slot leaked or double-counted");
        drop(h);
        let act = a.activate().unwrap();
        assert_eq!(p.counts(), (0, 0, 1));
        drop(act);
        assert_eq!(p.counts(), (0, 0, 0));
    }
}
