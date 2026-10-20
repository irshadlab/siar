//! §169-173: multi-recipient files, revocation limits, re-keying cost and
//! key caching.
//!
//! **§169/§170** One encrypted blob, plus one small wrapped copy of its
//! key per recipient ([`MultiRecipientBlob`]). A gigabyte sent to fifty
//! people is stored and transferred once; only 32-byte keys are
//! per-recipient.
//!
//! This module wraps with a **pairwise secret the caller supplies** (for
//! example derived from the session with that device). It does not do
//! key agreement and does not decide who the group is — §170: "exact
//! group crypto depends on group architecture". The wrap is
//! ChaCha20-Poly1305 with the blob id and recipient bound as associated
//! data, so a wrapped key cannot be replayed for another blob or another
//! person.
//!
//! **§171** [`MultiRecipientBlob::revoke`] removes a recipient's wrap, so
//! no *future* access is granted. It cannot take back a key the recipient
//! already unwrapped, and its return type says so
//! ([`RevocationEffect::StopsFutureAccessOnly`]) — there is no variant
//! that promises erasure.
//!
//! **§172** [`rekey_cost`] makes the expense visible before anyone
//! chooses to re-key.
//!
//! **§173** [`KeyCache`]: unwrapped keys live only in a small, bounded,
//! expiring, zeroizing cache and are never written anywhere.

use std::collections::HashMap;

use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, Key, Nonce};
use siar_domain::DeviceId;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::ids::{BlobEncryptionKey, BlobId};

/// Hard bound on recipients per blob (§139: nothing unbounded from a peer).
pub const MAX_RECIPIENTS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum KeyWrapError {
    #[error("wrapping failed")]
    WrapFailed,
    #[error("unwrapping failed — wrong secret, wrong blob/recipient, or tampering")]
    UnwrapFailed,
    #[error("recipient limit reached")]
    TooManyRecipients,
    #[error("recipient already has a wrapped key")]
    AlreadyPresent,
}

/// Encoded length of a [`WrappedKey`]: 12-byte nonce, 32-byte key, 16-byte tag.
pub const WRAPPED_KEY_LEN: usize = 12 + 32 + 16;

/// A blob key sealed for one recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedKey {
    nonce: [u8; 12],
    sealed: Vec<u8>,
}

impl WrappedKey {
    /// `nonce ‖ sealed`, always [`WRAPPED_KEY_LEN`] bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(WRAPPED_KEY_LEN);
        v.extend_from_slice(&self.nonce);
        v.extend_from_slice(&self.sealed);
        v
    }

    /// Strict: any length other than [`WRAPPED_KEY_LEN`] is refused. The
    /// content is still untrusted until [`unwrap_blob_key`] authenticates it.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != WRAPPED_KEY_LEN {
            return None;
        }
        let (nonce, sealed) = bytes.split_at(12);
        Some(Self {
            nonce: nonce.try_into().ok()?,
            sealed: sealed.to_vec(),
        })
    }
}

fn aad(blob: BlobId, recipient: DeviceId) -> Vec<u8> {
    let mut v = Vec::with_capacity(32 + 16);
    v.extend_from_slice(blob.as_bytes());
    v.extend_from_slice(recipient.to_string().as_bytes());
    v
}

pub fn wrap_blob_key(
    blob_key: &BlobEncryptionKey,
    recipient_secret: &[u8; 32],
    blob: BlobId,
    recipient: DeviceId,
) -> Result<WrappedKey, KeyWrapError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(recipient_secret));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let sealed = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &blob_key.0,
                aad: &aad(blob, recipient),
            },
        )
        .map_err(|_| KeyWrapError::WrapFailed)?;
    let mut n = [0u8; 12];
    n.copy_from_slice(&nonce);
    Ok(WrappedKey { nonce: n, sealed })
}

pub fn unwrap_blob_key(
    wrapped: &WrappedKey,
    recipient_secret: &[u8; 32],
    blob: BlobId,
    recipient: DeviceId,
) -> Result<BlobEncryptionKey, KeyWrapError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(recipient_secret));
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&wrapped.nonce),
                Payload {
                    msg: &wrapped.sealed,
                    aad: &aad(blob, recipient),
                },
            )
            .map_err(|_| KeyWrapError::UnwrapFailed)?,
    );
    let key: [u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| KeyWrapError::UnwrapFailed)?;
    Ok(BlobEncryptionKey(key))
}

