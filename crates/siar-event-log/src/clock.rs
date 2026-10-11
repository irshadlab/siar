//! `04-offline-event-log-architecture.md` §27 "Logical Clocks".
//!
//! "Wall clocks are useful for display, not universal ordering.
//! Use: stream version, generation, domain sequence, logical clock where
//! necessary. A hybrid logical clock can be added later if cross-device
//! approximate chronology becomes valuable."
//!
//! Within one stream, `stream_version` already gives a total order, and
//! every cursor, gap check and reassembly in this crate relies on it —
//! none of that changes. A hybrid logical clock (HLC) answers a
//! different question: **across** streams and devices, which of two
//! independently-made changes should a *domain rule* treat as later?
//! (§54's "domain conflict rules"; §88's two offline devices that "later
//! converge" with "no central clock required".) [`HybridClock`] is that
//! tool, available to any domain that needs a last-writer-wins style
//! rule; it is **not** stored in [`crate::envelope::EventEnvelope`], so
//! it changes no existing wire or storage format. A domain that wants it
//! puts an [`HlcTimestamp`] in its own event payload.
//!
//! ## Properties
//!
//! An [`HlcTimestamp`] is `(wall_ms, counter, node)`, ordered
//! lexicographically — a *total* order, because `node` (the device id's
//! bytes) breaks any remaining tie, so two devices never produce equal
//! timestamps.
//!
//! - **Monotonic per device**, even if the OS clock steps backwards:
//!   [`HybridClock::now`] never returns anything `<=` a previous result.
//! - **Causal**: after [`HybridClock::observe`]-ing a remote timestamp,
//!   every later local timestamp is greater than it. If event B was
//!   created after seeing A, `hlc(B) > hlc(A)`.
//! - **Close to real time**: `wall_ms` tracks the physical clock rather
//!   than drifting into an unbounded counter, so it is still usable as an
//!   approximate "when" for display (the wall-clock use §27 allows).
//!
//! ## A hostile or broken peer cannot drag the clock into the future
//!
//! HLCs have a known failure mode: one device with its clock set to the
//! year 2099 sends a timestamp, every honest device `observe`s it, and
//! from then on *all* of them stamp events in 2099 — and "last write
//! wins" is permanently decided in the attacker's favour. So
//! [`HybridClock::observe`] **rejects** a remote timestamp more than
//! `max_drift_ms` ahead of the local physical clock
//! ([`ClockError::DriftTooLarge`]); the event itself can still be
//! stored and replicated (§24/§53 don't care), it just doesn't move this
//! clock. The same check bounds the counter ([`ClockError::
//! CounterOverflow`]) rather than letting it wrap and break monotonicity.
//!
//! Physical time is an *argument*, never read here, so every behaviour
//! below is deterministic under test.

use std::sync::Mutex;

use siar_domain::DeviceId;
use thiserror::Error;

/// `(wall_ms, counter, node)` — see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HlcTimestamp {
    pub wall_ms: u64,
    pub counter: u32,
    pub node: [u8; 16],
}

/// Encoded length of an [`HlcTimestamp`].
pub const HLC_ENCODED_LEN: usize = 8 + 4 + 16;

impl HlcTimestamp {
    pub const ZERO: Self = Self {
        wall_ms: 0,
        counter: 0,
        node: [0; 16],
    };

    /// Big-endian, so the **byte** order equals the logical order: the
    /// encoding can be compared, sorted or used as an index key without
    /// decoding.
    pub fn to_bytes(&self) -> [u8; HLC_ENCODED_LEN] {
        let mut out = [0u8; HLC_ENCODED_LEN];
        out[..8].copy_from_slice(&self.wall_ms.to_be_bytes());
        out[8..12].copy_from_slice(&self.counter.to_be_bytes());
        out[12..].copy_from_slice(&self.node);
        out
    }

