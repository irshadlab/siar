//! Anonymous **relay passes**: a credential a mesh node can show to a relay
//! to prove "I hold a valid pass of at least this tier for this epoch"
//! without revealing *which* pass — so two presentations (to the same relay
//! or different ones) cannot be linked to each other or to the holder.
//!
//! Primitive: **BBS** signatures with selective-disclosure proofs, via
//! `zkryptium` (IETF `draft-irtf-cfrg-bbs-signatures`, ciphersuite
//! `BLS12-381-SHA-256`). This module implements **no** BBS math; it fixes the
//! credential's shape and the rules around presenting it:
//!
//! ```text
//! attributes:  [0] pass_id (16 random bytes — ALWAYS hidden)
//!              [1] epoch   (u64 BE — disclosed)
//!              [2] tier    (u8     — disclosed)
//! header:      "siar-relay-pass-v1"           (signed in; binds the format)
//! presentation header = "siar-relay-ph-v1" ‖ relay_id ‖ nonce
//! ```
//!
//! The relay chooses a fresh random `nonce` per attempt ([`NonceBook`]) and
//! the proof is bound to *that relay and that nonce*, so a captured proof
//! cannot be replayed to another relay or later to the same one, and each
//! nonce is consumed on first use.
//!
//! **What this is not — read before relying on it:**
//!
//! * It is the *credential layer* only. No mixnet exists in this workspace;
//!   this gives a mixnet's relays something to check, nothing more.
//! * BBS alone does **not** stop one pass being shown many times (unlinkable
//!   means exactly that). Rate-limiting needs a per-relay pseudonym
//!   (`zkryptium` offers BBS per-verifier linkability) — **not wired here**.
//! * `zkryptium` (on `bls12_381_plus`) has **no published audit** and draws
//!   proof randomness from `rand`'s OS-seeded `ThreadRng`. Treat as
//!   experimental until audited.

use std::collections::HashMap;

use zkryptium::keys::pair::KeyPair;
use zkryptium::schemes::algorithms::{BBSplus, BbsBls12381Sha256};
use zkryptium::schemes::generics::{PoKSignature, Signature};

use crate::{fill_random, Error};

type Scheme = BBSplus<<BbsBls12381Sha256 as zkryptium::schemes::algorithms::Scheme>::Ciphersuite>;

const HEADER: &[u8] = b"siar-relay-pass-v1";
const PH_TAG: &[u8] = b"siar-relay-ph-v1";
const IKM_LEN: usize = 32;
const DISCLOSED: [usize; 2] = [1, 2];
const ATTRS: usize = 3;

/// Exact byte length of a proof for this credential shape: three G1 points
/// (48 B each) and three scalars (32 B each) of fixed part = 240 B, then one
/// 32 B response per *hidden* attribute (one: the pass id) and the 32 B
/// challenge.
///
/// Enforced *before* the bytes reach `zkryptium`.
/// `zkryptium` 0.8.1's upstream `PoKSignature::from_bytes` sliced its input
/// without a length check (panicking on < 240 bytes and ignoring trailing bytes);
/// this is fixed via workspace patch (`[patch.crates-io]`), and this exact check
/// is retained here as defence in depth for the relay-pass schema. A relay parses
/// proofs from strangers, so neither is acceptable.
pub const PROOF_LEN: usize = 240 + 32 * (ATTRS - DISCLOSED.len()) + 32;

/// Compressed G2 public key.
pub const ISSUER_PK_LEN: usize = 96;
/// Longest a nonce stays redeemable.
pub const NONCE_TTL_MS: u64 = 60_000;
/// Outstanding challenges a relay will track (bounds memory under abuse).
pub const MAX_OUTSTANDING: usize = 4096;

