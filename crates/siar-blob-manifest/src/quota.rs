//! §47 "Quotas" and §48 "Storage Reservation".
//!
//! §47's list — max file size, max concurrent transfers, max per-peer
//! bytes, max daily received bytes, max relay storage, max cache size —
//! splits cleanly along *who knows the answer*:
//!
//! - **The store knows** how many bytes it holds, how many it has
//!   promised to in-flight writes, how many writes are running, and how
//!   big one blob may be. Those are [`StoreLimits`], enforced here.
//! - **Policy knows** who the peer is and what the day's budget is:
//!   per-peer bytes, daily bytes and relay/cache classification live in
//!   [`crate::resource_policy`], [`crate::auto_accept`] and
//!   [`crate::storage_lifecycle`], which decide *whether to start* a
//!   transfer. By the time the store is asked to begin a write that
//!   question is already answered; the store's job is to make the
//!   answer *stay true* while the bytes arrive.
//!
//! ## §48: promise the space before accepting the bytes
//!
//! "Avoid accepting 20 GB then failing at 99%." [`QuotaLedger::reserve`]
//! is that promise: it earmarks the *declared* size against
//! `max_total_bytes` before the first chunk is written, so a transfer
//! that cannot fit is refused at the start
//! ([`BlobError::ReservationDenied`]) rather than discovered full at the
//! end. Concurrent writes cannot jointly over-promise: `used + reserved`
//! is checked and updated under one lock.
//!
//! A [`Reservation`] is a guard. Its three exits:
//!
//! - [`Reservation::commit`] — the blob was finalized; the bytes move
//!   from "reserved" to "used";
//! - drop — the write was abandoned or failed; the earmark is released
//!   and nothing is counted as used;
//! - either way the concurrent-write slot is freed.
//!
//! Because release happens in `Drop`, a panic or early return cannot
//! leak reserved space.

use std::sync::{Arc, Mutex};

use crate::store_error::BlobError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreLimits {
    /// Everything the store may hold, finished blobs plus reservations.
    pub max_total_bytes: u64,
    /// Largest single blob.
    pub max_blob_bytes: u64,
    pub max_concurrent_writes: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 20 * 1024 * 1024 * 1024,
            max_blob_bytes: 10 * 1024 * 1024 * 1024,
            max_concurrent_writes: 8,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    used: u64,
    reserved: u64,
    active_writes: usize,
}

#[derive(Debug)]
pub struct QuotaLedger {
    limits: StoreLimits,
    state: Mutex<State>,
}

impl QuotaLedger {
    /// `already_used`: bytes of finished blobs found on disk at open.
    pub fn new(limits: StoreLimits, already_used: u64) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::new(State {
                used: already_used,
                reserved: 0,
                active_writes: 0,
            }),
        })
    }

    pub fn limits(&self) -> StoreLimits {
        self.limits
    }

    pub fn used_bytes(&self) -> u64 {
        self.state.lock().expect("ledger poisoned").used
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.state.lock().expect("ledger poisoned").reserved
    }

    pub fn active_writes(&self) -> usize {
        self.state.lock().expect("ledger poisoned").active_writes
    }

    /// Bytes neither used nor promised.
    pub fn free_bytes(&self) -> u64 {
        let s = self.state.lock().expect("ledger poisoned");
        self.limits
            .max_total_bytes
            .saturating_sub(s.used.saturating_add(s.reserved))
    }

    /// Earmarks `bytes` for one new write. See the module doc.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Result<Reservation, BlobError> {
        if bytes > self.limits.max_blob_bytes {
            return Err(BlobError::BlobTooLarge {
                size: bytes,
                limit: self.limits.max_blob_bytes,
            });
        }
        let mut s = self.state.lock().expect("ledger poisoned");
        if s.active_writes >= self.limits.max_concurrent_writes {
            return Err(BlobError::TooManyConcurrentWrites {
                limit: self.limits.max_concurrent_writes,
            });
        }
        let committed = s.used.saturating_add(s.reserved);
        let available = self.limits.max_total_bytes.saturating_sub(committed);
        if bytes > available {
            return Err(BlobError::ReservationDenied {
                requested: bytes,
                available,
            });
        }
        s.reserved += bytes;
        s.active_writes += 1;
        Ok(Reservation {
            ledger: Arc::clone(self),
            bytes,
            settled: false,
        })
    }

    /// A finished blob was deleted: give its bytes back.
    pub fn release_used(&self, bytes: u64) {
        let mut s = self.state.lock().expect("ledger poisoned");
        s.used = s.used.saturating_sub(bytes);
    }
}

