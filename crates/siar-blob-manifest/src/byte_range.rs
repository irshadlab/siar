//! §86 `ByteRange` — a validated `[start, start + len)` window.
//!
//! "Avoid APIs that require materializing entire objects": every read
//! names the bytes it wants. The type exists so the arithmetic that is
//! classically wrong (`start + len` overflowing, a range past the end,
//! a zero/huge length) is done once, here, with checked operations.

use crate::store_error::BlobError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    start: u64,
    len: u64,
}

impl ByteRange {
    pub const fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    pub const fn start(&self) -> u64 {
        self.start
    }

    pub const fn len(&self) -> u64 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Exclusive end, or `None` on `u64` overflow.
    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.len)
    }

    /// Checks the range lies within a `size`-byte object and does not
    /// exceed `max_len`.
    pub fn validate(&self, size: u64, max_len: u64) -> Result<(), BlobError> {
        match self.end() {
            Some(end) if end <= size => {}
            _ => {
                return Err(BlobError::RangeOutOfBounds {
                    start: self.start,
                    len: self.len,
                    size,
                })
            }
        }
        if self.len > max_len {
            return Err(BlobError::RangeTooLarge {
                len: self.len,
                max: max_len,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn in_bounds_and_edge_cases() {
        assert!(ByteRange::new(0, 10).validate(10, 100).is_ok());
        assert!(
            ByteRange::new(10, 0).validate(10, 100).is_ok(),
            "empty range at the end"
        );
        assert!(matches!(
            ByteRange::new(5, 6).validate(10, 100),
            Err(BlobError::RangeOutOfBounds { .. })
        ));
        assert!(matches!(
            ByteRange::new(11, 0).validate(10, 100),
            Err(BlobError::RangeOutOfBounds { .. })
        ));
        assert!(matches!(
            ByteRange::new(0, 10).validate(10, 5),
            Err(BlobError::RangeTooLarge { max: 5, .. })
        ));
    }

    #[test]
    fn overflowing_ranges_are_rejected_not_wrapped() {
        let r = ByteRange::new(u64::MAX - 1, 10);
        assert_eq!(r.end(), None);
        assert!(matches!(
            r.validate(u64::MAX, u64::MAX),
            Err(BlobError::RangeOutOfBounds { .. })
        ));
    }

    proptest! {
        #[test]
        fn validate_never_panics_and_ok_implies_in_bounds(start in any::<u64>(), len in any::<u64>(), size in any::<u64>(), max in any::<u64>()) {
            let r = ByteRange::new(start, len);
            if r.validate(size, max).is_ok() {
                prop_assert!(start.checked_add(len).unwrap() <= size);
                prop_assert!(len <= max);
            }
        }
    }
}
