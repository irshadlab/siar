//! §42 connection limits, §43 file-descriptor limits.
//!
//! Both are RAII: a permit returns its capacity on `Drop`, so a dropped
//! future or an unwinding panic cannot leak a slot. The per-peer map only
//! holds peers with at least one live permit and is therefore bounded by
//! the global cap — an attacker cycling peer identities cannot grow it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::permits::BoundedCounter;

pub type PeerKey = [u8; 32];

#[derive(Debug)]
struct State {
    total: u32,
    per_peer: HashMap<PeerKey, u32>,
}

#[derive(Debug)]
pub(crate) struct PeerLimiterInner {
    max_total: u32,
    max_per_peer: u32,
    state: Mutex<State>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitDenied {
    #[error("global limit reached")]
    Global,
    #[error("per-peer limit reached")]
    PerPeer,
}

/// Shared by [`ConnectionLimiter`] and [`crate::slots::TransferSlots`].
#[derive(Debug, Clone)]
pub(crate) struct PeerLimiter {
    inner: Arc<PeerLimiterInner>,
}

#[derive(Debug)]
pub(crate) struct PeerHold {
    inner: Arc<PeerLimiterInner>,
    key: PeerKey,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    // A panic while holding this lock leaves only plain counters behind;
    // continuing with them is safe and better than cascading the panic.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl PeerLimiter {
    pub(crate) fn new(max_total: u32, max_per_peer: u32) -> Self {
        Self {
            inner: Arc::new(PeerLimiterInner {
                max_total,
                max_per_peer,
                state: Mutex::new(State {
                    total: 0,
                    per_peer: HashMap::new(),
                }),
            }),
        }
    }

    /// `total_cap` may be lower than the configured maximum (used to keep
    /// headroom for higher classes); it can never be higher.
    pub(crate) fn try_acquire(
        &self,
        key: PeerKey,
        total_cap: u32,
    ) -> Result<PeerHold, LimitDenied> {
        let cap = total_cap.min(self.inner.max_total);
        let mut guard = lock(&self.inner.state);
        let st = &mut *guard;
        if st.total >= cap {
            return Err(LimitDenied::Global);
        }
        let n = st.per_peer.entry(key).or_insert(0);
        if *n >= self.inner.max_per_peer {
            // Only reachable with an empty entry when max_per_peer == 0.
            if *n == 0 {
                st.per_peer.remove(&key);
            }
            return Err(LimitDenied::PerPeer);
        }
        *n += 1;
        st.total += 1;
        drop(guard);
        Ok(PeerHold {
            inner: Arc::clone(&self.inner),
            key,
        })
    }

    pub(crate) fn active(&self) -> u32 {
        lock(&self.inner.state).total
    }

    pub(crate) fn active_for(&self, key: &PeerKey) -> u32 {
        lock(&self.inner.state)
            .per_peer
            .get(key)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn max_total(&self) -> u32 {
        self.inner.max_total
    }
}

impl Drop for PeerHold {
    fn drop(&mut self) {
        let mut st = lock(&self.inner.state);
        st.total = st.total.saturating_sub(1);
        if let Some(n) = st.per_peer.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                st.per_peer.remove(&self.key);
            }
        }
    }
}

/// §42: "max active peer sessions" globally, "max parallel connections"
/// per peer. (Transports should reuse sessions; the per-peer cap is the
/// backstop when they don't.)
#[derive(Debug, Clone)]
pub struct ConnectionLimiter(PeerLimiter);

#[derive(Debug)]
#[must_use = "dropping the permit closes the slot"]
pub struct ConnectionPermit(#[allow(dead_code)] PeerHold);

impl ConnectionLimiter {
    pub fn new(max_sessions: u32, max_per_peer: u32) -> Self {
        Self(PeerLimiter::new(max_sessions, max_per_peer))
    }

    pub fn try_acquire(&self, peer: PeerKey) -> Result<ConnectionPermit, LimitDenied> {
        self.0
            .try_acquire(peer, self.0.max_total())
            .map(ConnectionPermit)
    }

    pub fn active(&self) -> u32 {
        self.0.active()
    }

    pub fn active_for(&self, peer: &PeerKey) -> u32 {
        self.0.active_for(peer)
    }
}

/// §43: the four FD consumers the spec names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdClass {
    BlobReader,
    StagingFile,
    Socket,
    Watcher,
}

impl FdClass {
    const COUNT: usize = 4;
    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug)]
