//! §91 "Quarantine".

use serde::{Deserialize, Serialize};

use crate::ids::BlobId;

/// §91's own two named purposes for keeping a suspicious object around
/// at all, rather than deleting it immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuarantineReason {
    MalformedContent,
    FailedIntegrityCheck,
}

/// §91: "may be placed in quarantine for diagnostics before deletion.
/// Do not expose them to normal applications." A distinct type from
/// [`crate::storage_lifecycle::BlobClassification`] on purpose — that
/// enum answers "how should a healthy blob's lifecycle be managed"
/// (durable/cache/temporary/relay), a question that presumes the blob
/// is safe to expose in the first place. A quarantined object isn't a
/// fifth classification alongside those; it's outside that system
/// entirely, which is exactly what "do not expose to normal
/// applications" means — nothing about `BlobClassification` enforces
/// non-exposure, but a caller checking for a `QuarantineRecord` before
/// ever consulting classification does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineRecord {
    pub blob_id: BlobId,
    pub reason: QuarantineReason,
    pub quarantined_at_millis: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarantine_record_round_trips_through_postcard() {
        let record = QuarantineRecord {
            blob_id: BlobId::from_ciphertext(b"suspicious bytes"),
            reason: QuarantineReason::FailedIntegrityCheck,
            quarantined_at_millis: 5_000,
        };
        let bytes = postcard::to_allocvec(&record).unwrap();
        let decoded: QuarantineRecord = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, record);
    }
}
