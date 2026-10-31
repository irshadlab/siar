//! §60-§62, §65-§66: the admission gate for untrusted inbound work.
//!
//! Order is the spec's (§61), and **each step is cheaper than the next**, so
//! hostile input is shed as early and as cheaply as possible:
//!
//! 1. length, 2. version — constant time, touch no state;
//! 3. blocklist — one hash lookup, **before any rate-limit state is created**
//!    (a blocked peer cannot even grow our tables);
//! 4. rate limit — a per-peer bucket, plus a **global** bucket shared by all
//!    unknown peers;
//! 5. duplicate check;
//! 6. authorization class.
//!
//! Only a returned [`Admit`] entitles the caller to spend CPU on signature or
//! handshake verification.
//!
//! **Sybil (§66), stated plainly:** nothing here stops someone creating many
//! identities. What bounds the damage is that unknown peers *share* a global
//! budget, so 10 000 fresh identities get the same total allowance as one,
//! and that per-peer state is capped and evicts the least recently seen.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::bucket::Bucket;

/// Opaque, caller-scoped peer key (§35: derive it per context, never reuse a
/// global identifier here).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerKey(pub [u8; 16]);

/// §62's separate quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum QuotaKind {
    Handshake = 0,
    MessageRequest = 1,
    FileOffer = 2,
    PairingAttempt = 3,
    DtnBundle = 4,
    SignatureVerify = 5,
}

const KINDS: usize = 6;

impl QuotaKind {
    pub const ALL: [Self; KINDS] = [
        Self::Handshake,
        Self::MessageRequest,
        Self::FileOffer,
        Self::PairingAttempt,
        Self::DtnBundle,
        Self::SignatureVerify,
    ];

    const fn idx(self) -> usize {
        self as usize
    }
}

/// How much we trust a peer (derived from trust state by the caller, §40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Standing {
    Blocked,
    Unknown,
    Contact,
    Verified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub burst: u32,
    /// One more operation allowed per this many milliseconds. 0 = unlimited.
    pub period_ms: u64,
}

const fn l(burst: u32, period_ms: u64) -> Limit {
    Limit { burst, period_ms }
}

#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Per-peer limits for an unknown peer, by [`QuotaKind`] order.
    pub unknown_peer: [Limit; KINDS],
    /// Shared by **all** unknown peers together (the Sybil bound).
    pub unknown_global: [Limit; KINDS],
    pub contact: [Limit; KINDS],
    pub verified: [Limit; KINDS],
    /// Largest accepted size per kind (bytes), checked first.
    pub max_len: [usize; KINDS],
    pub accepted_versions: (u8, u8),
    /// Most distinct peers whose state we keep (LRU eviction beyond).
    pub max_tracked_peers: usize,
    pub max_blocked: usize,
    pub dup_window: usize,
    /// §60 "pairing UI": consecutive failures before cooldown starts.
    pub pairing_failures_before_cooldown: u32,
    pub pairing_cooldown_base_ms: u64,
    pub pairing_cooldown_max_ms: u64,
}

impl Default for GateConfig {
    fn default() -> Self {
        const MIN: u64 = 60_000;
        const HOUR: u64 = 60 * MIN;
        Self {
            //                  handshake      msg request    file offer     pairing         dtn bundle     sig verify
            unknown_peer: [
                l(3, MIN / 3),
                l(5, HOUR / 5),
                l(2, HOUR / 2),
                l(5, 10 * MIN / 5),
                l(20, 3_000),
                l(10, 1_000),
            ],
            unknown_global: [
                l(30, 2_000),
                l(100, 36_000),
                l(20, 180_000),
                l(30, 20_000),
                l(200, 300),
                l(200, 100),
            ],
            contact: [
                l(20, 3_000),
                l(0, 0),
                l(30, 20_000),
                l(10, 10_000),
                l(200, 300),
                l(0, 0),
            ],
            verified: [
                l(60, 1_000),
                l(0, 0),
                l(120, 5_000),
                l(10, 10_000),
                l(0, 0),
                l(0, 0),
            ],
            max_len: [
                4 * 1024,
                64 * 1024,
                8 * 1024,
                2 * 1024,
                256 * 1024,
                16 * 1024,
            ],
            accepted_versions: (1, 1),
            max_tracked_peers: 4_096,
            max_blocked: 100_000,
            dup_window: 65_536,
            pairing_failures_before_cooldown: 3,
            pairing_cooldown_base_ms: 30_000,
            pairing_cooldown_max_ms: 60 * MIN,
        }
    }
}

