//! §49-50 delivery ACK, §107 ACK aggregation, §108 negative ack.
//!
//! §107: aggregation "must not create false positive delivery
//! semantics". A Bloom filter would claim delivery of bundles that were
//! never delivered, so [`AckSummary`] is an exact, sorted, bounded ID set.

use serde::{Deserialize, Serialize};

use crate::types::BundleId;

pub const MAX_ACKS_PER_SUMMARY: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AckSummary {
    ids: Vec<BundleId>, // sorted by raw bytes, deduplicated
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ack summary with {0} ids exceeds {MAX_ACKS_PER_SUMMARY}")]
pub struct AckSummaryTooLarge(pub usize);

fn key(id: &BundleId) -> [u8; 16] {
    *id.as_bytes()
}

impl AckSummary {
    /// Builds a summary of at most [`MAX_ACKS_PER_SUMMARY`] ids; the rest
    /// are returned so the caller can send another summary — dropping
    /// them silently would make a delivered bundle look undelivered.
    pub fn from_ids(ids: impl IntoIterator<Item = BundleId>) -> (Self, Vec<BundleId>) {
        let mut v: Vec<BundleId> = ids.into_iter().collect();
        v.sort_unstable_by_key(key);
        v.dedup();
        let overflow = if v.len() > MAX_ACKS_PER_SUMMARY {
            v.split_off(MAX_ACKS_PER_SUMMARY)
        } else {
            Vec::new()
        };
        (Self { ids: v }, overflow)
    }

    /// Validates a peer-supplied summary (order, uniqueness, size) — the
    /// binary search in [`Self::contains`] is only correct on sorted input.
    pub fn validated(self) -> Result<Self, AckSummaryTooLarge> {
        if self.ids.len() > MAX_ACKS_PER_SUMMARY {
            return Err(AckSummaryTooLarge(self.ids.len()));
        }
        let (clean, _) = Self::from_ids(self.ids);
        Ok(clean)
    }

    pub fn contains(&self, id: &BundleId) -> bool {
        self.ids.binary_search_by_key(&key(id), key).is_ok()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &BundleId> {
        self.ids.iter()
    }
}

/// §108.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NackReason {
    RejectedQuota,
    Expired,
    Unsupported,
    Unauthorized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nack {
    pub bundle_id: BundleId,
    pub reason: NackReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryAdvice {
    /// Stop: retrying cannot succeed.
    Never,
    /// Quota pressure is transient: retry no sooner than this.
    NotBeforeMillis(u64),
}

/// Quota backoff doubles per failed attempt, capped at 1 h.
pub const QUOTA_BACKOFF_BASE_MILLIS: u64 = 30_000;
pub const QUOTA_BACKOFF_CAP_MILLIS: u64 = 3_600_000;

impl Nack {
    pub fn retry_advice(&self, now_millis: u64, prior_attempts: u32) -> RetryAdvice {
        match self.reason {
            NackReason::Expired | NackReason::Unsupported | NackReason::Unauthorized => {
                RetryAdvice::Never
            }
            NackReason::RejectedQuota => {
                let shift = prior_attempts.min(16);
                let delay = QUOTA_BACKOFF_BASE_MILLIS
                    .saturating_mul(1u64 << shift)
                    .min(QUOTA_BACKOFF_CAP_MILLIS);
                RetryAdvice::NotBeforeMillis(now_millis.saturating_add(delay))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_exact_sorted_deduped() {
        let a = BundleId::new();
        let b = BundleId::new();
        let (s, overflow) = AckSummary::from_ids([a, b, a]);
        assert!(overflow.is_empty());
        assert_eq!(s.len(), 2);
        assert!(s.contains(&a) && s.contains(&b));
        assert!(!s.contains(&BundleId::new()));
    }

    #[test]
    fn overflow_is_returned_not_dropped() {
        let ids: Vec<_> = (0..MAX_ACKS_PER_SUMMARY + 5)
            .map(|_| BundleId::new())
            .collect();
        let (s, overflow) = AckSummary::from_ids(ids.clone());
        assert_eq!(s.len(), MAX_ACKS_PER_SUMMARY);
        assert_eq!(overflow.len(), 5);
        for id in &ids {
            assert!(s.contains(id) ^ overflow.contains(id));
        }
    }

    #[test]
    fn unsorted_peer_summary_is_normalised_on_validation() {
        let mut ids: Vec<_> = (0..20).map(|_| BundleId::new()).collect();
        ids.sort_unstable_by_key(key);
        ids.reverse();
        let raw = AckSummary { ids: ids.clone() }.validated().unwrap();
        assert!(ids.iter().all(|i| raw.contains(i)));
        let big = AckSummary {
            ids: (0..MAX_ACKS_PER_SUMMARY + 1)
                .map(|_| BundleId::new())
                .collect(),
        };
        assert!(big.validated().is_err());
    }

    #[test]
    fn nack_retry_advice() {
        let n = |reason| Nack {
            bundle_id: BundleId::new(),
            reason,
        };
        assert_eq!(
            n(NackReason::Expired).retry_advice(0, 0),
            RetryAdvice::Never
        );
        assert_eq!(
            n(NackReason::Unauthorized).retry_advice(0, 0),
            RetryAdvice::Never
        );
        assert_eq!(
            n(NackReason::RejectedQuota).retry_advice(100, 0),
            RetryAdvice::NotBeforeMillis(30_100)
        );
        assert_eq!(
            n(NackReason::RejectedQuota).retry_advice(0, 99),
            RetryAdvice::NotBeforeMillis(QUOTA_BACKOFF_CAP_MILLIS)
        );
    }
}