/// §171: what revocation can honestly claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationEffect {
    /// No new wrapped key will be handed out. A key already unwrapped,
    /// and any ciphertext already held, are beyond recall.
    StopsFutureAccessOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revocation {
    pub effect: RevocationEffect,
    /// Whether the recipient had a wrap at all (so may already hold the
    /// key). When true and the file is sensitive, only re-keying
    /// (§172) changes anything — for copies not yet made.
    pub recipient_may_hold_key: bool,
}

/// One blob, many recipients.
#[derive(Debug, Clone)]
pub struct MultiRecipientBlob {
    blob_id: BlobId,
    wraps: Vec<(DeviceId, WrappedKey)>,
}

impl MultiRecipientBlob {
    pub fn new(blob_id: BlobId) -> Self {
        Self {
            blob_id,
            wraps: Vec::new(),
        }
    }

    pub fn blob_id(&self) -> BlobId {
        self.blob_id
    }

    pub fn recipients(&self) -> impl Iterator<Item = DeviceId> + '_ {
        self.wraps.iter().map(|(d, _)| *d)
    }

    pub fn len(&self) -> usize {
        self.wraps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.wraps.is_empty()
    }

    pub fn add_recipient(
        &mut self,
        blob_key: &BlobEncryptionKey,
        recipient_secret: &[u8; 32],
        recipient: DeviceId,
    ) -> Result<(), KeyWrapError> {
        if self.wraps.len() >= MAX_RECIPIENTS {
            return Err(KeyWrapError::TooManyRecipients);
        }
        if self.wraps.iter().any(|(d, _)| *d == recipient) {
            return Err(KeyWrapError::AlreadyPresent);
        }
        let w = wrap_blob_key(blob_key, recipient_secret, self.blob_id, recipient)?;
        self.wraps.push((recipient, w));
        Ok(())
    }

    pub fn wrapped_for(&self, recipient: DeviceId) -> Option<&WrappedKey> {
        self.wraps
            .iter()
            .find(|(d, _)| *d == recipient)
            .map(|(_, w)| w)
    }

    pub fn open_for(
        &self,
        recipient: DeviceId,
        recipient_secret: &[u8; 32],
    ) -> Result<BlobEncryptionKey, KeyWrapError> {
        let w = self
            .wrapped_for(recipient)
            .ok_or(KeyWrapError::UnwrapFailed)?;
        unwrap_blob_key(w, recipient_secret, self.blob_id, recipient)
    }

    pub fn revoke(&mut self, recipient: DeviceId) -> Revocation {
        let before = self.wraps.len();
        self.wraps.retain(|(d, _)| *d != recipient);
        Revocation {
            effect: RevocationEffect::StopsFutureAccessOnly,
            recipient_may_hold_key: self.wraps.len() != before,
        }
    }
}

/// §172: the price of changing a blob's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyCost {
    /// Every byte is re-encrypted (and so gets a new `BlobId`).
    pub bytes_to_reencrypt: u64,
    /// Every remaining recipient needs a fresh wrap.
    pub wraps_to_redo: usize,
}

pub fn rekey_cost(blob_size: u64, remaining_recipients: usize) -> RekeyCost {
    RekeyCost {
        bytes_to_reencrypt: blob_size,
        wraps_to_redo: remaining_recipients,
    }
}

// ---------------------------------------------------------------------
// §173 key cache
// ---------------------------------------------------------------------

struct CachedKey {
    key: Zeroizing<[u8; 32]>,
    inserted_millis: u64,
    last_used: u64,
}

/// Active keys only, a few at a time, for a short time. Zeroized on
/// eviction, expiry and [`KeyCache::clear`]; there is deliberately no
/// way to enumerate or serialize the contents.
pub struct KeyCache {
    max_entries: usize,
    ttl_millis: u64,
    entries: HashMap<BlobId, CachedKey>,
    tick: u64,
}

