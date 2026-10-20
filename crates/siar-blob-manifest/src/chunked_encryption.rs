//! Blob format v2: chunk-wise authenticated encryption (the STREAM
//! construction), which removes the "plaintext cannot be released before
//! the final tag verifies" limit of the whole-blob v1 format (see
//! [`crate::preprocess`]).
//!
//! ```text
//! ciphertext = header ‖ segment₀ ‖ segment₁ ‖ … ‖ segmentₙ
//! header     = "SBv2" ‖ segment_plaintext_size (u32 BE) ‖ nonce_prefix (7)
//! segmentᵢ   = ChaCha20-Poly1305(plaintext chunk, nonce = prefix ‖ i (u32 BE) ‖ last (1))
//!              with the header as associated data
//! ```
//!
//! * **Each segment authenticates on its own**, so a reader can verify
//!   and release plaintext segment by segment; an attacker cannot make it
//!   release forged bytes.
//! * **Reordering and splicing fail**: the segment index is in the nonce.
//! * **Truncation fails**: only the final segment is sealed with
//!   `last = 1`, and [`ChunkedDecryptor::finish`] errors if it never came
//!   — cutting the stream at a segment boundary is detected, not accepted
//!   as a shorter file.
//! * **The header is authenticated** (associated data), so the segment
//!   size and nonce prefix cannot be altered.
//! * `BlobId` stays BLAKE3 of the whole ciphertext (header included), so
//!   content addressing, manifests and dedup are unchanged.
//!
//! The key is fresh per blob (§19), so `(key, nonce)` never repeats. A
//! caller that must encrypt the same plaintext twice — hash first, then
//! store, to stay memory-bounded — gets byte-identical output from
//! [`ChunkedEncryptor::replay`] (same key, same prefix, same plaintext:
//! identical ciphertext, not nonce reuse over different data).
//!
//! Compatibility: nothing in a message's attachment reference says which
//! format a blob uses, so [`looks_chunked`] sniffs the magic and the
//! reader falls back to v1 if a v2 parse or decrypt fails (a v1 blob
//! starts with a random nonce, so it matches the magic by chance once in
//! 2³², and the fallback makes that harmless). A peer that only
//! understands v1 simply fails to decrypt a v2 blob, cleanly.

use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, Key, Nonce};
use thiserror::Error;

use crate::ids::BlobEncryptionKey;

pub const MAGIC: [u8; 4] = *b"SBv2";
pub const PREFIX_LEN: usize = 7;
pub const HEADER_LEN: usize = 4 + 4 + PREFIX_LEN;
pub const TAG_LEN: usize = 16;
/// Largest plaintext segment accepted from a header (bounds allocation).
pub const MAX_SEGMENT: u32 = 16 * 1024 * 1024;
/// Default plaintext segment size.
pub const DEFAULT_SEGMENT: u32 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ChunkedError {
    #[error("not a v2 blob header")]
    BadHeader,
    #[error("segment size is zero or over the limit")]
    BadSegmentSize,
    #[error("segment has the wrong length for its position")]
    SegmentLength,
    #[error("segment failed authentication — wrong key, corruption or tampering")]
    AuthFailed,
    #[error("stream ended before its final segment")]
    Truncated,
    #[error("data after the final segment")]
    TrailingData,
    #[error("the stream is already finished")]
    Finished,
    #[error("too many segments")]
    TooManySegments,
    #[error("encryption failed")]
    EncryptFailed,
}

fn cipher(key: &BlobEncryptionKey) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(&key.0))
}

fn nonce(prefix: &[u8; PREFIX_LEN], counter: u32, last: bool) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..PREFIX_LEN].copy_from_slice(prefix);
    n[PREFIX_LEN..PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    n[11] = u8::from(last);
    n
}

fn header_bytes(segment: u32, prefix: &[u8; PREFIX_LEN]) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(&MAGIC);
    h[4..8].copy_from_slice(&segment.to_be_bytes());
    h[8..].copy_from_slice(prefix);
    h
}

