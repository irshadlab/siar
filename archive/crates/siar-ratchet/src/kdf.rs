//! Key derivation. Every call site uses a distinct, versioned `info` label
//! (§44 key separation, §48 downgrade resistance): a key derived for one
//! purpose can never be mistaken for another, and a suite change changes
//! every derived key.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

pub const ROOT_INFO: &[u8] = b"siar/ratchet/root/v1";
pub const MESSAGE_INFO: &[u8] = b"siar/ratchet/message/v1";
pub const HANDSHAKE_INFO: &[u8] = b"siar/ratchet/handshake/v1";

type HmacSha256 = Hmac<Sha256>;

/// HKDF-SHA256 (RFC 5869). `out.len()` must be ≤ 255·32; all callers here use
/// fixed small lengths, so a failure is a programming error.
pub fn hkdf_sha256(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out)
        .expect("fixed, small output length");
}

/// HMAC-SHA256 (RFC 2104 / 4231).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.finalize().into_bytes().into()
}

/// Root KDF: `(RK, DH output) → (RK', chain key)`.
pub fn kdf_root(rk: &[u8; 32], dh_out: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let mut out = Zeroizing::new([0u8; 64]);
    hkdf_sha256(rk, dh_out, ROOT_INFO, &mut *out);
    let (mut a, mut b) = (Zeroizing::new([0u8; 32]), Zeroizing::new([0u8; 32]));
    a.copy_from_slice(&out[..32]);
    b.copy_from_slice(&out[32..]);
    (a, b)
}

/// Chain KDF: `chain key → (next chain key, message key)` using the
/// constants from the Signal specification (`0x01` message, `0x02` chain), so
/// a leaked message key reveals nothing about the chain's future or past.
pub fn kdf_chain(ck: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    (
        Zeroizing::new(hmac_sha256(ck, &[0x02])),
        Zeroizing::new(hmac_sha256(ck, &[0x01])),
    )
}

/// Expand a one-use message key into an AEAD key and nonce. The nonce is
/// *derived from the single-use key*, never random (§17 nonce safety): two
/// messages can only share a nonce if they share a key, and no key is ever
/// used twice.
pub fn message_keys(mk: &[u8; 32]) -> (Zeroizing<[u8; 32]>, [u8; 12]) {
    let mut out = Zeroizing::new([0u8; 44]);
    hkdf_sha256(&[0u8; 32], mk, MESSAGE_INFO, &mut *out);
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&out[..32]);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&out[32..]);
    (key, nonce)
}