impl KeyCache {
    pub fn new(max_entries: usize, ttl_millis: u64) -> Self {
        Self {
            max_entries,
            ttl_millis,
            entries: HashMap::new(),
            tick: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, blob: BlobId, key: &BlobEncryptionKey, now_millis: u64) {
        if self.max_entries == 0 {
            return;
        }
        self.evict_expired(now_millis);
        if !self.entries.contains_key(&blob) && self.entries.len() >= self.max_entries {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| *k)
            {
                self.entries.remove(&oldest);
            }
        }
        self.tick += 1;
        self.entries.insert(
            blob,
            CachedKey {
                key: Zeroizing::new(key.0),
                inserted_millis: now_millis,
                last_used: self.tick,
            },
        );
    }

    pub fn get(&mut self, blob: BlobId, now_millis: u64) -> Option<BlobEncryptionKey> {
        let expired = self
            .entries
            .get(&blob)
            .is_some_and(|e| now_millis.saturating_sub(e.inserted_millis) >= self.ttl_millis);
        if expired {
            self.entries.remove(&blob);
            return None;
        }
        self.tick += 1;
        let tick = self.tick;
        let e = self.entries.get_mut(&blob)?;
        e.last_used = tick;
        Some(BlobEncryptionKey(*e.key))
    }