fn check_segment_size(s: u32) -> Result<(), ChunkedError> {
    if s == 0 || s > MAX_SEGMENT {
        Err(ChunkedError::BadSegmentSize)
    } else {
        Ok(())
    }
}

/// Does this ciphertext start like a v2 blob?
pub fn looks_chunked(ciphertext: &[u8]) -> bool {
    ciphertext.len() >= HEADER_LEN && ciphertext[..4] == MAGIC
}

/// Total ciphertext length for a plaintext of `plain_len` bytes, or
/// `None` on overflow. An empty plaintext is one empty (tag-only) segment.
pub fn ciphertext_len(plain_len: u64, segment: u32) -> Option<u64> {
    let seg = u64::from(segment);
    if seg == 0 {
        return None;
    }
    let segments = plain_len.div_ceil(seg).max(1);
    (HEADER_LEN as u64)
        .checked_add(plain_len)?
        .checked_add(segments.checked_mul(TAG_LEN as u64)?)
}

pub struct ChunkedEncryptor {
    key: BlobEncryptionKey,
    cipher: ChaCha20Poly1305,
    segment: u32,
    prefix: [u8; PREFIX_LEN],
    header: [u8; HEADER_LEN],
    counter: u32,
    finished: bool,
}

impl ChunkedEncryptor {
    pub fn new(key: &BlobEncryptionKey, segment: u32) -> Result<Self, ChunkedError> {
        // A random 96-bit nonce's first 7 bytes serve as the per-blob
        // prefix; the rest of the nonce is the counter and last flag.
        let n = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let mut prefix = [0u8; PREFIX_LEN];
        prefix.copy_from_slice(&n[..PREFIX_LEN]);
        Self::with_prefix(key, segment, prefix)
    }

    fn with_prefix(
        key: &BlobEncryptionKey,
        segment: u32,
        prefix: [u8; PREFIX_LEN],
    ) -> Result<Self, ChunkedError> {
        check_segment_size(segment)?;
        Ok(Self {
            key: *key,
            cipher: cipher(key),
            segment,
            prefix,
            header: header_bytes(segment, &prefix),
            counter: 0,
            finished: false,
        })
    }

    /// The blob header; it is the first [`HEADER_LEN`] bytes of the blob.
    pub fn header(&self) -> &[u8; HEADER_LEN] {
        &self.header
    }

    pub fn segment_size(&self) -> u32 {
        self.segment
    }

    /// A fresh encryptor with the same key and prefix: feeding it the
    /// same plaintext reproduces the same bytes (see module docs).
    pub fn replay(&self) -> Self {
        Self::with_prefix(&self.key, self.segment, self.prefix).expect("already validated")
    }

    /// Seals one segment. Every segment but the last must be exactly
    /// `segment_size` bytes; the last may be shorter (or empty, only for
    /// an empty file).
    pub fn encrypt_segment(&mut self, plain: &[u8], last: bool) -> Result<Vec<u8>, ChunkedError> {
        if self.finished {
            return Err(ChunkedError::Finished);
        }
        if plain.len() > self.segment as usize || (!last && plain.len() != self.segment as usize) {
            return Err(ChunkedError::SegmentLength);
        }
        let n = nonce(&self.prefix, self.counter, last);
        let sealed = self
            .cipher
            .encrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: plain,
                    aad: &self.header,
                },
            )
            .map_err(|_| ChunkedError::EncryptFailed)?;
        if last {
            self.finished = true;
        } else {
            self.counter = self
                .counter
                .checked_add(1)
                .ok_or(ChunkedError::TooManySegments)?;
        }
        Ok(sealed)
    }
}

pub struct ChunkedDecryptor {
    cipher: ChaCha20Poly1305,
    header: [u8; HEADER_LEN],
    segment: u32,
    prefix: [u8; PREFIX_LEN],
    counter: u32,
    finished: bool,
}

