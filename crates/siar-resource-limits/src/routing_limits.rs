//! §68 routing backpressure, §70 discovery backpressure.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::connections::PeerKey;
use crate::permits::BoundedCounter;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutingLimits {
    pub concurrent_connects: u64,
    pub discovery_jobs: u64,
    pub retry_timers: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RoutingDenied {
    #[error("too many concurrent connection attempts")]
    ConnectsFull,
    #[error("a connection attempt to this target is already in flight")]
    AlreadyConnecting,
    #[error("too many active discovery jobs")]
    DiscoveryFull,
    #[error("too many pending retry timers")]
    TimersFull,
}

#[derive(Debug)]
struct Inner {
    connects: BoundedCounter,
    discovery: BoundedCounter,
    timers: BoundedCounter,
    in_flight_targets: Mutex<HashSet<PeerKey>>,
}

/// Routing must not turn one flaky peer into an unbounded storm of path
/// acquisition attempts. Besides the three global caps, at most one
/// connect per target is in flight — a second request for the same target
/// is refused rather than racing the first.
#[derive(Debug, Clone)]
pub struct RoutingBackpressure(Arc<Inner>);

#[derive(Debug)]
#[must_use = "dropping the permit ends the connect attempt's slot"]
pub struct ConnectPermit {
    inner: Arc<Inner>,
    target: PeerKey,
}

#[derive(Debug)]
#[must_use]
pub struct DiscoveryPermit(Arc<Inner>);

#[derive(Debug)]
#[must_use]
pub struct TimerPermit(Arc<Inner>);

impl RoutingBackpressure {
    pub fn new(l: RoutingLimits) -> Self {
        Self(Arc::new(Inner {
            connects: BoundedCounter::new(l.concurrent_connects),
            discovery: BoundedCounter::new(l.discovery_jobs),
            timers: BoundedCounter::new(l.retry_timers),
            in_flight_targets: Mutex::new(HashSet::new()),
        }))
    }

    pub fn try_connect(&self, target: PeerKey) -> Result<ConnectPermit, RoutingDenied> {
        let mut set = lock(&self.0.in_flight_targets);
        if set.contains(&target) {
            return Err(RoutingDenied::AlreadyConnecting);
        }
        if !self.0.connects.try_reserve(1) {
            return Err(RoutingDenied::ConnectsFull);
        }
        set.insert(target);
        drop(set);
        Ok(ConnectPermit {
            inner: Arc::clone(&self.0),
            target,
        })
    }

    pub fn try_discovery(&self) -> Result<DiscoveryPermit, RoutingDenied> {
        if self.0.discovery.try_reserve(1) {
            Ok(DiscoveryPermit(Arc::clone(&self.0)))
        } else {
            Err(RoutingDenied::DiscoveryFull)
        }
    }

    pub fn try_retry_timer(&self) -> Result<TimerPermit, RoutingDenied> {
        if self.0.timers.try_reserve(1) {
            Ok(TimerPermit(Arc::clone(&self.0)))
        } else {
            Err(RoutingDenied::TimersFull)
        }
    }

    pub fn connects_in_flight(&self) -> u64 {
        self.0.connects.in_use()
    }
}

impl Drop for ConnectPermit {
    fn drop(&mut self) {
        lock(&self.inner.in_flight_targets).remove(&self.target);
        self.inner.connects.release(1);
    }
}
impl Drop for DiscoveryPermit {
    fn drop(&mut self) {
        self.0.discovery.release(1);
    }
}
impl Drop for TimerPermit {
    fn drop(&mut self) {
        self.0.timers.release(1);
    }
}

// ------------------------------------------------------------ discovery

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// First sighting: a new peer to tell the world about.
    New,
    /// Advertised state differs from what we had.
    Changed,
    /// Same state; only the last-seen time moved. No notification.
    Refreshed,
    /// Table full and nothing stale enough to evict.
    Ignored,
}

#[derive(Debug)]
struct Seen {
    fingerprint: u64,
    last_seen: u64,
    dirty: bool,
}

/// §70: nearby-discovery results are noisy (a BLE peer re-advertises many
/// times a second). Instead of queueing each advertisement, keep one row
/// per peer, update it in place, and let the consumer drain the *set of
/// peers whose state actually changed* — at most one entry per peer per
/// drain, however many advertisements arrived.
#[derive(Debug)]
pub struct DiscoveryTable {
    peers: HashMap<PeerKey, Seen>,
    max_peers: usize,
}

impl DiscoveryTable {
    pub fn new(max_peers: usize) -> Self {
        Self {
            peers: HashMap::new(),
            max_peers: max_peers.max(1),
        }
    }

