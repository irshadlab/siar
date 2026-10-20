//! Who has been given a key to which blob, and whose access has been
//! revoked. The service consults it before it will send a key or serve a
//! pull, and writes to it whenever it hands a key out.
//!
//! **What revocation can and cannot do** (`siar-blob-manifest::key_wrapping`
//! says the same at the key level): it stops *future* access — no further
//! key is sent, no pull is served, an unfinished transfer is cancelled. It
//! cannot take back a key a recipient already holds, nor ciphertext
//! already delivered. [`siar_blob_manifest::key_wrapping::Revocation`]
//! carries that, and `recipient_may_hold_key` says whether it could
//! matter for this recipient.
//!
//! **Durability.** [`MemoryAccessLedger`] lives in memory and forgets on
//! restart; with it, a revocation does not survive a restart and the
//! "had a key" answer is lost. A deployment that needs revocation to stick
//! implements [`AccessLedger`] over its own durable store and passes it in
//! `FilesConfig::access_ledger`.

use std::collections::HashSet;
use std::sync::Mutex;

use siar_blob_manifest::ids::BlobId;
use siar_domain::DeviceId;

pub trait AccessLedger: Send + Sync {
    /// A key for `blob` was sent to `device`.
    fn record_grant(&self, blob: BlobId, device: DeviceId);

    /// Revokes `device`'s access to `blob`. Returns whether a key had ever
    /// been granted to it (so it may still hold one).
    fn revoke(&self, blob: BlobId, device: DeviceId) -> bool;

    /// Lifts a revocation (the owner changed their mind). Grants already
    /// recorded are kept.
    fn restore(&self, blob: BlobId, device: DeviceId);

    fn is_revoked(&self, blob: BlobId, device: DeviceId) -> bool;
}

#[derive(Default)]
pub struct MemoryAccessLedger {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    granted: HashSet<(BlobId, DeviceId)>,
    revoked: HashSet<(BlobId, DeviceId)>,
}

impl MemoryAccessLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AccessLedger for MemoryAccessLedger {
    fn record_grant(&self, blob: BlobId, device: DeviceId) {
        self.inner
            .lock()
            .expect("access ledger")
            .granted
            .insert((blob, device));
    }

    fn revoke(&self, blob: BlobId, device: DeviceId) -> bool {
        let mut g = self.inner.lock().expect("access ledger");
        g.revoked.insert((blob, device));
        g.granted.contains(&(blob, device))
    }

    fn restore(&self, blob: BlobId, device: DeviceId) {
        self.inner
            .lock()
            .expect("access ledger")
            .revoked
            .remove(&(blob, device));
    }

    fn is_revoked(&self, blob: BlobId, device: DeviceId) -> bool {
        self.inner
            .lock()
            .expect("access ledger")
            .revoked
            .contains(&(blob, device))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(n: u8) -> BlobId {
        BlobId::from_ciphertext(&[n])
    }

    #[test]
    fn a_revocation_reports_whether_a_key_had_been_granted() {
        let l = MemoryAccessLedger::new();
        let (alice, bob) = (DeviceId::new(), DeviceId::new());
        l.record_grant(b(1), alice);
        assert!(l.revoke(b(1), alice), "alice was given a key");
        assert!(!l.revoke(b(1), bob), "bob never was");
        assert!(l.is_revoked(b(1), alice) && l.is_revoked(b(1), bob));
    }

    #[test]
    fn revocation_is_per_blob_and_per_device() {
        let l = MemoryAccessLedger::new();
        let (alice, bob) = (DeviceId::new(), DeviceId::new());
        l.revoke(b(1), alice);
        assert!(!l.is_revoked(b(2), alice), "another blob is unaffected");
        assert!(!l.is_revoked(b(1), bob), "another device is unaffected");
    }

    #[test]
    fn restoring_lifts_the_block_but_remembers_the_grant() {
        let l = MemoryAccessLedger::new();
        let d = DeviceId::new();
        l.record_grant(b(1), d);
        l.revoke(b(1), d);
        l.restore(b(1), d);
        assert!(!l.is_revoked(b(1), d));
        assert!(l.revoke(b(1), d), "the earlier grant is still on record");
    }
}