impl ChunkedDecryptor {
    pub fn new(key: &BlobEncryptionKey, header: &[u8]) -> Result<Self, ChunkedError> {
        let header: [u8; HEADER_LEN] = header
            .get(..HEADER_LEN)
            .and_then(|h| h.try_into().ok())
            .ok_or(ChunkedError::BadHeader)?;
        if header[..4] != MAGIC {
            return Err(ChunkedError::BadHeader);
        }
        let segment = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        check_segment_size(segment)?;
        let mut prefix = [0u8; PREFIX_LEN];
        prefix.copy_from_slice(&header[8..]);
        Ok(Self {
            cipher: cipher(key),
            header,
            segment,
            prefix,
            counter: 0,
            finished: false,
        })
    }

    pub fn segment_size(&self) -> u32 {
        self.segment
    }

    /// Ciphertext bytes of every segment but the last.
    pub fn stride(&self) -> usize {
        self.segment as usize + TAG_LEN
    }

    /// Authenticates and opens the next segment. Nothing is returned for
    /// a segment that fails.
    pub fn decrypt_segment(&mut self, segment: &[u8], last: bool) -> Result<Vec<u8>, ChunkedError> {
        if self.finished {
            return Err(ChunkedError::Finished);
        }
        let len_ok = if last {
            (TAG_LEN..=self.stride()).contains(&segment.len())
        } else {
            segment.len() == self.stride()
        };
        if !len_ok {
            return Err(ChunkedError::SegmentLength);
        }
        let n = nonce(&self.prefix, self.counter, last);
        let plain = self
            .cipher
            .decrypt(
                Nonce::from_slice(&n),
                Payload {
                    msg: segment,
                    aad: &self.header,
                },
            )
            .map_err(|_| ChunkedError::AuthFailed)?;
        if last {
            self.finished = true;
        } else {
            self.counter = self
                .counter
                .checked_add(1)
                .ok_or(ChunkedError::TooManySegments)?;
        }
        Ok(plain)
    }

    /// Call when the stream ends: errors unless the final segment was
    /// seen and verified.
    pub fn finish(self) -> Result<(), ChunkedError> {
        if self.finished {
            Ok(())
        } else {
            Err(ChunkedError::Truncated)
        }
    }
}

/// Whole-buffer convenience: the full v2 ciphertext.
pub fn encrypt_chunked(
    key: &BlobEncryptionKey,
    plaintext: &[u8],
    segment: u32,
) -> Result<Vec<u8>, ChunkedError> {
    let mut enc = ChunkedEncryptor::new(key, segment)?;
    let total =
        ciphertext_len(plaintext.len() as u64, segment).ok_or(ChunkedError::TooManySegments)?;
    let mut out = Vec::new();
    out.try_reserve_exact(usize::try_from(total).map_err(|_| ChunkedError::TooManySegments)?)
        .map_err(|_| ChunkedError::TooManySegments)?;
    out.extend_from_slice(enc.header());
    let seg = segment as usize;
    let mut chunks = plaintext.chunks(seg).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&enc.encrypt_segment(&[], true)?);
        return Ok(out);
    }
    while let Some(c) = chunks.next() {
        out.extend_from_slice(&enc.encrypt_segment(c, chunks.peek().is_none())?);
    }
    Ok(out)
}

/// Whole-buffer convenience. Verifies every segment, rejects truncation
/// and trailing data.
pub fn decrypt_chunked(
    key: &BlobEncryptionKey,
    ciphertext: &[u8],
) -> Result<Vec<u8>, ChunkedError> {
    let mut dec = ChunkedDecryptor::new(key, ciphertext)?;
    let body = &ciphertext[HEADER_LEN..];
    let stride = dec.stride();
    let mut out = Vec::new();
    out.try_reserve_exact(body.len())
        .map_err(|_| ChunkedError::TooManySegments)?;
    let mut rest = body;
    loop {
        let last = rest.len() <= stride;
        let (seg, tail) = rest.split_at(rest.len().min(stride));
        out.extend_from_slice(&dec.decrypt_segment(seg, last)?);
        if last {
            if !tail.is_empty() {
                return Err(ChunkedError::TrailingData);
            }
            break;
        }
        rest = tail;
    }
    dec.finish()?;
    Ok(out)
}