fn up<E: std::fmt::Debug>(e: E) -> Error {
    Error::Upstream(format!("{e:?}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct IssuerPublicKey(pub [u8; ISSUER_PK_LEN]);

impl std::fmt::Debug for IssuerPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IssuerPublicKey(..)")
    }
}

pub struct PassIssuer {
    pair: KeyPair<Scheme>,
}

impl PassIssuer {
    pub fn generate() -> Result<Self, Error> {
        let mut ikm = [0u8; IKM_LEN];
        fill_random(&mut ikm)?;
        let pair = KeyPair::<Scheme>::generate(&ikm, None, None).map_err(up);
        zeroize::Zeroize::zeroize(&mut ikm);
        Ok(Self { pair: pair? })
    }

    #[must_use]
    pub fn public_key(&self) -> IssuerPublicKey {
        IssuerPublicKey(self.pair.public_key().to_bytes())
    }

    /// Issue a pass for `epoch` at `tier` with a fresh random hidden id.
    pub fn issue(&self, epoch: u64, tier: u8) -> Result<RelayPass, Error> {
        let mut id = [0u8; 16];
        fill_random(&mut id)?;
        let attrs = vec![id.to_vec(), epoch.to_be_bytes().to_vec(), vec![tier]];
        let sig = Signature::<Scheme>::sign(
            Some(&attrs),
            self.pair.private_key(),
            self.pair.public_key(),
            Some(HEADER),
        )
        .map_err(up)?;
        Ok(RelayPass {
            attrs,
            signature: sig.to_bytes().to_vec(),
        })
    }
}

/// A relay's challenge. `nonce` is single-use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Challenge {
    pub relay_id: [u8; 32],
    pub nonce: [u8; 32],
}

impl Challenge {
    fn presentation_header(&self) -> Vec<u8> {
        let mut ph = Vec::with_capacity(PH_TAG.len() + 64);
        ph.extend_from_slice(PH_TAG);
        ph.extend_from_slice(&self.relay_id);
        ph.extend_from_slice(&self.nonce);
        ph
    }
}

/// The holder's secret credential. Never sent anywhere.
pub struct RelayPass {
    attrs: Vec<Vec<u8>>,
    signature: Vec<u8>,
}

impl RelayPass {
    /// Prove possession, revealing only `(epoch, tier)`, bound to `c`.
    pub fn present(&self, issuer: &IssuerPublicKey, c: &Challenge) -> Result<Presentation, Error> {
        let pk = zkryptium::bbsplus::keys::BBSplusPublicKey::from_bytes(&issuer.0)
            .map_err(|_| Error::Malformed("issuer key"))?;
        let ph = c.presentation_header();
        let proof = PoKSignature::<Scheme>::proof_gen(
            &pk,
            &self.signature,
            Some(HEADER),
            Some(&ph),
            Some(&self.attrs),
            Some(&DISCLOSED),
        )
        .map_err(up)?;
        Ok(Presentation {
            proof: proof.to_bytes(),
            epoch: u64::from_be_bytes(
                self.attrs[1]
                    .as_slice()
                    .try_into()
                    .map_err(|_| Error::Malformed("epoch"))?,
            ),
            tier: self.attrs[2][0],
        })
    }
}

/// What travels to the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    pub proof: Vec<u8>,
    pub epoch: u64,
    pub tier: u8,
}

/// Outstanding challenges, bounded and expiring.
#[derive(Default)]
pub struct NonceBook {
    outstanding: HashMap<[u8; 32], u64>,
}

impl NonceBook {
    pub fn issue(&mut self, relay_id: [u8; 32], now_ms: u64) -> Result<Challenge, Error> {
        self.outstanding.retain(|_, exp| *exp > now_ms);
        if self.outstanding.len() >= MAX_OUTSTANDING {
            return Err(Error::InvalidParameter("too many outstanding challenges"));
        }
        let mut nonce = [0u8; 32];
        fill_random(&mut nonce)?;
        self.outstanding
            .insert(nonce, now_ms.saturating_add(NONCE_TTL_MS));
        Ok(Challenge { relay_id, nonce })
    }

