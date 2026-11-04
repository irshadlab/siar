//! FROST (RFC 9591) threshold signing for the account **root key**.
//!
//! `frost-ed25519` emits ordinary Ed25519 signatures under an ordinary
//! 32-byte Ed25519 public key, so the rest of Siar needs *no change*:
//! `siar_identity_multidevice::RootPublicKey::verify`, device certificates
//! and root-rotation events keep working byte-for-byte. The root private key
//! simply never exists in one place — each participant holds a share, and
//! any `min` of `max` can sign.
//!
//! This module is a thin, checked layer over the upstream crate:
//!
//! * [`Threshold`] rejects degenerate settings (1-of-n is not a threshold;
//!   `min > max` is nonsense);
//! * the signature [`aggregate`] returns is **re-verified with
//!   `ed25519_dalek::VerifyingKey::verify_strict`** before it is handed out,
//!   so a bug or a malicious coordinator can never make us emit a signature
//!   the rest of the system would reject (or accept loosely);
//! * key material is exposed only as the 32-byte public key.
//!
//! Key generation is the upstream **distributed key generation** (no trusted
//! dealer): [`dkg_part1`] → [`dkg_part2`] → [`dkg_part3`].
//!
//! Not provided: transport of the round packages between participants (they
//! must travel over authenticated channels — the DKG's round-2 packages are
//! secret), and share storage (`KeyPackage::serialize` output must be
//! encrypted at rest by the caller).

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, VerifyingKey};
use frost_ed25519 as frost;

pub use frost::keys::dkg::{round1, round2};
pub use frost::keys::{KeyPackage, PublicKeyPackage};
pub use frost::round1::{SigningCommitments, SigningNonces};
pub use frost::round2::SignatureShare;
pub use frost::{Identifier, SigningPackage};

use crate::Error;

/// Hard cap on participants. FROST identifiers are `u16`, but a root-key
/// ceremony with hundreds of people is a mistake, not a feature.
pub const MAX_PARTICIPANTS: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threshold {
    min: u16,
    max: u16,
}

impl Threshold {
    pub fn new(min: u16, max: u16) -> Result<Self, Error> {
        if min < 2 {
            return Err(Error::InvalidParameter(
                "threshold must be at least 2 (1-of-n is a single key)",
            ));
        }
        if min > max {
            return Err(Error::InvalidParameter("threshold exceeds participants"));
        }
        if max > MAX_PARTICIPANTS {
            return Err(Error::InvalidParameter("too many participants"));
        }
        Ok(Self { min, max })
    }
    #[must_use]
    pub const fn min(self) -> u16 {
        self.min
    }
    #[must_use]
    pub const fn max(self) -> u16 {
        self.max
    }
}

/// A participant number, `1..=MAX_PARTICIPANTS`.
pub fn identifier(n: u16) -> Result<Identifier, Error> {
    if n == 0 || n > MAX_PARTICIPANTS {
        return Err(Error::InvalidParameter("participant number out of range"));
    }
    Identifier::try_from(n).map_err(|e| Error::Upstream(e.to_string()))
}

fn up<E: std::fmt::Display>(e: E) -> Error {
    Error::Upstream(e.to_string())
}

/// DKG round 1. Keep the secret; broadcast the package to every other
/// participant.
pub fn dkg_part1(
    me: Identifier,
    t: Threshold,
) -> Result<(round1::SecretPackage, round1::Package), Error> {
    let rng = rand_core::OsRng;
    frost::keys::dkg::part1(me, t.max, t.min, rng).map_err(up)
}

/// DKG round 2: consume everyone else's round-1 broadcasts. The returned
/// per-recipient packages are **secret** — send each only to its recipient,
/// over an authenticated, encrypted channel.
pub fn dkg_part2(
    secret: round1::SecretPackage,
    others: &BTreeMap<Identifier, round1::Package>,
) -> Result<(round2::SecretPackage, BTreeMap<Identifier, round2::Package>), Error> {
    frost::keys::dkg::part2(secret, others).map_err(up)
}

/// DKG round 3: derive this participant's long-term share and the group
/// public package. Every honest participant ends with the same
/// [`PublicKeyPackage`].
pub fn dkg_part3(
    secret: &round2::SecretPackage,
    round1_from_others: &BTreeMap<Identifier, round1::Package>,
    round2_for_me: &BTreeMap<Identifier, round2::Package>,
) -> Result<(KeyPackage, PublicKeyPackage), Error> {
    frost::keys::dkg::part3(secret, round1_from_others, round2_for_me).map_err(up)
}

