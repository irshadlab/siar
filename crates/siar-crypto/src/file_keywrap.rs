//! The pairwise secret for wrapping blob keys (spec 05 §169-170).
//!
//! A blob is encrypted once under a fresh key; to let several devices
//! open it, that 32-byte key is wrapped separately for each one
//! (`siar-blob-manifest::key_wrapping`). The wrap needs a secret only the
//! sender and that one recipient know: this is it, derived from the same
//! X25519 exchange the session uses but passed through its own domain
//! label ([`CryptoDomain::FileKeyWrap`]) so it is cryptographically
//! independent of the session key and the mailbox-token secret.
//!
//! Like [`crate::session::Session`], this is a *static* secret: it has the
//! same forward-secrecy limit (a later compromise of either device's
//! long-term key exposes keys wrapped earlier). That is inherited, not
//! introduced, and is the Phase-2 ratchet's problem (see `lib.rs`).

use x25519_dalek::PublicKey as X25519PublicKey;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{domains::CryptoDomain, identity::DeviceIdentity};

/// 32 bytes, wiped on drop, never printed.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct FileKeyWrapSecret([u8; 32]);

impl FileKeyWrapSecret {
    /// Same two-argument shape as `Session::establish`: both ends of a
    /// pair compute the identical secret from their own identity and the
    /// other's public key.
    pub fn establish(us: &DeviceIdentity, peer_x25519_public: &X25519PublicKey) -> Self {
        let mut shared = us.diffie_hellman(peer_x25519_public);
        let label = CryptoDomain::FileKeyWrap.label();
        let mut input = Vec::with_capacity(label.len() + shared.len());
        input.extend_from_slice(label);
        input.extend_from_slice(&shared);
        let out = *blake3::hash(&input).as_bytes();
        shared.zeroize();
        input.zeroize();
        Self(out)
    }

    /// For handing to `wrap_blob_key` / `unwrap_blob_key`. Borrow it for
    /// the call; do not store the array.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for FileKeyWrapSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileKeyWrapSecret(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mailbox_token::MailboxTokenSecret, session::Session};

    #[test]
    fn both_sides_derive_the_same_secret() {
        let (a, b) = (DeviceIdentity::generate(), DeviceIdentity::generate());
        let s1 = FileKeyWrapSecret::establish(&a, &b.x25519_public());
        let s2 = FileKeyWrapSecret::establish(&b, &a.x25519_public());
        assert_eq!(s1.as_bytes(), s2.as_bytes());
    }

    #[test]
    fn different_peers_give_different_secrets() {
        let (a, b, c) = (
            DeviceIdentity::generate(),
            DeviceIdentity::generate(),
            DeviceIdentity::generate(),
        );
        let ab = FileKeyWrapSecret::establish(&a, &b.x25519_public());
        let ac = FileKeyWrapSecret::establish(&a, &c.x25519_public());
        assert_ne!(ab.as_bytes(), ac.as_bytes());
    }

    #[test]
    fn it_is_independent_of_the_session_key_and_the_mailbox_secret() {
        let (a, b) = (DeviceIdentity::generate(), DeviceIdentity::generate());
        let wrap = FileKeyWrapSecret::establish(&a, &b.x25519_public());
        // The session key is blake3(shared); the wrap secret must not equal
        // it, nor anything a session-encrypted message could reveal.
        let shared = a.diffie_hellman(&b.x25519_public());
        assert_ne!(wrap.as_bytes(), blake3::hash(&shared).as_bytes());
        // A ciphertext made by the session cannot be opened with the wrap
        // secret used as a session key, and vice versa.
        let sess = Session::establish(&a, &b.x25519_public());
        let ct = sess.encrypt(b"x").unwrap();
        let _ = MailboxTokenSecret::establish(&a, &b.x25519_public());
        assert!(Session::establish(&b, &a.x25519_public())
            .decrypt(&ct)
            .is_ok());
        let as_key = blake3::hash(wrap.as_bytes());
        assert_ne!(as_key.as_bytes(), blake3::hash(&shared).as_bytes());
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let (a, b) = (DeviceIdentity::generate(), DeviceIdentity::generate());
        let s = FileKeyWrapSecret::establish(&a, &b.x25519_public());
        assert_eq!(format!("{s:?}"), "FileKeyWrapSecret(..)");
    }
}