/// One inbound item, described **without looking inside it**.
#[derive(Debug, Clone, Copy)]
pub struct Incoming {
    pub peer: PeerKey,
    pub kind: QuotaKind,
    pub len: usize,
    pub version: u8,
    /// A message/bundle/offer id for replay suppression, if it has one.
    pub id: Option<[u8; 16]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    TooLong,
    BadVersion,
    Blocked,
    RateLimited {
        retry_after_ms: u64,
    },
    /// Unknown peers as a group are over budget (the Sybil bound).
    GlobalLimited {
        retry_after_ms: u64,
    },
    Duplicate,
    PairingCooldown {
        retry_after_ms: u64,
    },
}

/// Permission to spend crypto/CPU on this item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admit {
    pub standing: Standing,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GateStats {
    pub admitted: u64,
    pub too_long: u64,
    pub bad_version: u64,
    pub blocked: u64,
    pub rate_limited: u64,
    pub global_limited: u64,
    pub duplicates: u64,
    pub pairing_cooldown: u64,
}

struct PeerState {
    buckets: [Bucket; KINDS],
    last_seen_ms: u64,
    pairing_failures: u32,
    pairing_strikes: u32,
    cooldown_until_ms: u64,
}

pub struct Gate {
    cfg: GateConfig,
    blocked: HashSet<PeerKey>,
    standings: HashMap<PeerKey, Standing>,
    peers: HashMap<PeerKey, PeerState>,
    global_unknown: [Bucket; KINDS],
    seen: HashSet<(u8, [u8; 16])>,
    seen_order: VecDeque<(u8, [u8; 16])>,
    stats: GateStats,
}

fn buckets(limits: &[Limit; KINDS], now: u64) -> [Bucket; KINDS] {
    std::array::from_fn(|i| Bucket::new(limits[i].burst, limits[i].period_ms, now))
}

impl Gate {
    #[must_use]
    pub fn new(cfg: GateConfig, now_ms: u64) -> Self {
        let global_unknown = buckets(&cfg.unknown_global, now_ms);
        Self {
            cfg,
            blocked: HashSet::new(),
            standings: HashMap::new(),
            peers: HashMap::new(),
            global_unknown,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            stats: GateStats::default(),
        }
    }

    #[must_use]
    pub const fn stats(&self) -> GateStats {
        self.stats
    }

    #[must_use]
    pub fn tracked_peers(&self) -> usize {
        self.peers.len()
    }

    /// §65. Returns `false` if the blocklist is full (the caller should say so).
    pub fn block(&mut self, p: PeerKey) -> bool {
        if self.blocked.len() >= self.cfg.max_blocked && !self.blocked.contains(&p) {
            return false;
        }
        self.blocked.insert(p);
        // Blocking revokes standing; unblocking must not silently restore trust.
        self.standings.remove(&p);
        self.peers.remove(&p);
        true
    }

    pub fn unblock(&mut self, p: PeerKey) {
        self.blocked.remove(&p);
    }

    #[must_use]
    pub fn is_blocked(&self, p: PeerKey) -> bool {
        self.blocked.contains(&p)
    }

    /// Record the trust-derived standing of a peer (unknown peers are the default).
    pub fn set_standing(&mut self, p: PeerKey, s: Standing) {
        match s {
            Standing::Blocked => {
                self.block(p);
            }
            Standing::Unknown => {
                self.standings.remove(&p);
            }
            other => {
                self.blocked.remove(&p);
                self.standings.insert(p, other);
                // Re-created lazily with the new standing's (larger) limits.
                self.peers.remove(&p);
            }
        }
    }

    #[must_use]
    pub fn standing(&self, p: PeerKey) -> Standing {
        if self.blocked.contains(&p) {
            Standing::Blocked
        } else {
            self.standings.get(&p).copied().unwrap_or(Standing::Unknown)
        }
    }