/// The account root public key: the standard 32-byte Ed25519 key, usable as
/// `RootPublicKey(bytes)` unchanged.
pub fn root_public_key(p: &PublicKeyPackage) -> Result<[u8; 32], Error> {
    let bytes = p.verifying_key().serialize().map_err(up)?;
    bytes
        .try_into()
        .map_err(|_| Error::Malformed("group key is not 32 bytes"))
}

/// Signing round 1 (each signer): fresh nonces + the commitment to send to
/// the coordinator. **Nonces must be used for exactly one signature and then
/// dropped** — reuse leaks the share. They are consumed by value in
/// [`sign_round2`] for that reason.
pub fn sign_round1(key: &KeyPackage) -> (SigningNonces, SigningCommitments) {
    let mut rng = rand_core::OsRng;
    frost::round1::commit(key.signing_share(), &mut rng)
}

/// Coordinator: bind the message and the chosen signers' commitments.
pub fn signing_package(
    commitments: BTreeMap<Identifier, SigningCommitments>,
    message: &[u8],
) -> SigningPackage {
    SigningPackage::new(commitments, message)
}

/// Signing round 2 (each signer). Consumes the nonces.
pub fn sign_round2(
    package: &SigningPackage,
    nonces: SigningNonces,
    key: &KeyPackage,
) -> Result<SignatureShare, Error> {
    frost::round2::sign(package, &nonces, key).map_err(up)
}

/// Coordinator: combine shares into a standard 64-byte Ed25519 signature,
/// then **verify it strictly before returning it**.
pub fn aggregate(
    package: &SigningPackage,
    shares: &BTreeMap<Identifier, SignatureShare>,
    public: &PublicKeyPackage,
) -> Result<[u8; 64], Error> {
    let sig = frost::aggregate(package, shares, public).map_err(up)?;
    let bytes: [u8; 64] = sig
        .serialize()
        .map_err(up)?
        .try_into()
        .map_err(|_| Error::Malformed("signature is not 64 bytes"))?;
    verify_ed25519_strict(&root_public_key(public)?, package.message(), &bytes)?;
    Ok(bytes)
}