struct FdInner {
    total: BoundedCounter,
    classes: [BoundedCounter; FdClass::COUNT],
}

/// Per-class caps *and* a total ceiling under the OS limit, so neither
/// a pile of blob readers nor a socket flood can exhaust descriptors the
/// other classes need.
#[derive(Debug, Clone)]
pub struct FdLimiter(Arc<FdInner>);

#[derive(Debug)]
#[must_use = "dropping the permit closes the descriptor budget"]
pub struct FdPermit {
    inner: Arc<FdInner>,
    class: FdClass,
}

impl FdLimiter {
    /// `per_class` is indexed in [`FdClass`] declaration order.
    pub fn new(total: u64, per_class: [u64; 4]) -> Self {
        Self(Arc::new(FdInner {
            total: BoundedCounter::new(total),
            classes: per_class.map(BoundedCounter::new),
        }))
    }

    pub fn try_acquire(&self, class: FdClass) -> Option<FdPermit> {
        let c = &self.0.classes[class.index()];
        if !c.try_reserve(1) {
            return None;
        }
        if !self.0.total.try_reserve(1) {
            c.release(1);
            return None;
        }
        Some(FdPermit {
            inner: Arc::clone(&self.0),
            class,
        })
    }

    pub fn in_use(&self, class: FdClass) -> u64 {
        self.0.classes[class.index()].in_use()
    }

    pub fn total_in_use(&self) -> u64 {
        self.0.total.in_use()
    }
}

impl Drop for FdPermit {
    fn drop(&mut self) {
        self.inner.classes[self.class.index()].release(1);
        self.inner.total.release(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PeerKey = [1; 32];
    const B: PeerKey = [2; 32];

    #[test]
    fn global_and_per_peer_caps_both_apply() {
        let l = ConnectionLimiter::new(3, 2);
        let _a1 = l.try_acquire(A).unwrap();
        let _a2 = l.try_acquire(A).unwrap();
        assert_eq!(l.try_acquire(A).unwrap_err(), LimitDenied::PerPeer);
        let _b1 = l.try_acquire(B).unwrap();
        assert_eq!(l.try_acquire(B).unwrap_err(), LimitDenied::Global);
        assert_eq!(l.active(), 3);
    }

    #[test]
    fn dropping_a_permit_frees_the_slot_and_forgets_the_peer() {
        let l = ConnectionLimiter::new(2, 2);
        let p = l.try_acquire(A).unwrap();
        assert_eq!(l.active_for(&A), 1);
        drop(p);
        assert_eq!((l.active(), l.active_for(&A)), (0, 0));
        assert!(l.0.inner.state.lock().unwrap().per_peer.is_empty());
    }

    #[test]
    fn identity_churn_cannot_grow_the_map_past_the_global_cap() {
        let l = ConnectionLimiter::new(4, 1);
        let mut held = Vec::new();
        for i in 0..1_000u32 {
            let mut k = [0u8; 32];
            k[..4].copy_from_slice(&i.to_le_bytes());
            if let Ok(p) = l.try_acquire(k) {
                held.push(p);
            }
        }
        assert_eq!(held.len(), 4);
        assert!(l.0.inner.state.lock().unwrap().per_peer.len() <= 4);
    }

    #[test]
    fn panic_while_holding_a_permit_releases_it() {
        let l = ConnectionLimiter::new(1, 1);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _p = l.try_acquire(A).unwrap();
            panic!("boom");
        }));
        assert!(r.is_err());
        assert!(l.try_acquire(A).is_ok());
    }

    #[test]
    fn fd_classes_are_isolated_and_total_is_enforced() {
        let l = FdLimiter::new(5, [2, 2, 4, 1]);
        let _r1 = l.try_acquire(FdClass::BlobReader).unwrap();
        let _r2 = l.try_acquire(FdClass::BlobReader).unwrap();
        assert!(l.try_acquire(FdClass::BlobReader).is_none(), "class cap");
        let _w = l.try_acquire(FdClass::Watcher).unwrap();
        let _s1 = l.try_acquire(FdClass::Socket).unwrap();
        let _s2 = l.try_acquire(FdClass::Socket).unwrap();
        assert!(l.try_acquire(FdClass::Socket).is_none(), "total cap");
        assert_eq!(
            l.in_use(FdClass::Socket),
            2,
            "failed total reserve rolled the class back"
        );
        assert_eq!(l.total_in_use(), 5);
    }
}