    /// The §61 pipeline. Cheap checks first; nothing is allocated for a peer
    /// until every earlier check has passed.
    pub fn admit(&mut self, inc: &Incoming, now_ms: u64) -> Result<Admit, Reject> {
        let k = inc.kind.idx();
        // 1. length  2. version — constant time, no state.
        if inc.len > self.cfg.max_len[k] {
            self.stats.too_long += 1;
            return Err(Reject::TooLong);
        }
        if inc.version < self.cfg.accepted_versions.0 || inc.version > self.cfg.accepted_versions.1
        {
            self.stats.bad_version += 1;
            return Err(Reject::BadVersion);
        }
        // 3. blocklist — before any per-peer state exists.
        if self.blocked.contains(&inc.peer) {
            self.stats.blocked += 1;
            return Err(Reject::Blocked);
        }
        let standing = self.standing(inc.peer);

        // Pairing cooldown (§60 pairing UI) is checked before spending a token.
        if inc.kind == QuotaKind::PairingAttempt {
            if let Some(p) = self.peers.get(&inc.peer) {
                if now_ms < p.cooldown_until_ms {
                    self.stats.pairing_cooldown += 1;
                    return Err(Reject::PairingCooldown {
                        retry_after_ms: p.cooldown_until_ms - now_ms,
                    });
                }
            }
        }

        // 4. rate limits: per-peer, then (for unknown peers) the shared global bucket.
        self.ensure_peer(inc.peer, standing, now_ms);
        let peer = self.peers.get_mut(&inc.peer).expect("just ensured");
        peer.last_seen_ms = now_ms;
        if let Err(retry_after_ms) = peer.buckets[k].take(1, now_ms) {
            self.stats.rate_limited += 1;
            return Err(Reject::RateLimited { retry_after_ms });
        }
        if standing == Standing::Unknown {
            if let Err(retry_after_ms) = self.global_unknown[k].take(1, now_ms) {
                self.stats.global_limited += 1;
                return Err(Reject::GlobalLimited { retry_after_ms });
            }
        }
        // 5. duplicate (after the rate limit: a replay flood is still limited).
        if let Some(id) = inc.id {
            let key = (inc.kind as u8, id);
            if !self.seen.insert(key) {
                self.stats.duplicates += 1;
                return Err(Reject::Duplicate);
            }
            self.seen_order.push_back(key);
            while self.seen_order.len() > self.cfg.dup_window {
                if let Some(old) = self.seen_order.pop_front() {
                    self.seen.remove(&old);
                }
            }
        }
        // 6. the caller applies authorization using `standing`.
        self.stats.admitted += 1;
        Ok(Admit { standing })
    }

    fn ensure_peer(&mut self, p: PeerKey, standing: Standing, now_ms: u64) {
        if self.peers.contains_key(&p) {
            return;
        }
        if self.peers.len() >= self.cfg.max_tracked_peers {
            // Evict the least recently seen *unknown* peer first, so a flood
            // of fresh identities cannot push out a verified contact's state.
            let victim = self
                .peers
                .iter()
                .filter(|(k, _)| !self.standings.contains_key(*k))
                .min_by_key(|(_, s)| s.last_seen_ms)
                .or_else(|| self.peers.iter().min_by_key(|(_, s)| s.last_seen_ms))
                .map(|(k, _)| *k);
            if let Some(v) = victim {
                self.peers.remove(&v);
            }
        }
        let limits = match standing {
            Standing::Verified => &self.cfg.verified,
            Standing::Contact => &self.cfg.contact,
            _ => &self.cfg.unknown_peer,
        };
        self.peers.insert(
            p,
            PeerState {
                buckets: buckets(limits, now_ms),
                last_seen_ms: now_ms,
                pairing_failures: 0,
                pairing_strikes: 0,
                cooldown_until_ms: 0,
            },
        );
    }

    /// A pairing attempt from `peer` failed (bad token, SAS mismatch…).
    /// After a few in a row the source is put on an exponentially growing
    /// cooldown so it cannot spam the user's pairing UI (§60).
    pub fn record_pairing_failure(&mut self, p: PeerKey, now_ms: u64) {
        let standing = self.standing(p);
        self.ensure_peer(p, standing, now_ms);
        let (threshold, base, max) = (
            self.cfg.pairing_failures_before_cooldown,
            self.cfg.pairing_cooldown_base_ms,
            self.cfg.pairing_cooldown_max_ms,
        );
        let peer = self.peers.get_mut(&p).expect("ensured");
        peer.pairing_failures += 1;
        if peer.pairing_failures >= threshold {
            let span = base
                .saturating_mul(1u64 << peer.pairing_strikes.min(20))
                .min(max);
            peer.pairing_strikes = peer.pairing_strikes.saturating_add(1);
            peer.pairing_failures = 0;
            peer.cooldown_until_ms = now_ms.saturating_add(span);
        }
    }