/// The exact check a consumer would do, but strict (rejects small-order and
/// non-canonical encodings).
pub fn verify_ed25519_strict(
    public_key: &[u8; 32],
    message: &[u8],
    signature: &[u8; 64],
) -> Result<(), Error> {
    let vk = VerifyingKey::from_bytes(public_key).map_err(|_| Error::Malformed("public key"))?;
    vk.verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| Error::VerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_identity_multidevice::root_key::RootPublicKey;

    struct Group {
        keys: BTreeMap<Identifier, KeyPackage>,
        public: PublicKeyPackage,
    }

    /// Run the real 3-round DKG between `t.max()` in-process participants.
    fn run_dkg(t: Threshold) -> Group {
        let ids: Vec<Identifier> = (1..=t.max()).map(|n| identifier(n).unwrap()).collect();
        let mut r1_secret = BTreeMap::new();
        let mut r1_pub = BTreeMap::new();
        for id in &ids {
            let (s, p) = dkg_part1(*id, t).unwrap();
            r1_secret.insert(*id, s);
            r1_pub.insert(*id, p);
        }
        let mut r2_secret = BTreeMap::new();
        let mut r2_inbox: BTreeMap<Identifier, BTreeMap<Identifier, round2::Package>> =
            BTreeMap::new();
        for id in &ids {
            let others: BTreeMap<_, _> = r1_pub
                .iter()
                .filter(|(k, _)| *k != id)
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            let (s, out) = dkg_part2(r1_secret.remove(id).unwrap(), &others).unwrap();
            r2_secret.insert(*id, s);
            for (to, pkg) in out {
                r2_inbox.entry(to).or_default().insert(*id, pkg);
            }
        }
        let mut keys = BTreeMap::new();
        let mut public = None;
        for id in &ids {
            let others: BTreeMap<_, _> = r1_pub
                .iter()
                .filter(|(k, _)| *k != id)
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            let (kp, pp) = dkg_part3(&r2_secret[id], &others, &r2_inbox[id]).unwrap();
            if let Some(prev) = &public {
                assert_eq!(
                    root_public_key(prev).unwrap(),
                    root_public_key(&pp).unwrap(),
                    "every participant must derive the same group key"
                );
            }
            public = Some(pp);
            keys.insert(*id, kp);
        }
        Group {
            keys,
            public: public.unwrap(),
        }
    }

    fn sign_with(g: &Group, signers: &[u16], msg: &[u8]) -> Result<[u8; 64], Error> {
        let mut nonces = BTreeMap::new();
        let mut commitments = BTreeMap::new();
        for n in signers {
            let id = identifier(*n).unwrap();
            let (nn, cc) = sign_round1(&g.keys[&id]);
            nonces.insert(id, nn);
            commitments.insert(id, cc);
        }
        let pkg = signing_package(commitments, msg);
        let mut shares = BTreeMap::new();
        for n in signers {
            let id = identifier(*n).unwrap();
            shares.insert(
                id,
                sign_round2(&pkg, nonces.remove(&id).unwrap(), &g.keys[&id])?,
            );
        }
        aggregate(&pkg, &shares, &g.public)
    }

    #[test]
    fn threshold_rejects_degenerate_settings() {
        assert!(Threshold::new(1, 3).is_err());
        assert!(Threshold::new(0, 0).is_err());
        assert!(Threshold::new(4, 3).is_err());
        assert!(Threshold::new(2, MAX_PARTICIPANTS + 1).is_err());
        assert!(Threshold::new(2, 3).is_ok());
        assert!(identifier(0).is_err());
        assert!(identifier(MAX_PARTICIPANTS + 1).is_err());
    }

    #[test]
    fn dkg_then_any_two_of_three_sign_a_standard_ed25519_signature() {
        let g = run_dkg(Threshold::new(2, 3).unwrap());
        let pk = root_public_key(&g.public).unwrap();
        let msg = b"siar root-signed payload";
        // Every 2-subset works…
        for pair in [[1u16, 2], [1, 3], [2, 3]] {
            let sig = sign_with(&g, &pair, msg).unwrap();
            // …and the result is checked by the *workspace's own* root verifier.
            RootPublicKey(pk).verify(msg, &sig).unwrap();
            verify_ed25519_strict(&pk, msg, &sig).unwrap();
            // Bound to the message.
            assert!(RootPublicKey(pk).verify(b"other", &sig).is_err());
        }
        // All three also work (≥ threshold).
        let sig = sign_with(&g, &[1, 2, 3], msg).unwrap();
        RootPublicKey(pk).verify(msg, &sig).unwrap();
    }

    #[test]
    fn one_share_alone_cannot_sign() {
        let g = run_dkg(Threshold::new(2, 3).unwrap());
        assert!(sign_with(&g, &[1], b"m").is_err());
    }

    #[test]
    fn a_tampered_share_is_rejected_not_silently_aggregated() {
        let g = run_dkg(Threshold::new(2, 3).unwrap());
        let msg = b"m";
        let mut nonces = BTreeMap::new();
        let mut commitments = BTreeMap::new();
        for n in [1u16, 2] {
            let id = identifier(n).unwrap();
            let (nn, cc) = sign_round1(&g.keys[&id]);
            nonces.insert(id, nn);
            commitments.insert(id, cc);
        }
        let pkg = signing_package(commitments, msg);
        let (i1, i2) = (identifier(1).unwrap(), identifier(2).unwrap());
        let s1 = sign_round2(&pkg, nonces.remove(&i1).unwrap(), &g.keys[&i1]).unwrap();
        // Signer 2 signs a DIFFERENT message under the same commitments.
        let evil_pkg = signing_package(pkg.signing_commitments().clone(), b"evil");
        let s2 = sign_round2(&evil_pkg, nonces.remove(&i2).unwrap(), &g.keys[&i2]).unwrap();
        let shares = BTreeMap::from([(i1, s1), (i2, s2)]);
        assert!(aggregate(&pkg, &shares, &g.public).is_err());
    }

    #[test]
    fn a_frost_group_can_be_the_new_root_in_a_real_rotation() {
        use siar_domain::AccountId;
        use siar_identity_multidevice::root_key::RootIdentityKey;
        use siar_identity_multidevice::root_rotation::{
            rotation_signing_payload, verify_root_rotation, RootRotation,
        };

        let g = run_dkg(Threshold::new(2, 3).unwrap());
        let new_root = RootPublicKey(root_public_key(&g.public).unwrap());
        let old = RootIdentityKey::generate();
        let old_pub = old.root_public_key();
        let account = AccountId::new();
        let payload = rotation_signing_payload(account, &old_pub, &new_root, 1);
        let rotation = RootRotation {
            account_id: account,
            old_root: old_pub,
            new_root,
            generation: 1,
            old_signature: old.sign(&payload).to_vec(),
            // The NEW root's acceptance is signed by 2 of 3 FROST shares.
            new_signature: sign_with(&g, &[1, 3], &payload).unwrap().to_vec(),
        };
        verify_root_rotation(&rotation).unwrap();
    }
}
