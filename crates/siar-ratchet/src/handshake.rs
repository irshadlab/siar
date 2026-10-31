//! Authenticated key agreement (§11, §115) — X3DH without one-time prekeys.
//!
//! The responder publishes a [`PrekeyBundle`]: an identity DH key and a
//! signed prekey, signed by its device signing key. The initiator checks that
//! signature against the **verifying key it already trusts** for that peer
//! (from pairing/QR/SAS, §37-§41), so a relay or DTN carrier that swaps the
//! bundle is caught. Three DHs mix both long-term identities and both
//! ephemerals; the identities and all public keys go into the KDF's `info`
//! and the ratchet's associated data (transcript binding, §15).
//!
//! **What this does not give:** with no one-time prekeys, the *first flight*
//! alone is protected only by the responder's signed prekey. It gains full
//! forward secrecy as soon as the first reply triggers a DH ratchet step.
//! Rotating the signed prekey regularly bounds that window.
//!
//! Key separation (§44): the identity DH key is *derived* from the device
//! identity under its own label; the long-term signing key is only ever used
//! to sign, and the static X25519 key used by the legacy `Session` is not
//! reused here.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rand_core::{CryptoRng, RngCore};
use serde::{Deserialize, Serialize};
use siar_crypto::DeviceIdentity;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

use crate::kdf::{hkdf_sha256, HANDSHAKE_INFO};
use crate::state::{RatchetError, RatchetState, SUITE};

const IDENTITY_DH_LABEL: &str = "siar/ratchet/identity-dh/v1";
const BUNDLE_SIG_DOMAIN: &[u8] = b"siar/ratchet/prekey-bundle/v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    #[error("prekey bundle signature is invalid")]
    BadSignature,
    #[error("prekey bundle is not signed by the key we trust for this peer")]
    WrongSigner,
    #[error("peer key is not acceptable")]
    WeakKey,
    #[error("the initial message targets a prekey we do not hold")]
    UnknownPrekey,
    #[error(transparent)]
    Ratchet(#[from] RatchetError),
}

fn identity_dh_secret(id: &DeviceIdentity) -> Zeroizing<[u8; 32]> {
    id.derive_secret(IDENTITY_DH_LABEL)
}

/// The identity DH public key peers will use for this device.
#[must_use]
pub fn identity_dh_public(id: &DeviceIdentity) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*identity_dh_secret(id))).to_bytes()
}

/// Secret half of a signed prekey. Zeroized on drop.
pub struct SignedPrekeySecret {
    secret: [u8; 32],
    public: [u8; 32],
}

impl Drop for SignedPrekeySecret {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl SignedPrekeySecret {
    #[must_use]
    pub const fn public(&self) -> [u8; 32] {
        self.public
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrekeyBundle {
    pub identity_dh_public: [u8; 32],
    pub signed_prekey_public: [u8; 32],
    pub signer: [u8; 32],
    #[serde(with = "sig_bytes")]
    pub signature: [u8; 64],
}

mod sig_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        v.try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))
    }
}

fn bundle_message(ik: &[u8; 32], spk: &[u8; 32]) -> Vec<u8> {
    let mut m = Vec::with_capacity(BUNDLE_SIG_DOMAIN.len() + 1 + 64);
    m.extend_from_slice(BUNDLE_SIG_DOMAIN);
    m.push(SUITE);
    m.extend_from_slice(ik);
    m.extend_from_slice(spk);
    m
}

/// Create a fresh signed prekey and the bundle that publishes it.
pub fn new_signed_prekey<R: RngCore + CryptoRng>(
    id: &DeviceIdentity,
    rng: &mut R,
) -> (SignedPrekeySecret, PrekeyBundle) {
    let s = StaticSecret::random_from_rng(rng);
    let public = PublicKey::from(&s).to_bytes();
    let ik = identity_dh_public(id);
    let signature = id.sign(&bundle_message(&ik, &public)).to_bytes();
    let bundle = PrekeyBundle {
        identity_dh_public: ik,
        signed_prekey_public: public,
        signer: id.verifying_key().to_bytes(),
        signature,
    };
    (
        SignedPrekeySecret {
            secret: s.to_bytes(),
            public,
        },
        bundle,
    )
}

impl PrekeyBundle {
    /// Check the bundle came from the device we trust. `trusted_signer` must
    /// come from the trust store, **not** from the bundle itself.
    pub fn verify(&self, trusted_signer: &VerifyingKey) -> Result<(), HandshakeError> {
        if self.signer != trusted_signer.to_bytes() {
            return Err(HandshakeError::WrongSigner);
        }
        let sig = Signature::from_bytes(&self.signature);
        trusted_signer
            .verify(
                &bundle_message(&self.identity_dh_public, &self.signed_prekey_public),
                &sig,
            )
            .map_err(|_| HandshakeError::BadSignature)
    }
}

/// Sent with the first ratchet message so the responder can derive the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialHeader {
    pub initiator_identity_dh: [u8; 32],
    pub initiator_ephemeral: [u8; 32],
    pub responder_prekey: [u8; 32],
}

impl InitialHeader {
    pub const LEN: usize = 96;