    pub fn record_pairing_success(&mut self, p: PeerKey) {
        if let Some(peer) = self.peers.get_mut(&p) {
            peer.pairing_failures = 0;
            peer.pairing_strikes = 0;
            peer.cooldown_until_ms = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u32) -> PeerKey {
        let mut b = [0u8; 16];
        b[..4].copy_from_slice(&n.to_le_bytes());
        PeerKey(b)
    }
    fn inc(peer: PeerKey, kind: QuotaKind) -> Incoming {
        Incoming {
            peer,
            kind,
            len: 100,
            version: 1,
            id: None,
        }
    }

    #[test]
    fn oversize_and_wrong_version_are_shed_without_touching_any_state() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let p = pk(1);
        for _ in 0..10_000 {
            assert_eq!(
                g.admit(
                    &Incoming {
                        len: usize::MAX,
                        ..inc(p, QuotaKind::Handshake)
                    },
                    0
                ),
                Err(Reject::TooLong)
            );
            assert_eq!(
                g.admit(
                    &Incoming {
                        version: 9,
                        ..inc(p, QuotaKind::Handshake)
                    },
                    0
                ),
                Err(Reject::BadVersion)
            );
        }
        assert_eq!(
            g.tracked_peers(),
            0,
            "§61: nothing allocated for an item that fails the cheap checks"
        );
        // …and they did not consume the peer's budget.
        for _ in 0..3 {
            assert!(g.admit(&inc(p, QuotaKind::Handshake), 0).is_ok());
        }
    }