    pub fn evict_expired(&mut self, now_millis: u64) {
        let ttl = self.ttl_millis;
        self.entries
            .retain(|_, e| now_millis.saturating_sub(e.inserted_millis) < ttl);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::generate_blob_key;

    fn secret(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn a_recipient_unwraps_the_blob_key_with_their_own_secret() {
        let key = generate_blob_key();
        let (blob, who) = (BlobId::from_ciphertext(b"b"), DeviceId::new());
        let w = wrap_blob_key(&key, &secret(1), blob, who).unwrap();
        assert_eq!(unwrap_blob_key(&w, &secret(1), blob, who).unwrap(), key);
    }

    #[test]
    fn a_wrapped_key_survives_its_byte_encoding_and_refuses_wrong_lengths() {
        let key = generate_blob_key();
        let (blob, who) = (BlobId::from_ciphertext(b"b"), DeviceId::new());
        let w = wrap_blob_key(&key, &secret(1), blob, who).unwrap();
        let bytes = w.to_bytes();
        assert_eq!(bytes.len(), WRAPPED_KEY_LEN);
        let back = WrappedKey::from_bytes(&bytes).unwrap();
        assert_eq!(back, w);
        assert_eq!(unwrap_blob_key(&back, &secret(1), blob, who).unwrap(), key);
        for bad in [0usize, 1, WRAPPED_KEY_LEN - 1, WRAPPED_KEY_LEN + 1, 4096] {
            assert!(
                WrappedKey::from_bytes(&vec![0u8; bad]).is_none(),
                "len {bad}"
            );
        }
    }

    #[test]
    fn a_wrap_is_bound_to_its_secret_blob_and_recipient() {
        let key = generate_blob_key();
        let (blob, who) = (BlobId::from_ciphertext(b"b"), DeviceId::new());
        let w = wrap_blob_key(&key, &secret(1), blob, who).unwrap();
        assert_eq!(
            unwrap_blob_key(&w, &secret(2), blob, who),
            Err(KeyWrapError::UnwrapFailed),
            "wrong secret"
        );
        assert_eq!(
            unwrap_blob_key(&w, &secret(1), BlobId::from_ciphertext(b"other"), who),
            Err(KeyWrapError::UnwrapFailed),
            "replayed for another blob"
        );
        assert_eq!(
            unwrap_blob_key(&w, &secret(1), blob, DeviceId::new()),
            Err(KeyWrapError::UnwrapFailed),
            "replayed for another recipient"
        );
    }

    #[test]
    fn a_tampered_wrap_is_refused() {
        let key = generate_blob_key();
        let (blob, who) = (BlobId::from_ciphertext(b"b"), DeviceId::new());
        let mut w = wrap_blob_key(&key, &secret(1), blob, who).unwrap();
        w.sealed[0] ^= 1;
        assert!(unwrap_blob_key(&w, &secret(1), blob, who).is_err());
    }

    #[test]
    fn one_blob_serves_many_recipients_each_with_their_own_wrap() {
        let key = generate_blob_key();
        let mut m = MultiRecipientBlob::new(BlobId::from_ciphertext(b"big"));
        let people: Vec<(DeviceId, [u8; 32])> =
            (1..=5).map(|i| (DeviceId::new(), secret(i))).collect();
        for (d, s) in &people {
            m.add_recipient(&key, s, *d).unwrap();
        }
        assert_eq!(m.len(), 5);
        for (d, s) in &people {
            assert_eq!(m.open_for(*d, s).unwrap(), key);
        }
        // Someone else's secret does not open my wrap.
        assert!(m.open_for(people[0].0, &people[1].1).is_err());
        // Each wrap is distinct even for the same key.
        assert_ne!(m.wrapped_for(people[0].0), m.wrapped_for(people[1].0));
    }

    #[test]
    fn duplicates_and_the_recipient_cap_are_enforced() {
        let key = generate_blob_key();
        let mut m = MultiRecipientBlob::new(BlobId::from_ciphertext(b"x"));
        let d = DeviceId::new();
        m.add_recipient(&key, &secret(1), d).unwrap();
        assert_eq!(
            m.add_recipient(&key, &secret(1), d),
            Err(KeyWrapError::AlreadyPresent)
        );
        for _ in 1..MAX_RECIPIENTS {
            m.add_recipient(&key, &secret(1), DeviceId::new()).unwrap();
        }
        assert_eq!(
            m.add_recipient(&key, &secret(1), DeviceId::new()),
            Err(KeyWrapError::TooManyRecipients)
        );
    }

    #[test]
    fn revocation_stops_future_access_and_says_nothing_more() {
        let key = generate_blob_key();
        let mut m = MultiRecipientBlob::new(BlobId::from_ciphertext(b"x"));
        let (a, b) = (DeviceId::new(), DeviceId::new());
        m.add_recipient(&key, &secret(1), a).unwrap();
        m.add_recipient(&key, &secret(2), b).unwrap();
        // `a` unwrapped the key before being revoked: that copy is theirs.
        let already_held = m.open_for(a, &secret(1)).unwrap();

        let r = m.revoke(a);
        assert_eq!(r.effect, RevocationEffect::StopsFutureAccessOnly);
        assert!(r.recipient_may_hold_key);
        assert!(m.open_for(a, &secret(1)).is_err(), "no future access");
        assert_eq!(
            already_held, key,
            "…but what they already hold is unchanged"
        );
        assert!(m.open_for(b, &secret(2)).is_ok());
        // Revoking someone who never had access does not claim they might.
        assert!(!m.revoke(DeviceId::new()).recipient_may_hold_key);
    }

    #[test]
    fn rekeying_costs_the_whole_file_and_every_remaining_wrap() {
        let c = rekey_cost(5_000_000_000, 7);
        assert_eq!(c.bytes_to_reencrypt, 5_000_000_000);
        assert_eq!(c.wraps_to_redo, 7);
    }

    #[test]
    fn the_key_cache_is_bounded_expiring_and_clearable() {
        let mut c = KeyCache::new(2, 100);
        let (k1, k2, k3) = (
            generate_blob_key(),
            generate_blob_key(),
            generate_blob_key(),
        );
        let id = |n: u8| BlobId::from_ciphertext(&[n]);
        c.insert(id(1), &k1, 0);
        c.insert(id(2), &k2, 0);
        assert_eq!(c.get(id(1), 1), Some(k1)); // refreshes 1's recency
        c.insert(id(3), &k3, 2);
        assert_eq!(c.len(), 2);
        assert!(c.get(id(2), 3).is_none(), "least recently used went");
        assert_eq!(c.get(id(3), 3), Some(k3));
        assert!(c.get(id(1), 100).is_none(), "ttl reached");
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn a_zero_sized_cache_remembers_nothing() {
        let mut c = KeyCache::new(0, 100);
        c.insert(BlobId::from_ciphertext(b"x"), &generate_blob_key(), 0);
        assert!(c.is_empty());
    }
}
