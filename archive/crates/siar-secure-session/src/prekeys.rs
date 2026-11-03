//! Signed-prekey publication and rotation (§11, §46).
//!
//! The responder's signed prekey is what protects a first flight, so it is
//! **rotated** (limiting how much a later key compromise exposes) but the
//! previous ones are **retained for a while**, because a peer may legitimately
//! start a session from a bundle it fetched days ago. Beyond the retention
//! window the secret is deleted, and a stale first message is refused with a
//! clear error rather than silently falling back to something weaker.
//!
//! The file is encrypted at rest. A corrupt file is an **error, never a
//! silent regeneration**: regenerating would orphan every peer holding the old
//! bundle without anyone being told.

use std::sync::{Arc, Mutex, PoisonError};

use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use siar_crypto::DeviceIdentity;
use siar_ratchet::{new_signed_prekey, PrekeyBundle, SignedPrekeySecret};

use crate::cipher::{AtRestKey, CipherError};
use crate::store::BlobStore;

const KEY: &str = "prekeys.state";
const AAD: &[u8] = b"siar-prekeys/v1";

#[derive(Debug, Clone, Copy)]
pub struct PrekeyPolicy {
    pub rotate_every_ms: u64,
    pub retain_ms: u64,
    /// Hard cap on stored prekeys (newest kept).
    pub max_retained: usize,
}