    #[test]
    fn a_blocked_peer_is_rejected_before_any_state_is_created() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let evil = pk(666);
        g.block(evil);
        for i in 0..50_000u32 {
            let r = g.admit(
                &Incoming {
                    id: Some([i as u8; 16]),
                    ..inc(evil, QuotaKind::MessageRequest)
                },
                u64::from(i),
            );
            assert_eq!(r, Err(Reject::Blocked));
        }
        assert_eq!(g.tracked_peers(), 0);
        assert_eq!(g.stats().blocked, 50_000);
        assert_eq!(
            g.stats().rate_limited,
            0,
            "blocking happens before rate limiting"
        );
    }

    #[test]
    fn blocking_a_known_peer_wins_over_their_standing_and_unblocking_restores_it() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let p = pk(2);
        g.set_standing(p, Standing::Verified);
        assert_eq!(
            g.admit(&inc(p, QuotaKind::MessageRequest), 0)
                .unwrap()
                .standing,
            Standing::Verified
        );
        g.block(p);
        assert_eq!(
            g.admit(&inc(p, QuotaKind::MessageRequest), 1),
            Err(Reject::Blocked)
        );
        g.unblock(p);
        assert_eq!(
            g.standing(p),
            Standing::Unknown,
            "unblocking does not silently restore trust"
        );
    }

    #[test]
    fn unknown_peers_are_limited_per_kind_and_recover_with_time() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let p = pk(3);
        for _ in 0..3 {
            assert!(g.admit(&inc(p, QuotaKind::Handshake), 0).is_ok());
        }
        let e = g.admit(&inc(p, QuotaKind::Handshake), 0).unwrap_err();
        assert!(matches!(e, Reject::RateLimited { retry_after_ms } if retry_after_ms > 0));
        // Quotas are separate: handshakes being exhausted does not block a message request.
        assert!(g.admit(&inc(p, QuotaKind::MessageRequest), 0).is_ok());
        assert!(
            g.admit(&inc(p, QuotaKind::Handshake), 20_000).is_ok(),
            "refilled after one period"
        );
    }

    #[test]
    fn verified_peers_get_far_more_than_unknown_ones() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let (known, stranger) = (pk(4), pk(5));
        g.set_standing(known, Standing::Verified);
        let count = |g: &mut Gate, p| {
            (0..500)
                .filter(|_| g.admit(&inc(p, QuotaKind::Handshake), 0).is_ok())
                .count()
        };
        let (a, b) = (count(&mut g, known), count(&mut g, stranger));
        assert!(a >= 60 && b <= 3 && a > 10 * b, "verified={a} unknown={b}");
    }

    #[test]
    fn many_fresh_identities_share_one_global_budget() {
        // §66: the attacker makes a new identity for every request, each well
        // inside its own per-peer limit. The shared budget still caps the total.
        let mut g = Gate::new(GateConfig::default(), 0);
        let mut admitted = 0;
        for i in 0..20_000u32 {
            if g.admit(&inc(pk(1_000 + i), QuotaKind::MessageRequest), 0)
                .is_ok()
            {
                admitted += 1;
            }
        }
        assert!(
            admitted <= 100,
            "global unknown message-request burst is 100, admitted {admitted}"
        );
        assert!(g.stats().global_limited > 19_000);
        assert!(
            g.tracked_peers() <= GateConfig::default().max_tracked_peers,
            "per-peer state stays bounded"
        );
    }

    #[test]
    fn a_stranger_flood_does_not_starve_or_evict_a_verified_contact() {
        let mut g = Gate::new(
            GateConfig {
                max_tracked_peers: 64,
                ..GateConfig::default()
            },
            0,
        );
        let friend = pk(7);
        g.set_standing(friend, Standing::Verified);
        assert!(g.admit(&inc(friend, QuotaKind::Handshake), 0).is_ok());
        for i in 0..10_000u32 {
            let _ = g.admit(&inc(pk(50_000 + i), QuotaKind::Handshake), u64::from(i));
        }
        assert!(g.tracked_peers() <= 64);
        assert!(
            g.admit(&inc(friend, QuotaKind::Handshake), 10_001).is_ok(),
            "the friend is unaffected by the flood"
        );
    }

    #[test]
    fn duplicates_are_suppressed_but_still_cost_budget() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let p = pk(8);
        let with_id = Incoming {
            id: Some([9; 16]),
            ..inc(p, QuotaKind::DtnBundle)
        };
        assert!(g.admit(&with_id, 0).is_ok());
        assert_eq!(g.admit(&with_id, 0), Err(Reject::Duplicate));
        // Same id under a different kind is a different item.
        assert!(g
            .admit(
                &Incoming {
                    kind: QuotaKind::MessageRequest,
                    ..with_id
                },
                0
            )
            .is_ok());
    }

    #[test]
    fn the_duplicate_window_is_bounded() {
        let mut g = Gate::new(
            GateConfig {
                dup_window: 100,
                ..GateConfig::default()
            },
            0,
        );
        let p = pk(9);
        g.set_standing(p, Standing::Verified);
        for i in 0..1_000u32 {
            let mut id = [0u8; 16];
            id[..4].copy_from_slice(&i.to_le_bytes());
            let _ = g.admit(
                &Incoming {
                    id: Some(id),
                    ..inc(p, QuotaKind::FileOffer)
                },
                u64::from(i) * 10_000,
            );
        }
        assert!(g.seen.len() <= 100 && g.seen_order.len() <= 100);
    }

    #[test]
    fn repeated_pairing_failures_trigger_a_growing_cooldown_and_success_resets_it() {
        let mut g = Gate::new(GateConfig::default(), 0);
        let p = pk(10);
        for _ in 0..3 {
            assert!(g.admit(&inc(p, QuotaKind::PairingAttempt), 0).is_ok());
            g.record_pairing_failure(p, 0);
        }
        let Err(Reject::PairingCooldown {
            retry_after_ms: first,
        }) = g.admit(&inc(p, QuotaKind::PairingAttempt), 1)
        else {
            panic!()
        };
        assert!(first > 20_000);
        // After the first cooldown, three more failures earn a longer one.
        let t = 1_000_000;
        for _ in 0..3 {
            let _ = g.admit(&inc(p, QuotaKind::PairingAttempt), t);
            g.record_pairing_failure(p, t);
        }
        let Err(Reject::PairingCooldown {
            retry_after_ms: second,
        }) = g.admit(&inc(p, QuotaKind::PairingAttempt), t + 1)
        else {
            panic!()
        };
        assert!(second > first, "{second} vs {first}");
        g.record_pairing_success(p);
        assert!(!matches!(
            g.admit(&inc(p, QuotaKind::PairingAttempt), t + 2),
            Err(Reject::PairingCooldown { .. })
        ));
    }

    #[test]
    fn the_blocklist_is_bounded() {
        let mut g = Gate::new(
            GateConfig {
                max_blocked: 10,
                ..GateConfig::default()
            },
            0,
        );
        let ok = (0..50).filter(|i| g.block(pk(*i))).count();
        assert_eq!(ok, 10);
        assert!(g.block(pk(0)), "re-blocking an existing entry is fine");
    }
}
