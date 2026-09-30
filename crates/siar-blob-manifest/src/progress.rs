//! §140 "Progress Model", §141 "ETA".

use serde::{Deserialize, Serialize};

use crate::transfer_state::TransferState;

/// §140's own struct, field-for-field, verbatim — the spec gives this
/// one as a literal Rust snippet, so there was no interpretation to
/// make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferProgress {
    pub total_bytes: u64,
    pub verified_bytes: u64,
    pub active_bytes_per_sec: Option<u64>,
    pub state: TransferState,
}

impl TransferProgress {
    /// Not in the spec's own snippet, but the obvious derived value a
    /// UI needs from these fields — `0.0` for a zero-byte transfer
    /// rather than a division-by-zero `NaN`, since "0 of 0 bytes" is
    /// unambiguously 100%, not undefined.
    pub fn percent_complete(&self) -> f64 {
        if self.total_bytes == 0 {
            return 100.0;
        }
        (self.verified_bytes as f64 / self.total_bytes as f64) * 100.0
    }
}

/// §141: "ETA is approximate. Do not present false precision." A
/// coarse enum, not a raw float or a `Duration` a UI might be tempted
/// to format down to the second — [`EtaEstimate::Unknown`] is a real,
/// distinct outcome (§141's own "omit when unstable"), not a sentinel
/// value smuggled through the number itself (like `0` or a negative
/// duration would be).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtaEstimate {
    ApproximateSeconds(u64),
    Unknown,
}

/// Unknown whenever there's no real rate to divide by — no active
/// transfer, a rate of exactly zero (stalled), or a transfer that's
/// already past its own total (nothing left to estimate). Otherwise
/// the straightforward `remaining / rate`, rounded down to the nearest
/// whole second — §141's own "~2 min," not "1 minute 58.3 seconds."
pub fn estimate_eta(progress: &TransferProgress) -> EtaEstimate {
    let Some(rate) = progress.active_bytes_per_sec else {
        return EtaEstimate::Unknown;
    };
    if rate == 0 {
        return EtaEstimate::Unknown;
    }
    let Some(remaining) = progress.total_bytes.checked_sub(progress.verified_bytes) else {
        return EtaEstimate::Unknown;
    };
    EtaEstimate::ApproximateSeconds(remaining / rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(total: u64, verified: u64, rate: Option<u64>) -> TransferProgress {
        TransferProgress {
            total_bytes: total,
            verified_bytes: verified,
            active_bytes_per_sec: rate,
            state: TransferState::InProgress,
        }
    }

    #[test]
    fn percent_complete_reflects_verified_over_total() {
        let p = progress(200, 50, None);
        assert_eq!(p.percent_complete(), 25.0);
    }

    #[test]
    fn a_zero_byte_transfer_is_always_one_hundred_percent() {
        let p = progress(0, 0, None);
        assert_eq!(p.percent_complete(), 100.0);
    }

    #[test]
    fn eta_is_unknown_with_no_active_rate() {
        let p = progress(1_000, 200, None);
        assert_eq!(estimate_eta(&p), EtaEstimate::Unknown);
    }

    #[test]
    fn eta_is_unknown_when_the_rate_has_stalled_to_zero() {
        let p = progress(1_000, 200, Some(0));
        assert_eq!(estimate_eta(&p), EtaEstimate::Unknown);
    }

    #[test]
    fn eta_divides_remaining_bytes_by_the_active_rate() {
        let p = progress(1_000, 200, Some(100));
        assert_eq!(estimate_eta(&p), EtaEstimate::ApproximateSeconds(8));
    }

    #[test]
    fn a_fully_verified_transfer_has_zero_seconds_remaining_not_unknown() {
        let p = progress(1_000, 1_000, Some(100));
        assert_eq!(estimate_eta(&p), EtaEstimate::ApproximateSeconds(0));
    }
}