impl Default for PrekeyPolicy {
    fn default() -> Self {
        const DAY: u64 = 86_400_000;
        Self {
            rotate_every_ms: 7 * DAY,
            retain_ms: 30 * DAY,
            max_retained: 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrekeyError {
    #[error("storage error: {0:?}")]
    Store(std::io::ErrorKind),
    #[error(transparent)]
    Cipher(#[from] CipherError),
    #[error("the prekey file is corrupt; refusing to regenerate it silently")]
    Corrupt,
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    created_ms: u64,
    secret: Vec<u8>,
    bundle: PrekeyBundle,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    keys: Vec<Stored>, // oldest first; the last is current
}

pub struct PrekeyManager {
    store: Arc<dyn BlobStore>,
    key: AtRestKey,
    policy: PrekeyPolicy,
    inner: Mutex<File>,
}

impl PrekeyManager {
    /// Load, or create the first prekey if none exists yet.
    pub fn open<R: RngCore + CryptoRng>(
        store: Arc<dyn BlobStore>,
        key: AtRestKey,
        identity: &DeviceIdentity,
        policy: PrekeyPolicy,
        now_ms: u64,
        rng: &mut R,
    ) -> Result<Self, PrekeyError> {
        let file = match store.get(KEY).map_err(|e| PrekeyError::Store(e.kind()))? {
            Some(blob) => {
                let plain = key.open(AAD, &blob)?;
                let f: File = postcard::from_bytes(&plain).map_err(|_| PrekeyError::Corrupt)?;
                if f.keys.is_empty() {
                    return Err(PrekeyError::Corrupt);
                }
                f
            }
            None => File::default(),
        };
        let m = Self {
            store,
            key,
            policy,
            inner: Mutex::new(file),
        };
        if m.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys
            .is_empty()
        {
            m.add_new(identity, now_ms, rng)?;
        }
        Ok(m)
    }

    fn persist(&self, f: &File) -> Result<(), PrekeyError> {
        let bytes = postcard::to_allocvec(f).map_err(|_| PrekeyError::Corrupt)?;
        self.store
            .put(KEY, &self.key.seal(AAD, &bytes))
            .map_err(|e| PrekeyError::Store(e.kind()))
    }

    fn add_new<R: RngCore + CryptoRng>(
        &self,
        identity: &DeviceIdentity,
        now_ms: u64,
        rng: &mut R,
    ) -> Result<(), PrekeyError> {
        let (secret, bundle) = new_signed_prekey(identity, rng);
        let mut f = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = File {
            keys: f.keys.clone(),
        };
        next.keys.push(Stored {
            created_ms: now_ms,
            secret: secret.to_bytes().to_vec(),
            bundle,
        });
        // Prune: drop expired ones (never the newest) and enforce the cap.
        let retain = self.policy.retain_ms;
        let newest = next.keys.len() - 1;
        let mut i = 0;
        next.keys.retain(|k| {
            let keep = i == newest || now_ms.saturating_sub(k.created_ms) <= retain;
            i += 1;
            keep
        });
        while next.keys.len() > self.policy.max_retained.max(1) {
            next.keys.remove(0);
        }
        self.persist(&next)?; // durable first…
        *f = next; // …then visible
        Ok(())
    }

    /// The bundle to publish (in a ticket, a directory entry, a mailbox record).
    #[must_use]
    pub fn current_bundle(&self) -> PrekeyBundle {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys
            .last()
            .expect("never empty")
            .bundle
            .clone()
    }

    /// Rotate if the current prekey is older than the policy allows.
    pub fn rotate_if_due<R: RngCore + CryptoRng>(
        &self,
        identity: &DeviceIdentity,
        now_ms: u64,
        rng: &mut R,
    ) -> Result<bool, PrekeyError> {
        let due = {
            let f = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            now_ms.saturating_sub(f.keys.last().expect("never empty").created_ms)
                >= self.policy.rotate_every_ms
        };
        if due {
            self.add_new(identity, now_ms, rng)?;
        }
        Ok(due)
    }

    /// The secret for a prekey a peer used, if we still hold it.
    #[must_use]
    pub fn secret_for(&self, public: &[u8; 32]) -> Option<SignedPrekeySecret> {
        let f = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let k = f
            .keys
            .iter()
            .find(|k| &k.bundle.signed_prekey_public == public)?;
        let bytes: [u8; 64] = k.secret.as_slice().try_into().ok()?;
        SignedPrekeySecret::from_bytes(&bytes).ok()
    }

    #[must_use]
    pub fn held(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemBlobStore;

    const DAY: u64 = 86_400_000;

    fn open(store: Arc<MemBlobStore>, id: &DeviceIdentity, now: u64) -> PrekeyManager {
        PrekeyManager::open(
            store,
            AtRestKey::derive(id, "prekeys"),
            id,
            PrekeyPolicy::default(),
            now,
            &mut rand_core::OsRng,
        )
        .unwrap()
    }

    #[test]
    fn the_first_prekey_is_created_signed_and_persisted_across_restarts() {
        let id = DeviceIdentity::generate();
        let store = Arc::new(MemBlobStore::new());
        let m = open(store.clone(), &id, 0);
        let b = m.current_bundle();
        assert!(b.verify(&id.verifying_key()).is_ok());
        let again = open(store, &id, 5 * DAY);
        assert_eq!(
            again.current_bundle(),
            b,
            "a restart does not change what we published"
        );
        assert!(again.secret_for(&b.signed_prekey_public).is_some());
    }

    #[test]
    fn rotation_publishes_a_new_prekey_but_keeps_the_old_secret_for_late_first_flights() {
        let id = DeviceIdentity::generate();
        let store = Arc::new(MemBlobStore::new());
        let m = open(store, &id, 0);
        let old = m.current_bundle();
        assert!(
            !m.rotate_if_due(&id, 6 * DAY, &mut rand_core::OsRng)
                .unwrap(),
            "not due yet"
        );
        assert!(m
            .rotate_if_due(&id, 8 * DAY, &mut rand_core::OsRng)
            .unwrap());
        let new = m.current_bundle();
        assert_ne!(old.signed_prekey_public, new.signed_prekey_public);
        assert!(
            m.secret_for(&old.signed_prekey_public).is_some(),
            "a peer holding the old bundle can still reach us"
        );
        assert!(m.secret_for(&new.signed_prekey_public).is_some());
    }

    #[test]
    fn expired_prekeys_are_deleted_and_forward_secrecy_improves() {
        let id = DeviceIdentity::generate();
        let m = open(Arc::new(MemBlobStore::new()), &id, 0);
        let old = m.current_bundle();
        m.rotate_if_due(&id, 8 * DAY, &mut rand_core::OsRng)
            .unwrap();
        m.rotate_if_due(&id, 40 * DAY, &mut rand_core::OsRng)
            .unwrap();
        assert!(
            m.secret_for(&old.signed_prekey_public).is_none(),
            "past retention the secret is gone"
        );
        assert!(m.held() <= 2);
    }

    #[test]
    fn the_cap_is_enforced_even_if_nothing_has_expired() {
        let id = DeviceIdentity::generate();
        let policy = PrekeyPolicy {
            rotate_every_ms: 1,
            retain_ms: u64::MAX,
            max_retained: 3,
        };
        let m = PrekeyManager::open(
            Arc::new(MemBlobStore::new()),
            AtRestKey::derive(&id, "prekeys"),
            &id,
            policy,
            0,
            &mut rand_core::OsRng,
        )
        .unwrap();
        for t in 1..10 {
            m.rotate_if_due(&id, t * 10, &mut rand_core::OsRng).unwrap();
        }
        assert_eq!(m.held(), 3);
    }

    #[test]
    fn a_corrupt_or_foreign_file_is_an_error_not_a_silent_new_identity() {
        let id = DeviceIdentity::generate();
        let store = Arc::new(MemBlobStore::new());
        open(store.clone(), &id, 0);
        let mut blob = store.get("prekeys.state").unwrap().unwrap();
        blob[40] ^= 1;
        store.put("prekeys.state", &blob).unwrap();
        let r = PrekeyManager::open(
            store.clone(),
            AtRestKey::derive(&id, "prekeys"),
            &id,
            PrekeyPolicy::default(),
            0,
            &mut rand_core::OsRng,
        );
        assert!(matches!(r, Err(PrekeyError::Cipher(_))));
        // A different device identity cannot open it either.
        let other = DeviceIdentity::generate();
        let r = PrekeyManager::open(
            store,
            AtRestKey::derive(&other, "prekeys"),
            &other,
            PrekeyPolicy::default(),
            0,
            &mut rand_core::OsRng,
        );
        assert!(r.is_err());
    }

    #[test]
    fn a_failed_write_does_not_publish_a_prekey_we_could_not_save() {
        use crate::store::FaultyStore;
        let id = DeviceIdentity::generate();
        let f = Arc::new(FaultyStore::new(MemBlobStore::new(), "prekeys"));
        let m = PrekeyManager::open(
            f.clone(),
            AtRestKey::derive(&id, "prekeys"),
            &id,
            PrekeyPolicy::default(),
            0,
            &mut rand_core::OsRng,
        )
        .unwrap();
        let before = m.current_bundle();
        f.fail_next(1);
        assert!(m
            .rotate_if_due(&id, 8 * DAY, &mut rand_core::OsRng)
            .is_err());
        assert_eq!(
            m.current_bundle(),
            before,
            "never publish a key whose secret is not durable"
        );
        assert!(m
            .rotate_if_due(&id, 8 * DAY, &mut rand_core::OsRng)
            .unwrap());
    }
}
