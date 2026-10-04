//! §129 unknown-peer admission, §130 connection admission.
//!
//! Both are *ordered* pipelines: the cheapest checks first, each one
//! rejecting before the next, more expensive, stage is reached, and
//! refundable resources (slots) taken before non-refundable ones
//! (rate tokens) so a late failure doesn't burn the caller's budget.
//!
//! §129: length → syntax → class-wide rate → (under load) stateless
//! cookie → per-source rate → *then* the caller may do real crypto.
//! Per-source state is created only for sources that proved they can
//! receive replies (valid cookie); before that, only the one shared
//! unknown-class bucket is touched, so a flood of spoofed sources cannot
//! grow any table.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::connections::{ConnectionLimiter, ConnectionPermit, LimitDenied, PeerKey};
use crate::storm::{ConnectionStormGuard, SourceClass, StormDecision, StormPolicy};
use crate::token_bucket::TokenBucket;
use crate::verification::{HalfOpen, HandshakeDenied, HandshakeLimits, HandshakePools};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------- cookies

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cookie(pub [u8; 16]);

/// Stateless address-ownership challenge: cookie = keyed-BLAKE3(source ‖
/// time window). Nothing is stored per challenged source. A cookie is
/// valid in its own and the following window, so a cookie issued just
/// before a boundary still works.
#[derive(Debug, Clone)]
pub struct CookieIssuer {
    key: [u8; 32],
    window_millis: u64,
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl CookieIssuer {
    pub fn new(key: [u8; 32], window_millis: u64) -> Self {
        Self {
            key,
            window_millis: window_millis.max(1),
        }
    }

    fn cookie_at(&self, source: &[u8], window: u64) -> Cookie {
        let mut h = blake3::Hasher::new_keyed(&self.key);
        h.update(&(source.len() as u64).to_le_bytes()); // length-prefixed: no ambiguity
        h.update(source);
        h.update(&window.to_le_bytes());
        let d = h.finalize();
        let mut c = [0u8; 16];
        c.copy_from_slice(&d.as_bytes()[..16]);
        Cookie(c)
    }

    pub fn issue(&self, source: &[u8], now_millis: u64) -> Cookie {
        self.cookie_at(source, now_millis / self.window_millis)
    }

    pub fn verify(&self, source: &[u8], cookie: &Cookie, now_millis: u64) -> bool {
        let w = now_millis / self.window_millis;
        // Check both windows without short-circuiting on the first.
        let cur = ct_eq(&self.cookie_at(source, w).0, &cookie.0);
        let prev = w
            .checked_sub(1)
            .is_some_and(|p| ct_eq(&self.cookie_at(source, p).0, &cookie.0));
        cur | prev
    }
}

// ------------------------------------------------------------- §129

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownGateLimits {
    pub min_hello_bytes: usize,
    pub max_hello_bytes: usize,
    /// (burst, per_sec) shared by *all* unknown sources.
    pub class_rate: (u64, u64),
    /// (burst, per_sec) per cookie-validated source.
    pub per_source_rate: (u64, u64),
    pub max_tracked_sources: usize,
}

impl Default for UnknownGateLimits {
    fn default() -> Self {
        Self {
            min_hello_bytes: 8,
            max_hello_bytes: 4_096,
            class_rate: (50, 20),
            per_source_rate: (5, 1),
            max_tracked_sources: 1_024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownHello<'a> {
    /// Transport-level source identity (e.g. address bytes).
    pub source: &'a [u8],
    pub payload_len: usize,
    /// Result of the caller's cheap structural parse (no crypto).
    pub syntax_ok: bool,
    pub cookie: Option<Cookie>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UnknownReject {
    #[error("hello has an implausible length")]
    BadLength,
    #[error("hello is malformed")]
    BadSyntax,
    #[error("unknown peers are being rate limited")]
    RateLimited,
    /// Not an error as such: answer with this cookie and wait for the
    /// source to echo it. Costs the server no state.
    #[error("challenge the source with a cookie")]
    Challenge(Cookie),
    #[error("cookie is invalid or expired")]
    BadCookie,
    #[error("this source is rate limited")]
    SourceRateLimited,
}

#[derive(Debug)]
struct GateState {
    class: TokenBucket,
    sources: HashMap<Vec<u8>, TokenBucket>,
}

#[derive(Debug)]
pub struct UnknownPeerGate {
    limits: UnknownGateLimits,
    cookies: CookieIssuer,
    state: Mutex<GateState>,
}

impl UnknownPeerGate {
    pub fn new(limits: UnknownGateLimits, cookies: CookieIssuer, now_millis: u64) -> Self {
        Self {
            limits,
            cookies,
            state: Mutex::new(GateState {
                class: TokenBucket::new(limits.class_rate.0, limits.class_rate.1, now_millis),
                sources: HashMap::new(),
            }),
        }
    }

    /// `Ok(())` means "proceed to the expensive handshake". `under_load`
    /// makes the cookie mandatory.
    pub fn check(
        &self,
        h: &UnknownHello<'_>,
        now_millis: u64,
        under_load: bool,
    ) -> Result<(), UnknownReject> {
        let l = &self.limits;
        // 1. length  2. syntax — pure functions of the input, no state.
        if h.payload_len < l.min_hello_bytes || h.payload_len > l.max_hello_bytes {
            return Err(UnknownReject::BadLength);
        }
        if !h.syntax_ok {
            return Err(UnknownReject::BadSyntax);
        }
        let mut st = lock(&self.state);
        // 3. shared class-wide rate: the only state a spoofed flood touches.
        if !st.class.try_consume(1, now_millis) {
            return Err(UnknownReject::RateLimited);
        }
        // 4. cookie, when load demands proof the source can receive replies.
        if under_load {
            match &h.cookie {
                None => {
                    return Err(UnknownReject::Challenge(
                        self.cookies.issue(h.source, now_millis),
                    ))
                }
                Some(c) if !self.cookies.verify(h.source, c, now_millis) => {
                    return Err(UnknownReject::BadCookie)
                }
                Some(_) => {}
            }
        }
        // 5. per-source rate — but only for sources that passed the cookie
        //    (or when no load-shedding is active and none is demanded).
        if under_load || h.cookie.is_some() {
            if !st.sources.contains_key(h.source) && st.sources.len() >= l.max_tracked_sources {
                let idlest = st
                    .sources
                    .iter_mut()
                    .map(|(k, b)| (k.clone(), b.available(now_millis)))
                    .max_by_key(|(_, a)| *a)
                    .map(|(k, _)| k);
                if let Some(k) = idlest {
                    st.sources.remove(&k);
                }
            }
            let (burst, rate) = l.per_source_rate;
            let b = st
                .sources
                .entry(h.source.to_vec())
                .or_insert_with(|| TokenBucket::new(burst, rate, now_millis));
            if !b.try_consume(1, now_millis) {
                return Err(UnknownReject::SourceRateLimited);
            }
        }
        Ok(())
    }

    pub fn tracked_sources(&self) -> usize {
        lock(&self.state).sources.len()
    }
}

// ------------------------------------------------------------- §130

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionAdmissionLimits {
    pub max_sessions: u32,
    pub max_per_source: u32,
    pub handshakes: HandshakeLimits,
    pub storm: StormPolicy,
    /// (burst, per_sec) of new connections from one source.
    pub per_source_rate: (u64, u64),
    pub max_tracked_sources: usize,
}

impl Default for ConnectionAdmissionLimits {
    fn default() -> Self {
        Self {
            max_sessions: 128,
            max_per_source: 4,
            handshakes: HandshakeLimits::default(),
            storm: StormPolicy::default(),
            per_source_rate: (4, 1),
            max_tracked_sources: 1_024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionReject {
    #[error("no connection slot: {0}")]
    NoSlot(LimitDenied),
    #[error("handshake budget exhausted")]
    HandshakeBudget(HandshakeDenied),
    #[error("this source is opening connections too fast")]
    SourceRate,
    #[error("connection flood protection: {0:?}")]
    Storm(StormDecision),
}

/// Holds the connection slot and the half-open handshake slot; drop both
/// (connection closed / handshake failed) to free them. Promote the
/// half-open permit with [`HalfOpen::authenticate`] once the handshake
/// completes.
#[derive(Debug)]
#[must_use]
pub struct AdmittedConnection {
    pub slot: ConnectionPermit,
    pub half_open: HalfOpen,
}

#[derive(Debug)]
struct Rates {
    storm: ConnectionStormGuard,
    sources: HashMap<PeerKey, TokenBucket>,
}

#[derive(Debug)]
pub struct ConnectionAdmission {
    limits: ConnectionAdmissionLimits,
    slots: ConnectionLimiter,
    handshakes: HandshakePools,
    rates: Mutex<Rates>,
}

impl ConnectionAdmission {
    pub fn new(limits: ConnectionAdmissionLimits, now_millis: u64) -> Self {
        Self {
            limits,
            slots: ConnectionLimiter::new(limits.max_sessions, limits.max_per_source),
            handshakes: HandshakePools::new(limits.handshakes),
            rates: Mutex::new(Rates {
                storm: ConnectionStormGuard::new(limits.storm, now_millis),
                sources: HashMap::new(),
            }),
        }
    }

    /// Before any session setup: global slot → handshake budget (both
    /// refundable) → per-source rate → class/global storm limits.
    pub fn try_admit(
        &self,
        source: PeerKey,
        class: SourceClass,
        now_millis: u64,
    ) -> Result<AdmittedConnection, ConnectionReject> {
        let slot = self
            .slots
            .try_acquire(source)
            .map_err(ConnectionReject::NoSlot)?;
        let half_open = self
            .handshakes
            .begin()
            .map_err(ConnectionReject::HandshakeBudget)?;

        let mut r = lock(&self.rates);
        if !r.sources.contains_key(&source) && r.sources.len() >= self.limits.max_tracked_sources {
            let idlest = r
                .sources
                .iter_mut()
                .map(|(k, b)| (*k, b.available(now_millis)))
                .max_by_key(|(_, a)| *a)
                .map(|(k, _)| k);
            if let Some(k) = idlest {
                r.sources.remove(&k);
            }
        }
        let (burst, rate) = self.limits.per_source_rate;
        if !r
            .sources
            .entry(source)
            .or_insert_with(|| TokenBucket::new(burst, rate, now_millis))
            .try_consume(1, now_millis)
        {
            return Err(ConnectionReject::SourceRate);
        }
        match r.storm.admit(class, now_millis) {
            StormDecision::Accept => {}
            other => return Err(ConnectionReject::Storm(other)),
        }
        drop(r);
        Ok(AdmittedConnection { slot, half_open })
    }

    pub fn open_connections(&self) -> u32 {
        self.slots.active()
    }

    pub fn handshake_counts(&self) -> (u64, u64, u64) {
        self.handshakes.counts()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> CookieIssuer {
        CookieIssuer::new([9; 32], 10_000)
    }

    fn hello<'a>(src: &'a [u8], cookie: Option<Cookie>) -> UnknownHello<'a> {
        UnknownHello {
            source: src,
            payload_len: 100,
            syntax_ok: true,
            cookie,
        }
    }

    fn gate(l: UnknownGateLimits) -> UnknownPeerGate {
        UnknownPeerGate::new(l, issuer(), 0)
    }

    #[test]
    fn cookies_are_stateless_source_bound_and_window_limited() {
        let i = issuer();
        let c = i.issue(b"1.2.3.4", 5_000);
        assert!(i.verify(b"1.2.3.4", &c, 5_001));
        assert!(
            i.verify(b"1.2.3.4", &c, 19_999),
            "still valid in the next window"
        );
        assert!(
            !i.verify(b"1.2.3.4", &c, 20_000),
            "expired after two windows"
        );
        assert!(!i.verify(b"5.6.7.8", &c, 5_001), "bound to the source");
        let other = CookieIssuer::new([1; 32], 10_000);
        assert!(
            !other.verify(b"1.2.3.4", &c, 5_001),
            "bound to the server key"
        );
        assert!(!i.verify(b"1.2.3.4", &Cookie([0; 16]), 5_000));
    }

    #[test]
    fn cheap_structural_checks_run_first_and_cost_no_tokens() {
        let g = gate(UnknownGateLimits {
            class_rate: (3, 0),
            ..Default::default()
        });
        for _ in 0..100 {
            assert_eq!(
                g.check(
                    &UnknownHello {
                        payload_len: 1,
                        ..hello(b"a", None)
                    },
                    0,
                    false
                ),
                Err(UnknownReject::BadLength)
            );
            assert_eq!(
                g.check(
                    &UnknownHello {
                        payload_len: 99_999,
                        ..hello(b"a", None)
                    },
                    0,
                    false
                ),
                Err(UnknownReject::BadLength)
            );
            assert_eq!(
                g.check(
                    &UnknownHello {
                        syntax_ok: false,
                        ..hello(b"a", None)
                    },
                    0,
                    false
                ),
                Err(UnknownReject::BadSyntax)
            );
        }
        assert!(
            (0..3).all(|_| g.check(&hello(b"a", None), 0, false).is_ok()),
            "all 3 tokens intact"
        );
        assert_eq!(
            g.check(&hello(b"a", None), 0, false),
            Err(UnknownReject::RateLimited)
        );
    }

    #[test]
    fn under_load_a_cookieless_hello_is_challenged_not_processed() {
        let g = gate(UnknownGateLimits::default());
        let Err(UnknownReject::Challenge(c)) = g.check(&hello(b"src", None), 0, true) else {
            panic!("expected a challenge");
        };
        assert_eq!(g.tracked_sources(), 0, "challenging stores nothing");
        assert_eq!(g.check(&hello(b"src", Some(c)), 1, true), Ok(()));
        assert_eq!(
            g.check(&hello(b"src", Some(Cookie([0; 16]))), 2, true),
            Err(UnknownReject::BadCookie)
        );
    }

    #[test]
    fn a_spoofed_source_flood_grows_no_per_source_state() {
        let g = gate(UnknownGateLimits {
            class_rate: (10_000, 0),
            ..Default::default()
        });
        for i in 0..5_000u32 {
            let src = i.to_le_bytes();
            let _ = g.check(&hello(&src, None), 0, true); // all challenged
        }
        assert_eq!(g.tracked_sources(), 0);
    }

    #[test]
    fn validated_sources_are_rate_limited_individually_and_the_table_is_bounded() {
        let l = UnknownGateLimits {
            per_source_rate: (2, 0),
            max_tracked_sources: 3,
            class_rate: (1_000, 0),
            ..Default::default()
        };
        let g = gate(l);
        let ck = |s: &[u8]| issuer().issue(s, 0);
        assert_eq!(g.check(&hello(b"a", Some(ck(b"a"))), 0, true), Ok(()));
        assert_eq!(g.check(&hello(b"a", Some(ck(b"a"))), 0, true), Ok(()));
        assert_eq!(
            g.check(&hello(b"a", Some(ck(b"a"))), 0, true),
            Err(UnknownReject::SourceRateLimited)
        );
        assert_eq!(
            g.check(&hello(b"b", Some(ck(b"b"))), 0, true),
            Ok(()),
            "b has its own bucket"
        );
        for s in [b"c", b"d", b"e", b"f"] {
            let _ = g.check(&hello(s, Some(ck(s))), 0, true);
        }
        assert!(g.tracked_sources() <= 3);
    }

    #[test]
    fn class_rate_applies_before_cookie_work() {
        let g = gate(UnknownGateLimits {
            class_rate: (1, 0),
            ..Default::default()
        });
        assert!(matches!(
            g.check(&hello(b"x", None), 0, true),
            Err(UnknownReject::Challenge(_))
        ));
        assert_eq!(
            g.check(&hello(b"y", None), 0, true),
            Err(UnknownReject::RateLimited)
        );
    }

    fn adm(l: ConnectionAdmissionLimits) -> ConnectionAdmission {
        ConnectionAdmission::new(l, 0)
    }

    #[test]
    fn admitted_connection_holds_a_slot_and_a_half_open_handshake() {
        let a = adm(ConnectionAdmissionLimits::default());
        let c = a.try_admit([1; 32], SourceClass::Known, 0).unwrap();
        assert_eq!((a.open_connections(), a.handshake_counts()), (1, (1, 0, 0)));
        let auth = c.half_open.authenticate().unwrap();
        assert_eq!(a.handshake_counts(), (0, 1, 0));
        drop(auth);
        drop(c.slot);
        assert_eq!((a.open_connections(), a.handshake_counts()), (0, (0, 0, 0)));
    }

    #[test]
    fn handshake_budget_failure_releases_the_connection_slot() {
        let l = ConnectionAdmissionLimits {
            handshakes: HandshakeLimits {
                half_open: 1,
                authenticated: 1,
                active: 1,
            },
            ..Default::default()
        };
        let a = adm(l);
        let _first = a.try_admit([1; 32], SourceClass::Known, 0).unwrap();
        assert_eq!(
            a.try_admit([2; 32], SourceClass::Known, 0).unwrap_err(),
            ConnectionReject::HandshakeBudget(HandshakeDenied::HalfOpenFull)
        );
        assert_eq!(
            a.open_connections(),
            1,
            "the refused attempt's slot was returned"
        );
    }

    #[test]
    fn per_source_rate_and_slot_caps() {
        let l = ConnectionAdmissionLimits {
            per_source_rate: (2, 0),
            max_per_source: 10,
            ..Default::default()
        };
        let a = adm(l);
        let _a1 = a.try_admit([1; 32], SourceClass::Known, 0).unwrap();
        let _a2 = a.try_admit([1; 32], SourceClass::Known, 0).unwrap();
        assert_eq!(
            a.try_admit([1; 32], SourceClass::Known, 0).unwrap_err(),
            ConnectionReject::SourceRate
        );
        assert!(a.try_admit([2; 32], SourceClass::Known, 0).is_ok());
        let l2 = ConnectionAdmissionLimits {
            max_per_source: 1,
            ..Default::default()
        };
        let b = adm(l2);
        let _x = b.try_admit([7; 32], SourceClass::Known, 0).unwrap();
        assert_eq!(
            b.try_admit([7; 32], SourceClass::Known, 0).unwrap_err(),
            ConnectionReject::NoSlot(LimitDenied::PerPeer)
        );
    }

    #[test]
    fn a_rotating_unknown_flood_cannot_starve_trusted_connections() {
        let a = adm(ConnectionAdmissionLimits::default());
        let mut held = Vec::new();
        let mut unknown_ok = 0;
        for i in 0..500u32 {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_le_bytes());
            if let Ok(c) = a.try_admit(k, SourceClass::Unknown, 0) {
                unknown_ok += 1;
                held.push(c);
            }
        }
        assert_eq!(
            unknown_ok, 8,
            "capped by the unknown class bucket (burst 8)"
        );
        assert!(a.try_admit([200; 32], SourceClass::Trusted, 0).is_ok());
    }

    #[test]
    fn the_per_source_table_is_bounded() {
        let l = ConnectionAdmissionLimits {
            max_tracked_sources: 4,
            storm: StormPolicy {
                global: crate::storm::Rate {
                    burst: 1_000_000,
                    per_sec: 0,
                },
                trusted: crate::storm::Rate {
                    burst: 1_000_000,
                    per_sec: 0,
                },
                known: crate::storm::Rate {
                    burst: 1_000_000,
                    per_sec: 0,
                },
                unknown: crate::storm::Rate {
                    burst: 1_000_000,
                    per_sec: 0,
                },
            },
            ..Default::default()
        };
        let a = adm(l);
        for i in 0..50u8 {
            let _ = a.try_admit([i; 32], SourceClass::Known, 0).map(drop);
        }
        assert!(lock(&a.rates).sources.len() <= 4);
    }
}