/// §129: [`decrypt_chunked`], returning bytes known to be authentic.
pub fn decrypt_chunked_verified(
    key: &BlobEncryptionKey,
    ciphertext: &[u8],
) -> Result<crate::preprocess::Verified<Vec<u8>>, ChunkedError> {
    decrypt_chunked(key, ciphertext).map(crate::preprocess::Verified::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::generate_blob_key;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn round_trips_across_segment_boundaries() {
        let key = generate_blob_key();
        for len in [0usize, 1, 99, 100, 101, 199, 200, 201, 1_000] {
            let ct = encrypt_chunked(&key, &data(len), 100).unwrap();
            assert_eq!(
                ct.len() as u64,
                ciphertext_len(len as u64, 100).unwrap(),
                "len {len}"
            );
            assert!(looks_chunked(&ct));
            assert_eq!(decrypt_chunked(&key, &ct).unwrap(), data(len), "len {len}");
        }
    }

    #[test]
    fn a_wrong_key_opens_nothing() {
        let ct = encrypt_chunked(&generate_blob_key(), &data(500), 100).unwrap();
        assert_eq!(
            decrypt_chunked(&generate_blob_key(), &ct),
            Err(ChunkedError::AuthFailed)
        );
    }

    #[test]
    fn any_single_bit_flip_anywhere_is_caught() {
        let key = generate_blob_key();
        let ct = encrypt_chunked(&key, &data(350), 100).unwrap();
        for i in 0..ct.len() {
            let mut bad = ct.clone();
            bad[i] ^= 0x01;
            assert!(
                decrypt_chunked(&key, &bad).is_err(),
                "flip at byte {i} was accepted"
            );
        }
    }

    #[test]
    fn truncating_at_a_segment_boundary_is_detected() {
        let key = generate_blob_key();
        let ct = encrypt_chunked(&key, &data(450), 100).unwrap();
        let stride = 100 + TAG_LEN;
        for keep in 1..=4 {
            let cut = &ct[..HEADER_LEN + keep * stride];
            assert!(decrypt_chunked(&key, cut).is_err(), "kept {keep} segments");
        }
        // And the streaming reader refuses to call it finished.
        let mut dec = ChunkedDecryptor::new(&key, &ct).unwrap();
        dec.decrypt_segment(&ct[HEADER_LEN..HEADER_LEN + stride], false)
            .unwrap();
        assert_eq!(dec.finish(), Err(ChunkedError::Truncated));
    }

    #[test]
    fn reordering_and_duplicating_segments_is_detected() {
        let key = generate_blob_key();
        let ct = encrypt_chunked(&key, &data(300), 100).unwrap();
        let s = 100 + TAG_LEN;
        let seg = |i: usize| ct[HEADER_LEN + i * s..HEADER_LEN + (i + 1) * s].to_vec();
        let mut swapped = ct[..HEADER_LEN].to_vec();
        swapped.extend(seg(1));
        swapped.extend(seg(0));
        swapped.extend(&ct[HEADER_LEN + 2 * s..]);
        assert!(decrypt_chunked(&key, &swapped).is_err());
        let mut dup = ct[..HEADER_LEN].to_vec();
        dup.extend(seg(0));
        dup.extend(seg(0));
        dup.extend(&ct[HEADER_LEN + 2 * s..]);
        assert!(decrypt_chunked(&key, &dup).is_err());
    }

    #[test]
    fn trailing_bytes_after_the_final_segment_are_refused() {
        let key = generate_blob_key();
        let mut ct = encrypt_chunked(&key, &data(100), 100).unwrap();
        ct.extend_from_slice(&[0u8; 5]);
        assert!(decrypt_chunked(&key, &ct).is_err());
    }

    #[test]
    fn the_header_is_authenticated() {
        let key = generate_blob_key();
        let mut ct = encrypt_chunked(&key, &data(250), 100).unwrap();
        // Change the declared segment size: the AAD no longer matches.
        ct[4..8].copy_from_slice(&50u32.to_be_bytes());
        assert!(decrypt_chunked(&key, &ct).is_err());
    }

    #[test]
    fn segment_sizes_are_bounded_and_validated() {
        let key = generate_blob_key();
        assert_eq!(
            ChunkedEncryptor::new(&key, 0).err(),
            Some(ChunkedError::BadSegmentSize)
        );
        assert_eq!(
            ChunkedEncryptor::new(&key, MAX_SEGMENT + 1).err(),
            Some(ChunkedError::BadSegmentSize)
        );
        let mut hdr = header_bytes(1, &[0; PREFIX_LEN]);
        hdr[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            ChunkedDecryptor::new(&key, &hdr).err(),
            Some(ChunkedError::BadSegmentSize)
        );
        assert_eq!(
            ChunkedDecryptor::new(&key, b"nope").err(),
            Some(ChunkedError::BadHeader)
        );
    }

    #[test]
    fn segment_length_rules_are_enforced_while_encrypting() {
        let key = generate_blob_key();
        let mut e = ChunkedEncryptor::new(&key, 10).unwrap();
        assert_eq!(
            e.encrypt_segment(&[0; 5], false),
            Err(ChunkedError::SegmentLength)
        );
        assert_eq!(
            e.encrypt_segment(&[0; 11], true),
            Err(ChunkedError::SegmentLength)
        );
        e.encrypt_segment(&[0; 10], false).unwrap();
        e.encrypt_segment(&[0; 3], true).unwrap();
        assert_eq!(
            e.encrypt_segment(&[0; 1], true),
            Err(ChunkedError::Finished)
        );
    }

    #[test]
    fn replay_reproduces_identical_bytes_for_the_two_pass_import() {
        let key = generate_blob_key();
        let plain = data(555);
        let mut first = ChunkedEncryptor::new(&key, 100).unwrap();
        let mut second = first.replay();
        assert_eq!(first.header(), second.header());
        let chunks: Vec<&[u8]> = plain.chunks(100).collect();
        for (i, c) in chunks.iter().enumerate() {
            let last = i + 1 == chunks.len();
            assert_eq!(
                first.encrypt_segment(c, last).unwrap(),
                second.encrypt_segment(c, last).unwrap()
            );
        }
    }

    #[test]
    fn fresh_blobs_get_fresh_prefixes() {
        let key = generate_blob_key();
        let a = ChunkedEncryptor::new(&key, 100).unwrap();
        let b = ChunkedEncryptor::new(&key, 100).unwrap();
        assert_ne!(a.header(), b.header());
    }

    #[test]
    fn a_v1_blob_is_not_mistaken_for_v2_by_decrypt() {
        let key = generate_blob_key();
        let (v1, _) = crate::encryption::encrypt_blob(&key, b"legacy").unwrap();
        // Even if the magic happened to match, a v2 decrypt would fail.
        assert!(decrypt_chunked(&key, &v1).is_err());
    }

    #[test]
    fn ciphertext_len_matches_reality_and_guards_overflow() {
        assert_eq!(ciphertext_len(0, 100), Some((HEADER_LEN + TAG_LEN) as u64));
        assert_eq!(
            ciphertext_len(100, 100),
            Some((HEADER_LEN + 100 + TAG_LEN) as u64)
        );
        assert_eq!(
            ciphertext_len(101, 100),
            Some((HEADER_LEN + 101 + 2 * TAG_LEN) as u64)
        );
        assert_eq!(ciphertext_len(u64::MAX, 1), None);
        assert_eq!(ciphertext_len(5, 0), None);
    }
}