    /// `None` unless `bytes` is exactly [`HLC_ENCODED_LEN`] long.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != HLC_ENCODED_LEN {
            return None;
        }
        Some(Self {
            wall_ms: u64::from_be_bytes(bytes[..8].try_into().ok()?),
            counter: u32::from_be_bytes(bytes[8..12].try_into().ok()?),
            node: bytes[12..].try_into().ok()?,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClockError {
    #[error("remote timestamp is {ahead_ms} ms ahead of the local clock (limit {limit_ms} ms); not adopted")]
    DriftTooLarge { ahead_ms: u64, limit_ms: u64 },
    #[error("logical counter exhausted within one millisecond")]
    CounterOverflow,
}

/// Default tolerance for a remote clock running ahead: one minute is
/// far beyond honest skew between devices that sync, and tiny next to
/// "the year 2099".
pub const DEFAULT_MAX_DRIFT_MS: u64 = 60_000;

#[derive(Debug)]
pub struct HybridClock {
    node: [u8; 16],
    max_drift_ms: u64,
    last: Mutex<HlcTimestamp>,
}

impl HybridClock {
    pub fn new(device: DeviceId) -> Self {
        Self::with_max_drift(device, DEFAULT_MAX_DRIFT_MS)
    }

    pub fn with_max_drift(device: DeviceId, max_drift_ms: u64) -> Self {
        let node = *device.as_uuid().as_bytes();
        Self {
            node,
            max_drift_ms,
            last: Mutex::new(HlcTimestamp {
                wall_ms: 0,
                counter: 0,
                node,
            }),
        }
    }

    /// A fresh timestamp for a local event.
    pub fn now(&self, physical_ms: u64) -> Result<HlcTimestamp, ClockError> {
        let mut last = self.last.lock().expect("hlc lock poisoned");
        let wall = last.wall_ms.max(physical_ms);
        let counter = if wall == last.wall_ms {
            last.counter
                .checked_add(1)
                .ok_or(ClockError::CounterOverflow)?
        } else {
            0
        };
        *last = HlcTimestamp {
            wall_ms: wall,
            counter,
            node: self.node,
        };
        Ok(*last)
    }

    /// Folds in a timestamp received from a peer and returns a fresh
    /// local timestamp that is greater than both it and every previous
    /// local one.
    pub fn observe(
        &self,
        remote: &HlcTimestamp,
        physical_ms: u64,
    ) -> Result<HlcTimestamp, ClockError> {
        let ahead = remote.wall_ms.saturating_sub(physical_ms);
        if ahead > self.max_drift_ms {
            return Err(ClockError::DriftTooLarge {
                ahead_ms: ahead,
                limit_ms: self.max_drift_ms,
            });
        }
        let mut last = self.last.lock().expect("hlc lock poisoned");
        let wall = last.wall_ms.max(remote.wall_ms).max(physical_ms);
        let counter = if wall == last.wall_ms && wall == remote.wall_ms {
            last.counter.max(remote.counter)
        } else if wall == last.wall_ms {
            last.counter
        } else if wall == remote.wall_ms {
            remote.counter
        } else {
            // Physical time moved past both: restart the counter, and
            // return directly (no increment needed — wall is new).
            *last = HlcTimestamp {
                wall_ms: wall,
                counter: 0,
                node: self.node,
            };
            return Ok(*last);
        }
        .checked_add(1)
        .ok_or(ClockError::CounterOverflow)?;
        *last = HlcTimestamp {
            wall_ms: wall,
            counter,
            node: self.node,
        };
        Ok(*last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn clock() -> HybridClock {
        HybridClock::new(DeviceId::new())
    }

    #[test]
    fn timestamps_are_strictly_monotonic_even_when_physical_time_stalls_or_reverses() {
        let c = clock();
        let t1 = c.now(1_000).unwrap();
        let t2 = c.now(1_000).unwrap(); // same millisecond
        let t3 = c.now(500).unwrap(); // clock stepped backwards
        let t4 = c.now(2_000).unwrap();
        assert!(t1 < t2 && t2 < t3 && t3 < t4);
        assert_eq!((t2.wall_ms, t2.counter), (1_000, 1));
        assert_eq!(
            (t3.wall_ms, t3.counter),
            (1_000, 2),
            "wall never goes backwards"
        );
        assert_eq!(
            (t4.wall_ms, t4.counter),
            (2_000, 0),
            "counter resets on new ms"
        );
    }

    #[test]
    fn observing_a_remote_makes_every_later_local_timestamp_greater() {
        let (a, b) = (clock(), clock());
        let from_a = a.now(5_000).unwrap();
        // B's physical clock is *behind* A's by 3 s.
        let b_after = b.observe(&from_a, 2_000).unwrap();
        assert!(
            b_after > from_a,
            "causality: B saw A, so B's event is later"
        );
        assert!(b.now(2_001).unwrap() > b_after);
    }

    #[test]
    fn two_devices_never_produce_equal_timestamps() {
        let (a, b) = (clock(), clock());
        let ta = a.now(1_000).unwrap();
        let tb = b.now(1_000).unwrap();
        assert_ne!(ta, tb);
        assert_eq!((ta.wall_ms, ta.counter), (tb.wall_ms, tb.counter));
        assert_ne!(
            ta.cmp(&tb),
            std::cmp::Ordering::Equal,
            "node breaks the tie"
        );
    }

    #[test]
    fn a_far_future_remote_is_rejected_and_does_not_poison_the_clock() {
        let c = clock();
        let honest = c.now(10_000).unwrap();
        let hostile = HlcTimestamp {
            wall_ms: 10_000 + 365 * 24 * 3_600_000, // a year ahead
            counter: 0,
            node: [9; 16],
        };
        assert!(matches!(
            c.observe(&hostile, 10_001),
            Err(ClockError::DriftTooLarge {
                limit_ms: DEFAULT_MAX_DRIFT_MS,
                ..
            })
        ));
        // The clock carries on in real time, unaffected.
        let after = c.now(10_002).unwrap();
        assert!(after > honest);
        assert_eq!(after.wall_ms, 10_002);

        // Honest skew within tolerance is accepted.
        let skewed = HlcTimestamp {
            wall_ms: 10_002 + 30_000,
            counter: 0,
            node: [9; 16],
        };
        assert!(c.observe(&skewed, 10_003).is_ok());
    }

    #[test]
    fn counter_overflow_is_an_error_not_a_wraparound() {
        let c = clock();
        *c.last.lock().unwrap() = HlcTimestamp {
            wall_ms: 1_000,
            counter: u32::MAX,
            node: c.node,
        };
        assert_eq!(c.now(1_000), Err(ClockError::CounterOverflow));
        // A newer millisecond recovers.
        assert!(c.now(1_001).is_ok());
    }

    #[test]
    fn byte_encoding_round_trips_and_sorts_like_the_timestamps() {
        let c = clock();
        let ts: Vec<_> = [100, 100, 300, 250, 900]
            .iter()
            .map(|p| c.now(*p).unwrap())
            .collect();
        for t in &ts {
            assert_eq!(HlcTimestamp::from_bytes(&t.to_bytes()), Some(*t));
        }
        let mut by_value = ts.clone();
        by_value.sort();
        let mut by_bytes = ts.clone();
        by_bytes.sort_by_key(|t| t.to_bytes());
        assert_eq!(by_value, by_bytes, "memcmp order == logical order");
        assert_eq!(HlcTimestamp::from_bytes(&[0; 27]), None);
        assert_eq!(HlcTimestamp::from_bytes(&[0; 29]), None);
    }

    proptest! {
        /// Under ANY interleaving of local events and remote
        /// observations with arbitrary (even backwards) physical time,
        /// a device's own timestamps strictly increase, and each
        /// observe result exceeds the remote it observed.
        #[test]
        fn monotonic_and_causal_under_arbitrary_schedules(
            steps in proptest::collection::vec((any::<bool>(), 0u64..5_000, 0u64..5_000, 0u32..8), 1..80),
        ) {
            let c = HybridClock::with_max_drift(DeviceId::new(), 10_000);
            let mut prev = HlcTimestamp::ZERO;
            for (is_observe, physical, remote_wall, remote_counter) in steps {
                let out = if is_observe {
                    let remote = HlcTimestamp { wall_ms: remote_wall, counter: remote_counter, node: [3; 16] };
                    match c.observe(&remote, physical) {
                        Ok(t) => { prop_assert!(t > remote); t }
                        Err(_) => continue,
                    }
                } else {
                    c.now(physical).unwrap()
                };
                prop_assert!(out > prev);
                prev = out;
            }
        }
    }
}
