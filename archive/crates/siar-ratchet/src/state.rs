//! The Double Ratchet (Signal specification, §3), with the properties Part 28
//! asks of it:
//!
//! * **Forward secrecy (§12).** Message keys are single-use and deleted on
//!   use; chain keys only move forward. A state captured *now* cannot decrypt
//!   anything received *before* now.
//! * **Post-compromise recovery (§13).** Every reply performs a fresh DH
//!   ratchet step, so a state captured earlier stops being able to follow the
//!   conversation after one round trip.
//! * **Crash safety (§75-§77).** The API is *functional*: [`RatchetState::encrypt`]
//!   and [`RatchetState::decrypt`] never mutate `self`; they return the **next**
//!   state. A failed or tampered decrypt therefore cannot advance or corrupt
//!   anything (§77 receive transaction), and the only way to use the API is
//!   "persist the returned state, *then* transmit" (§76: a crash can skip a
//!   key but can never reuse one).
//! * **Replay (§16) and nonce safety (§17).** A consumed key is gone, so a
//!   replay cannot decrypt; the nonce is derived from the single-use key.
//! * **Bounded work (§60).** Skipped-key work and storage are capped, so a
//!   hostile peer cannot make us hash or store without limit.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::kdf::{kdf_chain, kdf_root, message_keys};

pub const VERSION: u8 = 1;
/// X25519 · HKDF-SHA256/HMAC-SHA256 · ChaCha20-Poly1305. Part of every AAD and
/// every KDF label's version, and **never negotiated down** (§48): an unknown
/// suite is an error, not a fallback.
pub const SUITE: u8 = 1;
pub const MAX_SKIP: u32 = 1000;
pub const MAX_STORED_SKIPPED: usize = 2000;
/// How many previous peer DH keys we remember so a replay from an old epoch
/// is rejected immediately, without hashing through its claimed gap.
pub const MAX_OLD_DH: usize = 32;
pub const HEADER_LEN: usize = 2 + 32 + 4 + 4;
const TAG_LEN: usize = 16;
const MAX_STATE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RatchetError {
    #[error("message is malformed")]
    Malformed,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported cipher suite {0} (no fallback is ever attempted)")]
    UnsupportedSuite(u8),
    /// A responder cannot send before it has received the first message.
    #[error("no sending chain yet")]
    NoSendingChain,
    #[error("too many messages skipped; the session needs to be re-established")]
    TooManySkipped,
    /// Already received, or too old to keep keys for.
    #[error("duplicate or expired message")]
    DuplicateOrExpired,
    #[error("authentication failed")]
    AuthenticationFailed,
    #[error("message counter exhausted; the session needs to be re-established")]
    CounterExhausted,
    #[error("peer key is not acceptable")]
    WeakKey,
    /// The stored state is older than one we already used (§24, §103).
    #[error("stored session state is older than state already used; re-establish the session")]
    StaleState,
    #[error("session state is invalid")]
    InvalidState,
}

#[derive(Clone, Serialize, Deserialize)]
struct SkippedKey {
    dh: [u8; 32],
    n: u32,
    mk: [u8; 32],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RatchetState {
    version: u8,
    suite: u8,
    dhs_secret: [u8; 32],
    dhs_public: [u8; 32],
    dhr: Option<[u8; 32]>,
    rk: [u8; 32],
    cks: Option<[u8; 32]>,
    ckr: Option<[u8; 32]>,
    ns: u32,
    nr: u32,
    pn: u32,
    skipped: Vec<SkippedKey>,
    /// Peer DH keys of chains we have already moved past (bounded, FIFO).
    old_dhr: Vec<[u8; 32]>,
    /// Session-wide associated data: binds the conversation to both identities (§15).
    ad: Vec<u8>,
    /// Increments on every transition; compared against a high-water mark kept
    /// *outside* any backup to detect a stale restore (§24, §103).
    epoch: u64,
}

impl std::fmt::Debug for RatchetState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RatchetState")
            .field("epoch", &self.epoch)
            .field("ns", &self.ns)
            .field("nr", &self.nr)
            .field("skipped", &self.skipped.len())
            .finish_non_exhaustive() // never prints a key
    }
}

