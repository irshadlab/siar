//! §30 "Bloom Filter Use" — a compact "bundles I already have" summary.
//!
//! False positives only cost a missed redundant-transfer opportunity;
//! the filter must **never** feed an authorization or security decision
//! (see [`crate::tombstone`] for the exact-ID structure used for
//! reintroduction screening).
//!
//! Hardening choices:
//! - A per-filter random `salt` is mixed into every hash, so two
//!   encounters' filters for the same inventory are not byte-identical
//!   (§35 encounter-history privacy: a filter must not be a stable
//!   fingerprint of what a node stores).
//! - Deserialization goes through [`BloomWire`] validation. A peer
//!   controls these bytes; an empty bit array would otherwise reach a
//!   `% 0` in [`BundleBloom::may_contain`] and panic the node.
//! - Size is capped at [`MAX_BLOOM_BITS`] so a hostile summary can't
//!   force a large allocation.

use serde::{Deserialize, Serialize};

use crate::types::BundleId;

/// 1 Mi bits = 128 KiB; far above any realistic per-encounter summary.
pub const MAX_BLOOM_BITS: u32 = 1 << 20;
pub const MIN_BLOOM_BITS: u32 = 64;
pub const MAX_BLOOM_HASHES: u8 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleBloom {
    bits: Box<[u64]>,
    bit_len: u32,
    hashes: u8,
    salt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BloomError {
    #[error("bloom bit length {0} outside [{MIN_BLOOM_BITS}, {MAX_BLOOM_BITS}]")]
    BitLength(u32),
    #[error("bloom hash count {0} outside [1, {MAX_BLOOM_HASHES}]")]
    HashCount(u8),
    #[error("bloom word buffer does not match declared bit length")]
    BufferMismatch,
}

#[derive(Serialize, Deserialize)]
struct BloomWire {
    words: Vec<u64>,
    bit_len: u32,
    hashes: u8,
    salt: u64,
}

impl Serialize for BundleBloom {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        BloomWire {
            words: self.bits.to_vec(),
            bit_len: self.bit_len,
            hashes: self.hashes,
            salt: self.salt,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for BundleBloom {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let w = BloomWire::deserialize(d)?;
        Self::from_parts(w.words, w.bit_len, w.hashes, w.salt).map_err(serde::de::Error::custom)
    }
}

impl BundleBloom {
    /// Sizes a filter for `expected_items` at roughly `false_positive_rate`.
    pub fn with_capacity(expected_items: usize, false_positive_rate: f64, salt: u64) -> Self {
        let n = expected_items.max(1) as f64;
        let p = false_positive_rate.clamp(1e-6, 0.5);
        let ln2 = std::f64::consts::LN_2;
        let m = (-(n * p.ln()) / (ln2 * ln2)).ceil();
        let bit_len = (m as u64).clamp(MIN_BLOOM_BITS as u64, MAX_BLOOM_BITS as u64) as u32;
        let k = ((bit_len as f64 / n) * ln2).round();
        let hashes = (k as u64).clamp(1, MAX_BLOOM_HASHES as u64) as u8;
        let words = vec![0u64; (bit_len as usize).div_ceil(64)];
        Self {
            bits: words.into_boxed_slice(),
            bit_len,
            hashes,
            salt,
        }
    }

    pub fn from_parts(
        words: Vec<u64>,
        bit_len: u32,
        hashes: u8,
        salt: u64,
    ) -> Result<Self, BloomError> {
        if !(MIN_BLOOM_BITS..=MAX_BLOOM_BITS).contains(&bit_len) {
            return Err(BloomError::BitLength(bit_len));
        }
        if hashes == 0 || hashes > MAX_BLOOM_HASHES {
            return Err(BloomError::HashCount(hashes));
        }
        if words.len() != (bit_len as usize).div_ceil(64) {
            return Err(BloomError::BufferMismatch);
        }
        Ok(Self {
            bits: words.into_boxed_slice(),
            bit_len,
            hashes,
            salt,
        })
    }

    fn probes(&self, id: &BundleId) -> impl Iterator<Item = u32> + '_ {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.salt.to_le_bytes());
        hasher.update(id.as_bytes());
        let digest = hasher.finalize();
        let b = digest.as_bytes();
        let h1 = u64::from_le_bytes(b[0..8].try_into().expect("8 bytes"));
        // Odd step so double hashing cycles through the whole table.
        let h2 = u64::from_le_bytes(b[8..16].try_into().expect("8 bytes")) | 1;
        let m = u64::from(self.bit_len);
        (0..u64::from(self.hashes)).map(move |i| (h1.wrapping_add(i.wrapping_mul(h2)) % m) as u32)
    }

    pub fn insert(&mut self, id: &BundleId) {
        let probes: Vec<u32> = self.probes(id).collect();
        for bit in probes {
            self.bits[(bit / 64) as usize] |= 1u64 << (bit % 64);
        }
    }

    /// `false` is definitive; `true` is "probably".
    pub fn may_contain(&self, id: &BundleId) -> bool {
        self.probes(id)
            .all(|bit| self.bits[(bit / 64) as usize] & (1u64 << (bit % 64)) != 0)
    }

    pub fn bit_len(&self) -> u32 {
        self.bit_len
    }

    pub fn salt(&self) -> u64 {
        self.salt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserted_ids_are_always_found() {
        let mut f = BundleBloom::with_capacity(500, 0.01, 7);
        let ids: Vec<_> = (0..500).map(|_| BundleId::new()).collect();
        for id in &ids {
            f.insert(id);
        }
        assert!(ids.iter().all(|id| f.may_contain(id)));
    }

    #[test]
    fn false_positive_rate_is_near_target() {
        let mut f = BundleBloom::with_capacity(1000, 0.01, 1);
        for _ in 0..1000 {
            f.insert(&BundleId::new());
        }
        let fp = (0..20_000)
            .filter(|_| f.may_contain(&BundleId::new()))
            .count();
        assert!(fp < 600, "fp={fp}/20000 should be ~1%");
    }

    #[test]
    fn different_salts_give_different_filters_for_same_inventory() {
        let ids: Vec<_> = (0..50).map(|_| BundleId::new()).collect();
        let (mut a, mut b) = (
            BundleBloom::with_capacity(50, 0.01, 1),
            BundleBloom::with_capacity(50, 0.01, 2),
        );
        for id in &ids {
            a.insert(id);
            b.insert(id);
        }
        assert_ne!(a, b);
    }

    #[test]
    fn hostile_wire_values_are_rejected_not_panicked_on() {
        assert_eq!(
            BundleBloom::from_parts(vec![], 0, 3, 0),
            Err(BloomError::BitLength(0))
        );
        assert_eq!(
            BundleBloom::from_parts(vec![0], 64, 0, 0),
            Err(BloomError::HashCount(0))
        );
        assert_eq!(
            BundleBloom::from_parts(vec![0; 3], 64, 3, 0),
            Err(BloomError::BufferMismatch)
        );
        let bad = postcard::to_allocvec(&(Vec::<u64>::new(), 0u32, 3u8, 0u64)).unwrap();
        assert!(postcard::from_bytes::<BundleBloom>(&bad).is_err());
    }

    #[test]
    fn postcard_roundtrip() {
        let mut f = BundleBloom::with_capacity(10, 0.01, 9);
        let id = BundleId::new();
        f.insert(&id);
        let bytes = postcard::to_allocvec(&f).unwrap();
        let back: BundleBloom = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(f, back);
        assert!(back.may_contain(&id));
    }
}