    pub fn observe(&mut self, peer: PeerKey, fingerprint: u64, now_millis: u64) -> Observation {
        if let Some(s) = self.peers.get_mut(&peer) {
            s.last_seen = now_millis.max(s.last_seen);
            if s.fingerprint != fingerprint {
                s.fingerprint = fingerprint;
                s.dirty = true;
                return Observation::Changed;
            }
            return Observation::Refreshed;
        }
        if self.peers.len() >= self.max_peers {
            // Evict the stalest *clean* row; never one with an undelivered
            // change, and never a row seen more recently than the newcomer.
            let victim = self
                .peers
                .iter()
                .filter(|(_, s)| !s.dirty && s.last_seen <= now_millis)
                .min_by_key(|(_, s)| s.last_seen)
                .map(|(k, _)| *k);
            match victim {
                Some(k) => {
                    self.peers.remove(&k);
                }
                None => return Observation::Ignored,
            }
        }
        self.peers.insert(
            peer,
            Seen {
                fingerprint,
                last_seen: now_millis,
                dirty: true,
            },
        );
        Observation::New
    }

    /// Peers with undelivered state changes (each at most once).
    pub fn drain_changed(&mut self) -> Vec<PeerKey> {
        let mut out: Vec<PeerKey> = self
            .peers
            .iter_mut()
            .filter(|(_, s)| s.dirty)
            .map(|(k, s)| {
                s.dirty = false;
                *k
            })
            .collect();
        out.sort_unstable(); // deterministic for the consumer
        out
    }

    /// Drop peers not heard from within `ttl`. Returns the departed.
    pub fn expire(&mut self, now_millis: u64, ttl_millis: u64) -> Vec<PeerKey> {
        let gone: Vec<PeerKey> = self
            .peers
            .iter()
            .filter(|(_, s)| now_millis.saturating_sub(s.last_seen) > ttl_millis)
            .map(|(k, _)| *k)
            .collect();
        for k in &gone {
            self.peers.remove(k);
        }
        gone
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PeerKey = [1; 32];
    const B: PeerKey = [2; 32];

    fn rb() -> RoutingBackpressure {
        RoutingBackpressure::new(RoutingLimits {
            concurrent_connects: 2,
            discovery_jobs: 1,
            retry_timers: 1,
        })
    }

    #[test]
    fn one_connect_per_target_and_a_global_cap() {
        let r = rb();
        let a = r.try_connect(A).unwrap();
        assert_eq!(
            r.try_connect(A).unwrap_err(),
            RoutingDenied::AlreadyConnecting
        );
        let _b = r.try_connect(B).unwrap();
        assert_eq!(
            r.try_connect([3; 32]).unwrap_err(),
            RoutingDenied::ConnectsFull
        );
        drop(a);
        let _a2 = r.try_connect(A).expect("target and slot both freed");
        assert_eq!(r.connects_in_flight(), 2);
    }

    #[test]
    fn a_refused_connect_leaves_no_target_marker_behind() {
        let r = RoutingBackpressure::new(RoutingLimits {
            concurrent_connects: 0,
            discovery_jobs: 0,
            retry_timers: 0,
        });
        assert_eq!(r.try_connect(A).unwrap_err(), RoutingDenied::ConnectsFull);
        assert!(lock(&r.0.in_flight_targets).is_empty());
    }

    #[test]
    fn discovery_jobs_and_timers_are_capped_and_released() {
        let r = rb();
        let d = r.try_discovery().unwrap();
        assert_eq!(r.try_discovery().unwrap_err(), RoutingDenied::DiscoveryFull);
        drop(d);
        assert!(r.try_discovery().is_ok());
        let t = r.try_retry_timer().unwrap();
        assert_eq!(r.try_retry_timer().unwrap_err(), RoutingDenied::TimersFull);
        drop(t);
        assert!(r.try_retry_timer().is_ok());
    }

    #[test]
    fn a_noisy_peer_is_one_row_and_one_change_per_drain() {
        let mut t = DiscoveryTable::new(8);
        assert_eq!(t.observe(A, 1, 0), Observation::New);
        for i in 1..1_000 {
            assert_eq!(t.observe(A, 1, i), Observation::Refreshed);
        }
        assert_eq!(t.observe(A, 2, 1_000), Observation::Changed);
        assert_eq!(t.observe(A, 3, 1_001), Observation::Changed);
        assert_eq!(t.drain_changed(), vec![A], "coalesced to one entry");
        assert!(t.drain_changed().is_empty());
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn table_is_bounded_and_does_not_evict_undelivered_changes() {
        let mut t = DiscoveryTable::new(2);
        t.observe(A, 1, 0);
        t.observe(B, 1, 1);
        // Both dirty (undelivered): a third peer cannot displace either.
        assert_eq!(t.observe([3; 32], 1, 2), Observation::Ignored);
        t.drain_changed();
        assert_eq!(t.observe([3; 32], 1, 3), Observation::New);
        assert_eq!(t.len(), 2);
        assert!(t.drain_changed().contains(&[3; 32]));
    }

    #[test]
    fn stale_peers_expire() {
        let mut t = DiscoveryTable::new(8);
        t.observe(A, 1, 0);
        t.observe(B, 1, 900);
        assert_eq!(t.expire(1_000, 500), vec![A]);
        assert_eq!(t.len(), 1);
    }
}