    #[must_use]
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut o = [0u8; Self::LEN];
        o[..32].copy_from_slice(&self.initiator_identity_dh);
        o[32..64].copy_from_slice(&self.initiator_ephemeral);
        o[64..].copy_from_slice(&self.responder_prekey);
        o
    }

    #[must_use]
    pub fn decode(b: &[u8]) -> Option<(Self, &[u8])> {
        let (h, rest) = b.split_first_chunk::<{ Self::LEN }>()?;
        let a = |r: std::ops::Range<usize>| -> [u8; 32] { h[r].try_into().expect("32 bytes") };
        Some((
            Self {
                initiator_identity_dh: a(0..32),
                initiator_ephemeral: a(32..64),
                responder_prekey: a(64..96),
            },
            rest,
        ))
    }
}

fn dh3(a: &[u8; 32], b: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, HandshakeError> {
    let s = StaticSecret::from(*a).diffie_hellman(&PublicKey::from(*b));
    if !s.was_contributory() {
        return Err(HandshakeError::WeakKey);
    }
    Ok(Zeroizing::new(s.to_bytes()))
}

/// Shared secret + associated data, identical on both sides.
fn derive(
    dh1: &[u8; 32],
    dh2: &[u8; 32],
    dh3_: &[u8; 32],
    ika: &[u8; 32],
    ikb: &[u8; 32],
    eka: &[u8; 32],
    spkb: &[u8; 32],
) -> (Zeroizing<[u8; 32]>, Vec<u8>) {
    let mut ikm = Zeroizing::new(Vec::with_capacity(32 * 4));
    ikm.extend_from_slice(&[0xFF; 32]); // domain-separation prefix (as in X3DH)
    for d in [dh1, dh2, dh3_] {
        ikm.extend_from_slice(d);
    }
    let mut info = Vec::new();
    info.extend_from_slice(HANDSHAKE_INFO);
    info.push(SUITE);
    for k in [ika, ikb, eka, spkb] {
        info.extend_from_slice(k);
    }
    let mut sk = Zeroizing::new([0u8; 32]);
    hkdf_sha256(&[0u8; 32], &ikm, &info, &mut *sk);
    // Associated data names both identities in a fixed (initiator, responder) order.
    let mut ad = Vec::with_capacity(64 + 8);
    ad.extend_from_slice(b"siar-ad1");
    ad.extend_from_slice(ika);
    ad.extend_from_slice(ikb);
    (sk, ad)
}

/// Initiator: authenticate the bundle, agree on a key, start the ratchet.
pub fn initiate<R: RngCore + CryptoRng>(
    me: &DeviceIdentity,
    bundle: &PrekeyBundle,
    trusted_signer: &VerifyingKey,
    rng: &mut R,
) -> Result<(RatchetState, InitialHeader), HandshakeError> {
    bundle.verify(trusted_signer)?;
    let ik_secret = identity_dh_secret(me);
    let ika = identity_dh_public(me);
    let eph = StaticSecret::random_from_rng(&mut *rng);
    let eka = PublicKey::from(&eph).to_bytes();
    let eph_secret = Zeroizing::new(eph.to_bytes());

    let dh1 = dh3(&ik_secret, &bundle.signed_prekey_public)?; // DH(IKa, SPKb)
    let dh2 = dh3(&eph_secret, &bundle.identity_dh_public)?; //  DH(EKa, IKb)
    let dh3_ = dh3(&eph_secret, &bundle.signed_prekey_public)?; // DH(EKa, SPKb)
    let (sk, ad) = derive(
        &dh1,
        &dh2,
        &dh3_,
        &ika,
        &bundle.identity_dh_public,
        &eka,
        &bundle.signed_prekey_public,
    );
    let state = RatchetState::init_initiator(&sk, bundle.signed_prekey_public, ad, rng)?;
    Ok((
        state,
        InitialHeader {
            initiator_identity_dh: ika,
            initiator_ephemeral: eka,
            responder_prekey: bundle.signed_prekey_public,
        },
    ))
}

/// Responder: derive the same key from the initiator's header.
pub fn respond(
    me: &DeviceIdentity,
    prekey: &SignedPrekeySecret,
    init: &InitialHeader,
) -> Result<RatchetState, HandshakeError> {
    if init.responder_prekey != prekey.public {
        return Err(HandshakeError::UnknownPrekey);
    }
    let ik_secret = identity_dh_secret(me);
    let ikb = identity_dh_public(me);
    let dh1 = dh3(&prekey.secret, &init.initiator_identity_dh)?; // DH(SPKb, IKa)
    let dh2 = dh3(&ik_secret, &init.initiator_ephemeral)?; //        DH(IKb, EKa)
    let dh3_ = dh3(&prekey.secret, &init.initiator_ephemeral)?; //   DH(SPKb, EKa)
    let (sk, ad) = derive(
        &dh1,
        &dh2,
        &dh3_,
        &init.initiator_identity_dh,
        &ikb,
        &init.initiator_ephemeral,
        &prekey.public,
    );
    Ok(RatchetState::init_responder(&sk, prekey.secret, ad))
}