    /// Redeem exactly once, within the TTL.
    fn consume(&mut self, nonce: &[u8; 32], now_ms: u64) -> bool {
        matches!(self.outstanding.remove(nonce), Some(exp) if exp > now_ms)
    }

    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
}

/// Relay-side policy.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub min_tier: u8,
    /// Accept epochs in `[current - tolerance, current]`.
    pub epoch_tolerance: u64,
}

/// What a relay learns from an accepted pass: only these two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub epoch: u64,
    pub tier: u8,
}

pub struct RelayVerifier {
    issuer: IssuerPublicKey,
    relay_id: [u8; 32],
    nonces: NonceBook,
}

impl RelayVerifier {
    #[must_use]
    pub fn new(issuer: IssuerPublicKey, relay_id: [u8; 32]) -> Self {
        Self {
            issuer,
            relay_id,
            nonces: NonceBook::default(),
        }
    }

    pub fn challenge(&mut self, now_ms: u64) -> Result<Challenge, Error> {
        self.nonces.issue(self.relay_id, now_ms)
    }

    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.nonces.outstanding()
    }

    /// Verify a presentation against a challenge **this relay issued**.
    /// The nonce is consumed whether or not verification succeeds, so a
    /// failed attempt cannot be retried against the same challenge.
    pub fn verify(
        &mut self,
        nonce: &[u8; 32],
        p: &Presentation,
        now_ms: u64,
        current_epoch: u64,
        policy: Policy,
    ) -> Result<Accepted, Error> {
        if !self.nonces.consume(nonce, now_ms) {
            return Err(Error::VerificationFailed);
        }
        if p.tier < policy.min_tier
            || p.epoch > current_epoch
            || p.epoch < current_epoch.saturating_sub(policy.epoch_tolerance)
        {
            return Err(Error::VerificationFailed);
        }
        let pk = zkryptium::bbsplus::keys::BBSplusPublicKey::from_bytes(&self.issuer.0)
            .map_err(|_| Error::Malformed("issuer key"))?;
        let c = Challenge {
            relay_id: self.relay_id,
            nonce: *nonce,
        };
        let ph = c.presentation_header();
        if p.proof.len() != PROOF_LEN {
            return Err(Error::Malformed("proof length"));
        }
        let disclosed = vec![p.epoch.to_be_bytes().to_vec(), vec![p.tier]];
        // Defence in depth at an untrusted boundary: a panic inside the
        // upstream parser/verifier must be a rejection, not a remote crash.
        let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let proof = PoKSignature::<Scheme>::from_bytes(&p.proof).ok()?;
            proof
                .proof_verify(
                    &pk,
                    Some(&disclosed),
                    Some(&DISCLOSED),
                    Some(HEADER),
                    Some(&ph),
                )
                .ok()
        }));
        if !matches!(verdict, Ok(Some(()))) {
            return Err(Error::VerificationFailed);
        }
        Ok(Accepted {
            epoch: p.epoch,
            tier: p.tier,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: Policy = Policy {
        min_tier: 1,
        epoch_tolerance: 1,
    };

    fn setup() -> (PassIssuer, RelayVerifier) {
        let issuer = PassIssuer::generate().unwrap();
        let relay = RelayVerifier::new(issuer.public_key(), [7; 32]);
        (issuer, relay)
    }

    #[test]
    fn honest_pass_is_accepted_and_reveals_only_epoch_and_tier() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(100, 2).unwrap();
        let c = relay.challenge(1_000).unwrap();
        let p = pass.present(&issuer.public_key(), &c).unwrap();
        assert_eq!(
            relay.verify(&c.nonce, &p, 1_001, 100, POLICY).unwrap(),
            Accepted {
                epoch: 100,
                tier: 2
            }
        );
        // The hidden pass id appears nowhere in what was sent.
        let id = &pass.attrs[0];
        assert!(!p.proof.windows(id.len()).any(|w| w == id.as_slice()));
    }

    #[test]
    fn two_presentations_of_one_pass_share_no_bytes_worth_linking() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(5, 1).unwrap();
        let c1 = relay.challenge(1).unwrap();
        let c2 = relay.challenge(1).unwrap();
        let p1 = pass.present(&issuer.public_key(), &c1).unwrap();
        let p2 = pass.present(&issuer.public_key(), &c2).unwrap();
        assert_ne!(p1.proof, p2.proof);
        // No 16-byte run in common (a stable pseudonym/signature would show up).
        assert!(!p1
            .proof
            .windows(16)
            .any(|w| p2.proof.windows(16).any(|v| v == w)));
        relay.verify(&c1.nonce, &p1, 2, 5, POLICY).unwrap();
        relay.verify(&c2.nonce, &p2, 2, 5, POLICY).unwrap();
    }

    #[test]
    fn a_nonce_is_single_use_so_replay_fails() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(5, 1).unwrap();
        let c = relay.challenge(1).unwrap();
        let p = pass.present(&issuer.public_key(), &c).unwrap();
        relay.verify(&c.nonce, &p, 2, 5, POLICY).unwrap();
        assert!(relay.verify(&c.nonce, &p, 3, 5, POLICY).is_err());
    }

    #[test]
    fn a_proof_made_for_one_relay_is_useless_at_another() {
        let issuer = PassIssuer::generate().unwrap();
        let mut a = RelayVerifier::new(issuer.public_key(), [1; 32]);
        let mut b = RelayVerifier::new(issuer.public_key(), [2; 32]);
        let pass = issuer.issue(5, 1).unwrap();
        let ca = a.challenge(1).unwrap();
        let p = pass.present(&issuer.public_key(), &ca).unwrap();
        // Attacker (relay A, or a wiretap) forwards it to B under B's own nonce.
        let cb = b.challenge(1).unwrap();
        assert!(b.verify(&cb.nonce, &p, 2, 5, POLICY).is_err());
        // And it still works at A.
        a.verify(&ca.nonce, &p, 2, 5, POLICY).unwrap();
    }

    #[test]
    fn unknown_expired_and_failed_nonces_are_refused() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(5, 1).unwrap();
        let c = relay.challenge(1_000).unwrap();
        let p = pass.present(&issuer.public_key(), &c).unwrap();
        // Never issued.
        assert!(relay.verify(&[9; 32], &p, 1_001, 5, POLICY).is_err());
        // Expired.
        assert!(relay
            .verify(&c.nonce, &p, 1_000 + NONCE_TTL_MS, 5, POLICY)
            .is_err());
        // A failed attempt burns the nonce: the same challenge cannot be retried.
        let c2 = relay.challenge(2_000).unwrap();
        let mut bad = pass.present(&issuer.public_key(), &c2).unwrap();
        bad.proof[10] ^= 1;
        assert!(relay.verify(&c2.nonce, &bad, 2_001, 5, POLICY).is_err());
        let good = pass.present(&issuer.public_key(), &c2).unwrap();
        assert!(relay.verify(&c2.nonce, &good, 2_002, 5, POLICY).is_err());
    }

    #[test]
    fn disclosed_values_cannot_be_upgraded_after_the_fact() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(5, 1).unwrap(); // tier 1
        let c = relay.challenge(1).unwrap();
        let mut p = pass.present(&issuer.public_key(), &c).unwrap();
        p.tier = 9; // claim a higher tier than was signed
        let policy = Policy {
            min_tier: 5,
            epoch_tolerance: 0,
        };
        assert!(relay.verify(&c.nonce, &p, 2, 5, policy).is_err());

        let c = relay.challenge(1).unwrap();
        let mut p = pass.present(&issuer.public_key(), &c).unwrap();
        p.epoch = 6; // claim a newer epoch
        assert!(relay.verify(&c.nonce, &p, 2, 6, POLICY).is_err());
    }

    #[test]
    fn policy_checks_tier_and_epoch_window() {
        let (issuer, mut relay) = setup();
        let try_one = |relay: &mut RelayVerifier, epoch, tier, now_epoch, policy| {
            let pass = issuer.issue(epoch, tier).unwrap();
            let c = relay.challenge(1).unwrap();
            let p = pass.present(&issuer.public_key(), &c).unwrap();
            relay.verify(&c.nonce, &p, 2, now_epoch, policy).is_ok()
        };
        assert!(try_one(&mut relay, 10, 3, 10, POLICY));
        assert!(try_one(&mut relay, 9, 3, 10, POLICY), "within tolerance");
        assert!(!try_one(&mut relay, 8, 3, 10, POLICY), "too old");
        assert!(!try_one(&mut relay, 11, 3, 10, POLICY), "from the future");
        assert!(!try_one(&mut relay, 10, 0, 10, POLICY), "below min tier");
    }

    #[test]
    fn a_different_issuers_pass_is_rejected() {
        let (_issuer, mut relay) = setup();
        let rogue = PassIssuer::generate().unwrap();
        let pass = rogue.issue(5, 9).unwrap();
        let c = relay.challenge(1).unwrap();
        let p = pass.present(&rogue.public_key(), &c).unwrap();
        assert!(relay.verify(&c.nonce, &p, 2, 5, POLICY).is_err());
    }

    #[test]
    fn every_proof_length_is_handled_without_a_panic_and_only_the_exact_one_can_pass() {
        let (issuer, mut relay) = setup();
        let pass = issuer.issue(5, 1).unwrap();
        let c = relay.challenge(1).unwrap();
        let good = pass.present(&issuer.public_key(), &c).unwrap();
        assert_eq!(good.proof.len(), PROOF_LEN, "PROOF_LEN matches reality");

        // Truncations 0..PROOF_LEN, and extensions — upstream's own parser
        // panics on short input; ours must reject all of them.
        for len in (0..=PROOF_LEN + 40).filter(|l| *l != PROOF_LEN) {
            let c = relay.challenge(1).unwrap();
            let mut bytes = good.proof.clone();
            bytes.resize(len, 0xAA);
            let p = Presentation {
                proof: bytes,
                ..good.clone()
            };
            assert!(
                relay.verify(&c.nonce, &p, 2, 5, POLICY).is_err(),
                "len {len}"
            );
        }
        // Trailing garbage on an otherwise valid proof is not tolerated.
        let c = relay.challenge(1).unwrap();
        let mut extended = good.proof.clone();
        extended.push(0);
        let p = Presentation {
            proof: extended,
            ..good.clone()
        };
        assert!(relay.verify(&c.nonce, &p, 2, 5, POLICY).is_err());
    }

    #[test]
    fn challenges_are_bounded_and_expired_ones_are_reclaimed() {
        let (_i, mut relay) = setup();
        for _ in 0..MAX_OUTSTANDING {
            relay.challenge(0).unwrap();
        }
        assert!(relay.challenge(1).is_err(), "memory cannot be exhausted");
        // After the TTL they are swept and issuing works again.
        relay.challenge(NONCE_TTL_MS + 1).unwrap();
        assert_eq!(relay.outstanding(), 1);
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        let (issuer, mut relay) = setup();
        let c = relay.challenge(1).unwrap();
        let junk = Presentation {
            proof: vec![1, 2, 3],
            epoch: 5,
            tier: 1,
        };
        assert!(relay.verify(&c.nonce, &junk, 2, 5, POLICY).is_err());
        let pass = issuer.issue(5, 1).unwrap();
        let bad_pk = IssuerPublicKey([0xff; ISSUER_PK_LEN]);
        let c = relay.challenge(1).unwrap();
        assert!(pass.present(&bad_pk, &c).is_err());
    }
}