impl Drop for RatchetState {
    fn drop(&mut self) {
        self.dhs_secret.zeroize();
        self.rk.zeroize();
        if let Some(k) = &mut self.cks {
            k.zeroize();
        }
        if let Some(k) = &mut self.ckr {
            k.zeroize();
        }
        for s in &mut self.skipped {
            s.mk.zeroize();
        }
    }
}

struct Header {
    dh: [u8; 32],
    pn: u32,
    n: u32,
}

impl Header {
    fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0] = VERSION;
        out[1] = SUITE;
        out[2..34].copy_from_slice(&self.dh);
        out[34..38].copy_from_slice(&self.pn.to_le_bytes());
        out[38..42].copy_from_slice(&self.n.to_le_bytes());
        out
    }

    fn parse(msg: &[u8]) -> Result<(Self, &[u8]), RatchetError> {
        if msg.len() < HEADER_LEN + TAG_LEN {
            return Err(RatchetError::Malformed);
        }
        if msg[0] != VERSION {
            return Err(RatchetError::UnsupportedVersion(msg[0]));
        }
        if msg[1] != SUITE {
            return Err(RatchetError::UnsupportedSuite(msg[1]));
        }
        let mut dh = [0u8; 32];
        dh.copy_from_slice(&msg[2..34]);
        let pn = u32::from_le_bytes(msg[34..38].try_into().expect("4 bytes"));
        let n = u32::from_le_bytes(msg[38..42].try_into().expect("4 bytes"));
        Ok((Self { dh, pn, n }, &msg[HEADER_LEN..]))
    }
}

fn dh(secret: &[u8; 32], public: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, RatchetError> {
    let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(*public));
    // A low-order peer key yields an all-zero secret that an attacker could
    // predict; refuse it rather than ratchet on it.
    if !shared.was_contributory() {
        return Err(RatchetError::WeakKey);
    }
    Ok(Zeroizing::new(shared.to_bytes()))
}

fn new_keypair<R: RngCore + CryptoRng>(rng: &mut R) -> ([u8; 32], [u8; 32]) {
    let s = StaticSecret::random_from_rng(rng);
    (s.to_bytes(), PublicKey::from(&s).to_bytes())
}

/// AAD = len(ad) ‖ ad ‖ version ‖ suite ‖ header. Binding the identities, the
/// suite and the header means none of them can be altered or replayed into a
/// different conversation without failing authentication.
fn aad(ad: &[u8], header: &[u8; HEADER_LEN]) -> Vec<u8> {
    let mut a = Vec::with_capacity(4 + ad.len() + HEADER_LEN);
    a.extend_from_slice(&u32::try_from(ad.len()).unwrap_or(u32::MAX).to_le_bytes());
    a.extend_from_slice(ad);
    a.extend_from_slice(header);
    a
}

impl RatchetState {
    /// The party that sends first. `their_ratchet_pub` is the responder's
    /// signed prekey (already authenticated by the handshake).
    pub fn init_initiator<R: RngCore + CryptoRng>(
        sk: &[u8; 32],
        their_ratchet_pub: [u8; 32],
        ad: Vec<u8>,
        rng: &mut R,
    ) -> Result<Self, RatchetError> {
        let (dhs_secret, dhs_public) = new_keypair(rng);
        let out = dh(&dhs_secret, &their_ratchet_pub)?;
        let (rk, cks) = kdf_root(sk, &out);
        Ok(Self {
            version: VERSION,
            suite: SUITE,
            dhs_secret,
            dhs_public,
            dhr: Some(their_ratchet_pub),
            rk: *rk,
            cks: Some(*cks),
            ckr: None,
            ns: 0,
            nr: 0,
            pn: 0,
            skipped: Vec::new(),
            old_dhr: Vec::new(),
            ad,
            epoch: 0,
        })
    }