#[derive(Debug)]
pub struct Reservation {
    ledger: Arc<QuotaLedger>,
    bytes: u64,
    settled: bool,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// The write finished: reserved bytes become used bytes.
    pub fn commit(mut self) {
        let mut s = self.ledger.state.lock().expect("ledger poisoned");
        s.reserved -= self.bytes;
        s.used += self.bytes;
        s.active_writes -= 1;
        self.settled = true;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.settled {
            let mut s = self.ledger.state.lock().expect("ledger poisoned");
            s.reserved -= self.bytes;
            s.active_writes -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn limits(total: u64, blob: u64, writes: usize) -> StoreLimits {
        StoreLimits {
            max_total_bytes: total,
            max_blob_bytes: blob,
            max_concurrent_writes: writes,
        }
    }

    #[test]
    fn a_transfer_that_cannot_fit_is_refused_up_front_not_at_99_percent() {
        let l = QuotaLedger::new(limits(1000, 1000, 4), 0);
        let _a = l.reserve(700).unwrap();
        assert_eq!(
            l.reserve(400).unwrap_err(),
            BlobError::ReservationDenied {
                requested: 400,
                available: 300
            }
        );
        assert!(l.reserve(300).is_ok());
    }

    #[test]
    fn existing_usage_counts_against_the_total() {
        let l = QuotaLedger::new(limits(1000, 1000, 4), 900);
        assert_eq!(l.free_bytes(), 100);
        assert!(l.reserve(101).is_err());
        assert!(l.reserve(100).is_ok());
    }

    #[test]
    fn dropping_a_reservation_releases_it_and_commit_converts_it() {
        let l = QuotaLedger::new(limits(1000, 1000, 4), 0);
        {
            let _r = l.reserve(600).unwrap();
            assert_eq!(
                (l.reserved_bytes(), l.used_bytes(), l.active_writes()),
                (600, 0, 1)
            );
        }
        assert_eq!(
            (l.reserved_bytes(), l.used_bytes(), l.active_writes()),
            (0, 0, 0),
            "abandoned write frees its promise"
        );

        let r = l.reserve(600).unwrap();
        r.commit();
        assert_eq!(
            (l.reserved_bytes(), l.used_bytes(), l.active_writes()),
            (0, 600, 0)
        );
        l.release_used(600);
        assert_eq!(l.used_bytes(), 0);
        l.release_used(10_000); // over-release saturates, never underflows
        assert_eq!(l.used_bytes(), 0);
    }

    #[test]
    fn per_blob_and_concurrency_limits() {
        let l = QuotaLedger::new(limits(10_000, 500, 2), 0);
        assert_eq!(
            l.reserve(501).unwrap_err(),
            BlobError::BlobTooLarge {
                size: 501,
                limit: 500
            }
        );
        let _a = l.reserve(10).unwrap();
        let b = l.reserve(10).unwrap();
        assert_eq!(
            l.reserve(10).unwrap_err(),
            BlobError::TooManyConcurrentWrites { limit: 2 }
        );
        drop(b);
        assert!(l.reserve(10).is_ok(), "the slot came back");
    }

    #[test]
    fn concurrent_reservers_cannot_jointly_over_promise() {
        let l = QuotaLedger::new(limits(1000, 1000, 64), 0);
        let granted: Vec<_> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..32)
                .map(|_| {
                    let l = Arc::clone(&l);
                    s.spawn(move || l.reserve(100).ok())
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let ok = granted.iter().flatten().count();
        assert_eq!(ok, 10, "exactly total/100 reservations succeed, never more");
        assert!(l.reserved_bytes() <= 1000);
    }

    proptest! {
        /// Under any sequence of reserve / commit / drop, the books
        /// never exceed the limit and never go negative.
        #[test]
        fn accounting_invariants_hold_for_any_operation_sequence(
            ops in proptest::collection::vec((0u8..3, 1u64..400), 1..60),
        ) {
            let l = QuotaLedger::new(limits(1000, 1000, 8), 0);
            let mut held: Vec<Reservation> = Vec::new();
            for (op, n) in ops {
                match op {
                    0 => { if let Ok(r) = l.reserve(n) { held.push(r); } }
                    1 => { if let Some(r) = held.pop() { r.commit(); } }
                    _ => { held.pop(); }
                }
                prop_assert!(l.used_bytes() + l.reserved_bytes() <= 1000);
                prop_assert_eq!(l.active_writes(), held.len());
                prop_assert_eq!(l.reserved_bytes(), held.iter().map(|r| r.bytes()).sum::<u64>());
            }
        }
    }
}
