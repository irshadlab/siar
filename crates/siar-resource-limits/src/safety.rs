//! §124 length-conversion safety, §125 decompression limits, §126
//! recursive-structure limits — the inbound-data guards that stop a
//! peer-controlled number from becoming an allocation or a stack blowup.

use std::cell::Cell;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SafetyError {
    #[error("length {0} does not fit the target integer type")]
    LengthOverflow(u64),
    #[error("declared length {declared} exceeds limit {limit}")]
    OverLimit { declared: u64, limit: u64 },
    #[error("decompressed output {produced} exceeds limit {limit}")]
    OutputTooLarge { produced: u64, limit: u64 },
    #[error("decompression ratio exceeded (output {produced} from input {consumed})")]
    RatioExceeded { produced: u64, consumed: u64 },
    #[error("nesting deeper than {0}")]
    TooDeep(u32),
}

/// `usize → u32` for wire length prefixes. `as u32` would silently wrap a
/// 4 GiB+ buffer to a tiny prefix and desynchronise the stream.
pub fn len_to_u32(len: usize) -> Result<u32, SafetyError> {
    u32::try_from(len).map_err(|_| SafetyError::LengthOverflow(len as u64))
}

/// Peer-declared `u64` length → `usize` that is also within `limit`
/// (checked *before* any allocation sized from it).
pub fn declared_len(declared: u64, limit: usize) -> Result<usize, SafetyError> {
    let n = usize::try_from(declared).map_err(|_| SafetyError::LengthOverflow(declared))?;
    if n > limit {
        return Err(SafetyError::OverLimit {
            declared,
            limit: limit as u64,
        });
    }
    Ok(n)
}

/// Streaming decompression budget: absolute output cap plus an
/// output:input ratio cap (a "zip bomb" is small input, huge output).
/// The ratio only applies once output exceeds `ratio_grace_bytes`, so
/// tiny payloads that legitimately expand a lot aren't rejected.
#[derive(Debug)]
pub struct DecompressionGuard {
    max_output: u64,
    max_ratio: u64,
    ratio_grace_bytes: u64,
    consumed: u64,
    produced: u64,
}

impl DecompressionGuard {
    pub fn new(max_output: u64, max_ratio: u64, ratio_grace_bytes: u64) -> Self {
        Self {
            max_output,
            max_ratio,
            ratio_grace_bytes,
            consumed: 0,
            produced: 0,
        }
    }

    /// Call after each decoder step with the bytes it read and wrote.
    pub fn record(&mut self, consumed: u64, produced: u64) -> Result<(), SafetyError> {
        self.consumed = self.consumed.saturating_add(consumed);
        self.produced = self.produced.saturating_add(produced);
        if self.produced > self.max_output {
            return Err(SafetyError::OutputTooLarge {
                produced: self.produced,
                limit: self.max_output,
            });
        }
        if self.produced > self.ratio_grace_bytes
            && self.produced > self.consumed.saturating_mul(self.max_ratio)
        {
            return Err(SafetyError::RatioExceeded {
                produced: self.produced,
                consumed: self.consumed,
            });
        }
        Ok(())
    }
}

/// Recursion/nesting limiter. Single-threaded by design (`Cell`): one
/// per parse. Scopes decrement on drop, including on early `?` returns.
#[derive(Debug)]
pub struct DepthLimiter {
    depth: Cell<u32>,
    max: u32,
}

#[derive(Debug)]
pub struct DepthScope<'a> {
    limiter: &'a DepthLimiter,
}

impl DepthLimiter {
    pub fn new(max: u32) -> Self {
        Self {
            depth: Cell::new(0),
            max,
        }
    }

    pub fn enter(&self) -> Result<DepthScope<'_>, SafetyError> {
        let d = self.depth.get();
        if d >= self.max {
            return Err(SafetyError::TooDeep(self.max));
        }
        self.depth.set(d + 1);
        Ok(DepthScope { limiter: self })
    }

    pub fn current(&self) -> u32 {
        self.depth.get()
    }
}

impl Drop for DepthScope<'_> {
    fn drop(&mut self) {
        self.limiter
            .depth
            .set(self.limiter.depth.get().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_conversions_refuse_instead_of_wrapping() {
        assert_eq!(len_to_u32(7), Ok(7));
        if usize::BITS == 64 {
            assert!(len_to_u32(u32::MAX as usize + 1).is_err());
        }
        assert_eq!(declared_len(10, 100), Ok(10));
        assert_eq!(
            declared_len(101, 100),
            Err(SafetyError::OverLimit {
                declared: 101,
                limit: 100
            })
        );
        assert!(declared_len(u64::MAX, usize::MAX).is_err() || usize::BITS == 64);
    }

    #[test]
    fn absolute_output_cap() {
        let mut g = DecompressionGuard::new(1_000, 1_000_000, 0);
        assert!(g.record(500, 600).is_ok());
        assert_eq!(
            g.record(1, 500),
            Err(SafetyError::OutputTooLarge {
                produced: 1_100,
                limit: 1_000
            })
        );
    }

    #[test]
    fn ratio_cap_catches_a_bomb_but_spares_small_payloads() {
        let mut bomb = DecompressionGuard::new(u64::MAX, 100, 4_096);
        assert!(bomb.record(10, 1_000).is_ok(), "under grace");
        assert!(matches!(
            bomb.record(10, 50_000),
            Err(SafetyError::RatioExceeded { .. })
        ));
        let mut ok = DecompressionGuard::new(u64::MAX, 100, 4_096);
        assert!(ok.record(1_000, 90_000).is_ok());
    }

    #[test]
    fn depth_limiter_releases_on_scope_exit_and_early_return() {
        let l = DepthLimiter::new(3);
        fn recurse(l: &DepthLimiter, n: u32) -> Result<(), SafetyError> {
            let _s = l.enter()?;
            if n > 0 {
                recurse(l, n - 1)?;
            }
            Ok(())
        }
        assert!(recurse(&l, 2).is_ok());
        assert_eq!(l.current(), 0);
        assert_eq!(recurse(&l, 3), Err(SafetyError::TooDeep(3)));
        assert_eq!(l.current(), 0, "unwound cleanly through ? returns");
    }
}