    /// The party that receives first; `our_ratchet_secret` is the signed
    /// prekey secret the initiator targeted.
    #[must_use]
    pub fn init_responder(sk: &[u8; 32], our_ratchet_secret: [u8; 32], ad: Vec<u8>) -> Self {
        let dhs_public = PublicKey::from(&StaticSecret::from(our_ratchet_secret)).to_bytes();
        Self {
            version: VERSION,
            suite: SUITE,
            dhs_secret: our_ratchet_secret,
            dhs_public,
            dhr: None,
            rk: *sk,
            cks: None,
            ckr: None,
            ns: 0,
            nr: 0,
            pn: 0,
            skipped: Vec::new(),
            old_dhr: Vec::new(),
            ad,
            epoch: 0,
        }
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub fn skipped_len(&self) -> usize {
        self.skipped.len()
    }

    /// Encrypt one message. **Persist the returned state before sending the
    /// bytes** (§75-§76): if the process dies in between, a message key is
    /// skipped (harmless); if the order were reversed, a crash could re-send
    /// from the *old* state and reuse a key.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Self), RatchetError> {
        let mut s = self.clone();
        let cks = s.cks.ok_or(RatchetError::NoSendingChain)?;
        if s.ns == u32::MAX {
            return Err(RatchetError::CounterExhausted);
        }
        let (next_ck, mk) = kdf_chain(&cks);
        s.cks = Some(*next_ck);
        let header = Header {
            dh: s.dhs_public,
            pn: s.pn,
            n: s.ns,
        }
        .encode();
        s.ns += 1;
        s.epoch += 1;
        let (key, nonce) = message_keys(&mk);
        let ct = ChaCha20Poly1305::new((&*key).into())
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad(&s.ad, &header),
                },
            )
            .map_err(|_| RatchetError::AuthenticationFailed)?;
        let mut wire = Vec::with_capacity(HEADER_LEN + ct.len());
        wire.extend_from_slice(&header);
        wire.extend_from_slice(&ct);
        Ok((wire, s))
    }

    /// Decrypt one message with the operating-system RNG.
    pub fn decrypt(&self, wire: &[u8]) -> Result<(Vec<u8>, Self), RatchetError> {
        self.decrypt_with(wire, &mut rand_core::OsRng)
    }

    /// Decrypt, returning the **next** state. `self` is untouched on every
    /// error, so a forged or corrupted message costs nothing but the work of
    /// rejecting it (§77). Persist the returned state atomically with the
    /// stored plaintext.
    pub fn decrypt_with<R: RngCore + CryptoRng>(
        &self,
        wire: &[u8],
        rng: &mut R,
    ) -> Result<(Vec<u8>, Self), RatchetError> {
        let (h, ct) = Header::parse(wire)?;
        let header_bytes = h.encode();
        let mut s = self.clone();

        // 1. A key we skipped earlier?
        if let Some(i) = s.skipped.iter().position(|k| k.dh == h.dh && k.n == h.n) {
            let mk = Zeroizing::new(s.skipped[i].mk);
            let pt = open(&s.ad, &header_bytes, &mk, ct)?;
            s.skipped.remove(i);
            s.epoch += 1;
            return Ok((pt, s));
        }

        // A chain we already left, and whose key was not kept, is a replay or
        // an expired message — say so without doing any derivation work.
        if s.dhr != Some(h.dh) && s.old_dhr.contains(&h.dh) {
            return Err(RatchetError::DuplicateOrExpired);
        }

        // 2. A new sending chain from the peer ⇒ DH ratchet step.
        if s.dhr != Some(h.dh) {
            s.skip_message_keys(h.pn)?;
            s.dh_ratchet(h.dh, rng)?;
        }
        // Old chain, key already used (or never kept): that is a duplicate/expired message.
        if h.n < s.nr {
            return Err(RatchetError::DuplicateOrExpired);
        }
        s.skip_message_keys(h.n)?;
        let ckr = s.ckr.ok_or(RatchetError::Malformed)?;
        let (next_ck, mk) = kdf_chain(&ckr);
        s.ckr = Some(*next_ck);
        s.nr += 1;
        let pt = open(&s.ad, &header_bytes, &mk, ct)?;
        s.epoch += 1;
        Ok((pt, s))
    }

    fn skip_message_keys(&mut self, until: u32) -> Result<(), RatchetError> {
        let Some(mut ckr) = self.ckr else {
            return Ok(());
        };
        if until < self.nr {
            return Ok(());
        }
        if until - self.nr > MAX_SKIP {
            return Err(RatchetError::TooManySkipped);
        }
        let dhr = self.dhr.ok_or(RatchetError::Malformed)?;
        while self.nr < until {
            let (next, mk) = kdf_chain(&ckr);
            ckr = *next;
            self.skipped.push(SkippedKey {
                dh: dhr,
                n: self.nr,
                mk: *mk,
            });
            self.nr += 1;
        }
        while self.skipped.len() > MAX_STORED_SKIPPED {
            let mut old = self.skipped.remove(0);
            old.mk.zeroize();
        }
        self.ckr = Some(ckr);
        Ok(())
    }

    fn dh_ratchet<R: RngCore + CryptoRng>(
        &mut self,
        their: [u8; 32],
        rng: &mut R,
    ) -> Result<(), RatchetError> {
        self.pn = self.ns;
        self.ns = 0;
        self.nr = 0;
        if let Some(prev) = self.dhr {
            self.old_dhr.push(prev);
            if self.old_dhr.len() > MAX_OLD_DH {
                self.old_dhr.remove(0);
            }
        }
        self.dhr = Some(their);
        let (rk, ckr) = kdf_root(&self.rk, &*dh(&self.dhs_secret, &their)?);
        self.rk = *rk;
        self.ckr = Some(*ckr);
        let (secret, public) = new_keypair(rng);
        self.dhs_secret.zeroize();
        self.dhs_secret = secret;
        self.dhs_public = public;
        let (rk, cks) = kdf_root(&self.rk, &*dh(&self.dhs_secret, &their)?);
        self.rk = *rk;
        self.cks = Some(*cks);
        Ok(())
    }

    /// Serialize for storage. **This contains secrets**: encrypt it with a
    /// key from the platform key store before it touches disk (§50-§51).
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(postcard::to_allocvec(self).unwrap_or_default())
    }

    /// Parse and **validate** stored state.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RatchetError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(RatchetError::InvalidState);
        }
        let s: Self = postcard::from_bytes(bytes).map_err(|_| RatchetError::InvalidState)?;
        let consistent = s.version == VERSION
            && s.suite == SUITE
            && s.skipped.len() <= MAX_STORED_SKIPPED
            && s.old_dhr.len() <= MAX_OLD_DH
            && s.ad.len() <= 1024
            && PublicKey::from(&StaticSecret::from(s.dhs_secret)).to_bytes() == s.dhs_public
            && (s.dhr.is_some() || s.ckr.is_none());
        if consistent {
            Ok(s)
        } else {
            Err(RatchetError::InvalidState)
        }
    }
}

fn open(
    ad: &[u8],
    header: &[u8; HEADER_LEN],
    mk: &[u8; 32],
    ct: &[u8],
) -> Result<Vec<u8>, RatchetError> {
    let (key, nonce) = message_keys(mk);
    ChaCha20Poly1305::new((&*key).into())
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: ct,
                aad: &aad(ad, header),
            },
        )
        .map_err(|_| RatchetError::AuthenticationFailed)
}
