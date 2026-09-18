//! Per-device sync bookkeeping.
//!
//! This file used to also hold a plan.md-era device-trust model
//! (`VerificationState`, `DeviceEvent`, `DeviceDescriptor`,
//! `DeviceRegistry` — plan.md §38-40, §130) alongside
//! `siar_crypto::device_cert`. Both have been retired in favor of
//! `siar-identity-multidevice`'s root-key-signed
//! `certificate::DeviceCertificate` + `directory::DeviceDirectory` —
//! see `MIGRATION.md`'s device-certificate reconciliation section for
//! the full disposition. [`SyncCursor`] is unrelated to that model
//! (it's about message-sync progress, not device trust) and stays.

use crate::DeviceId;
use serde::{Deserialize, Serialize};

/// plan.md §20's `sync_cursor`: how far a given device has caught up on
/// another device's message stream (plan.md §39: sender syncs to their
/// own other devices too, not just the recipient's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    pub device: DeviceId,
    pub last_sequence: u64,
}

impl SyncCursor {
    pub fn advance(self, sequence: u64) -> Self {
        Self {
            device: self.device,
            last_sequence: self.last_sequence.max(sequence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_cursor_only_moves_forward() {
        let device = DeviceId::new();
        let cursor = SyncCursor {
            device,
            last_sequence: 10,
        };
        assert_eq!(cursor.advance(15).last_sequence, 15);
        assert_eq!(cursor.advance(5).last_sequence, 10); // doesn't go backward
    }
}
