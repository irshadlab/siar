//! §64 file backpressure, §65 transfer slots.

use crate::connections::{LimitDenied, PeerHold, PeerKey, PeerLimiter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    /// User is waiting on it (a photo they just sent).
    Interactive,
    /// Background / bulk; may wait durably.
    Background,
}

/// §65: global and per-peer active-transfer caps. `interactive_reserve`
/// slots are never given to background transfers, so a pile of bulk
/// downloads cannot block a new interactive send. A denial is not a
/// failure: the caller puts the transfer in durable deferred state.
#[derive(Debug, Clone)]
pub struct TransferSlots {
    limiter: PeerLimiter,
    interactive_reserve: u32,
}

#[derive(Debug)]
#[must_use = "dropping the permit frees the transfer slot"]
pub struct SlotPermit(#[allow(dead_code)] PeerHold);

impl TransferSlots {
    pub fn new(max_active: u32, max_per_peer: u32, interactive_reserve: u32) -> Self {
        Self {
            limiter: PeerLimiter::new(max_active, max_per_peer),
            interactive_reserve: interactive_reserve.min(max_active),
        }
    }

    pub fn try_acquire(
        &self,
        peer: PeerKey,
        kind: TransferKind,
    ) -> Result<SlotPermit, LimitDenied> {
        let cap = match kind {
            TransferKind::Interactive => self.limiter.max_total(),
            TransferKind::Background => self.limiter.max_total() - self.interactive_reserve,
        };
        self.limiter.try_acquire(peer, cap).map(SlotPermit)
    }

    pub fn active(&self) -> u32 {
        self.limiter.active()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallReason {
    NetworkQueueFull,
    EncryptorBusy,
    MemoryPoolExhausted,
}

/// §64: the reader pauses while any downstream stage is saturated.
pub fn read_gate(
    network_queue_full: bool,
    encryptor_busy: bool,
    memory_available: bool,
) -> Result<(), StallReason> {
    if network_queue_full {
        Err(StallReason::NetworkQueueFull)
    } else if encryptor_busy {
        Err(StallReason::EncryptorBusy)
    } else if !memory_available {
        Err(StallReason::MemoryPoolExhausted)
    } else {
        Ok(())
    }
}

/// Bounded read-ahead: bytes read but not yet flushed to the network.
/// "Do not read ahead unboundedly."
#[derive(Debug)]
pub struct ReadAheadWindow {
    max_bytes: u64,
    in_flight: u64,
}

impl ReadAheadWindow {
    pub fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            in_flight: 0,
        }
    }

    /// Reserve `chunk` bytes of read-ahead; `false` means stop reading.
    pub fn try_read(&mut self, chunk: u64) -> bool {
        match self.in_flight.checked_add(chunk) {
            Some(n) if n <= self.max_bytes => {
                self.in_flight = n;
                true
            }
            _ => false,
        }
    }

    /// `chunk` bytes left the pipeline (sent/acked).
    pub fn on_flushed(&mut self, chunk: u64) {
        self.in_flight = self.in_flight.saturating_sub(chunk);
    }

    pub fn in_flight(&self) -> u64 {
        self.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PeerKey = [9; 32];
    const Q: PeerKey = [8; 32];

    #[test]
    fn background_cannot_take_the_interactive_reserve() {
        let s = TransferSlots::new(4, 4, 1);
        let _a = s.try_acquire(P, TransferKind::Background).unwrap();
        let _b = s.try_acquire(P, TransferKind::Background).unwrap();
        let _c = s.try_acquire(Q, TransferKind::Background).unwrap();
        assert_eq!(
            s.try_acquire(Q, TransferKind::Background).unwrap_err(),
            LimitDenied::Global
        );
        assert!(
            s.try_acquire(Q, TransferKind::Interactive).is_ok(),
            "reserve still available"
        );
    }

    #[test]
    fn per_peer_cap_defers_extra_transfers() {
        let s = TransferSlots::new(10, 2, 0);
        let _a = s.try_acquire(P, TransferKind::Background).unwrap();
        let _b = s.try_acquire(P, TransferKind::Background).unwrap();
        assert_eq!(
            s.try_acquire(P, TransferKind::Interactive).unwrap_err(),
            LimitDenied::PerPeer
        );
    }

    #[test]
    fn reserve_larger_than_capacity_is_clamped_not_underflowed() {
        let s = TransferSlots::new(2, 2, 99);
        assert_eq!(
            s.try_acquire(P, TransferKind::Background).unwrap_err(),
            LimitDenied::Global
        );
        assert!(s.try_acquire(P, TransferKind::Interactive).is_ok());
    }

    #[test]
    fn read_gate_reports_the_first_blocking_stage() {
        assert_eq!(read_gate(false, false, true), Ok(()));
        assert_eq!(
            read_gate(true, true, false),
            Err(StallReason::NetworkQueueFull)
        );
        assert_eq!(
            read_gate(false, false, false),
            Err(StallReason::MemoryPoolExhausted)
        );
    }

    #[test]
    fn read_ahead_is_bounded_and_resumes_after_flush() {
        let mut w = ReadAheadWindow::new(100);
        assert!(w.try_read(60));
        assert!(!w.try_read(60));
        w.on_flushed(60);
        assert!(w.try_read(60));
        assert!(!w.try_read(u64::MAX), "overflow is a refusal, not a wrap");
        w.on_flushed(u64::MAX);
        assert_eq!(w.in_flight(), 0);
    }
}
