//! BLS12-381 aggregate attestations over an **audit-log head**, for large
//! groups.
//!
//! In a group of N members, "N people each checked the audit log and agree
//! its head at epoch E is H" would cost N × 96 bytes as separate signatures.
//! With BLS the same statement is **one 96-byte signature plus an N-bit
//! bitmap**, verified with one pairing check no matter how many signed.
//!
//! Primitive: `blst` (supranational), minimal-pubkey-size variant, the
//! IETF BLS `POP` ciphersuite
//! (`BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_`) — the same one Ethereum
//! uses. This module adds only the rules that make aggregation *safe*:
//!
//! * **Proof of possession on registration.** Same-message aggregation
//!   (`fast_aggregate_verify`) is only sound if no member can choose their
//!   key as a function of another's (the *rogue-key attack*). [`Roster`]
//!   therefore admits a key only together with a valid proof of
//!   possession (a signature over the key's own bytes, under a separate
//!   domain tag), so a [`Roster`] simply cannot contain an unproven key.
//! * **Subgroup/infinity checks** on every key and signature.
//! * **No double counting:** one slot per member, duplicates rejected both
//!   when collecting and when decoding a bitmap.
//! * **Canonical bitmap:** exact length, no stray bits past the roster size.
//! * **Threshold is the verifier's choice,** never read from the attestation.
//! * A signed [`AuditHead`] carries its own domain tag, group id, epoch and
//!   head hash, so an attestation for one group/epoch cannot be replayed for
//!   another.

use std::collections::BTreeMap;

use blst::min_pk::{AggregateSignature, PublicKey, SecretKey, Signature};
use blst::BLST_ERROR;

use crate::{fill_random, Error};

const DST_SIG: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
const DST_POP: &[u8] = b"BLS_POP_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
const HEAD_TAG: &[u8] = b"siar-audit-head-v1";

pub const PK_LEN: usize = 48;
pub const SIG_LEN: usize = 96;
/// Upper bound on roster size (bitmap ≤ 12.5 KB, verification linear in
/// signers).
pub const MAX_ROSTER: usize = 100_000;

fn bls(e: BLST_ERROR) -> Error {
    Error::Upstream(format!("{e:?}"))
}

/// What members attest to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditHead {
    pub group: [u8; 32],
    pub epoch: u64,
    pub log_head: [u8; 32],
}

impl AuditHead {
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut m = Vec::with_capacity(HEAD_TAG.len() + 72);
        m.extend_from_slice(HEAD_TAG);
        m.extend_from_slice(&self.group);
        m.extend_from_slice(&self.epoch.to_be_bytes());
        m.extend_from_slice(&self.log_head);
        m
    }
}

pub struct AuditKey {
    sk: SecretKey,
}

impl AuditKey {
    pub fn generate() -> Result<Self, Error> {
        let mut ikm = [0u8; 32];
        fill_random(&mut ikm)?;
        let sk = SecretKey::key_gen(&ikm, &[]).map_err(bls);
        zeroize::Zeroize::zeroize(&mut ikm);
        Ok(Self { sk: sk? })
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; PK_LEN] {
        self.sk.sk_to_pk().to_bytes()
    }

    /// Proof of possession: a signature over this key's own public bytes
    /// under the POP domain tag.
    #[must_use]
    pub fn proof_of_possession(&self) -> [u8; SIG_LEN] {
        self.sk.sign(&self.public_key(), DST_POP, &[]).to_bytes()
    }

    #[must_use]
    pub fn sign_head(&self, head: &AuditHead) -> [u8; SIG_LEN] {
        self.sk.sign(&head.signing_bytes(), DST_SIG, &[]).to_bytes()
    }
}

impl Drop for AuditKey {
    fn drop(&mut self) {
        // blst's SecretKey is plain data; overwrite our copy's bytes.
        let mut b = self.sk.to_bytes();
        zeroize::Zeroize::zeroize(&mut b);
    }
}

/// An ordered set of members whose keys have all proven possession. A
/// member's *index* is their slot in attestation bitmaps.
#[derive(Default)]
pub struct Roster {
    keys: Vec<PublicKey>,
    seen: std::collections::BTreeSet<[u8; PK_LEN]>,
}

impl Roster {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Admit a member. Returns their index. Rejects: malformed / off-curve /
    /// wrong-subgroup / infinity keys, an invalid proof of possession, and a
    /// key that is already registered.
    pub fn register(&mut self, pk: &[u8; PK_LEN], pop: &[u8; SIG_LEN]) -> Result<usize, Error> {
        if self.keys.len() >= MAX_ROSTER {
            return Err(Error::InvalidParameter("roster full"));
        }
        let key = PublicKey::key_validate(pk).map_err(|_| Error::Malformed("public key"))?;
        let pop_sig = Signature::sig_validate(pop, true)
            .map_err(|_| Error::Malformed("proof of possession"))?;
        if pop_sig.verify(true, pk, DST_POP, &[], &key, false) != BLST_ERROR::BLST_SUCCESS {
            return Err(Error::VerificationFailed);
        }
        if !self.seen.insert(*pk) {
            return Err(Error::InvalidParameter("key already registered"));
        }
        self.keys.push(key);
        Ok(self.keys.len() - 1)
    }

    /// Check a single member's signature (used while collecting, so one bad
    /// contribution cannot poison an aggregate).
    pub fn verify_one(
        &self,
        index: usize,
        head: &AuditHead,
        sig: &[u8; SIG_LEN],
    ) -> Result<(), Error> {
        let key = self
            .keys
            .get(index)
            .ok_or(Error::InvalidParameter("no such member"))?;
        let s = Signature::sig_validate(sig, true).map_err(|_| Error::Malformed("signature"))?;
        match s.verify(true, &head.signing_bytes(), DST_SIG, &[], key, false) {
            BLST_ERROR::BLST_SUCCESS => Ok(()),
            _ => Err(Error::VerificationFailed),
        }
    }

    /// Verify an aggregate attestation. `min_signers` is chosen by the
    /// **verifier** (a policy: majority, two-thirds…), and must be ≥ 1.
    /// Returns the number of distinct members who signed.
    pub fn verify(
        &self,
        head: &AuditHead,
        att: &Attestation,
        min_signers: usize,
    ) -> Result<usize, Error> {
        if min_signers == 0 {
            return Err(Error::InvalidParameter("min_signers must be at least 1"));
        }
        let signers = att.signer_indices(self.keys.len())?;
        if signers.len() < min_signers {
            return Err(Error::VerificationFailed);
        }
        let pks: Vec<&PublicKey> = signers.iter().map(|i| &self.keys[*i]).collect();
        let sig = Signature::sig_validate(&att.signature, true)
            .map_err(|_| Error::Malformed("aggregate signature"))?;
        match sig.fast_aggregate_verify(true, &head.signing_bytes(), DST_SIG, &pks) {
            BLST_ERROR::BLST_SUCCESS => Ok(signers.len()),
            _ => Err(Error::VerificationFailed),
        }
    }
}

/// One aggregate signature + who contributed (bitmap, LSB-first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub bitmap: Vec<u8>,
    pub signature: [u8; SIG_LEN],
}

impl Attestation {
    /// Decode the bitmap strictly: exact length for `roster_len`, no bits set
    /// beyond it.
    fn signer_indices(&self, roster_len: usize) -> Result<Vec<usize>, Error> {
        if self.bitmap.len() != roster_len.div_ceil(8) {
            return Err(Error::Malformed("bitmap length"));
        }
        let mut out = Vec::new();
        for (byte_i, byte) in self.bitmap.iter().enumerate() {
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    let idx = byte_i * 8 + bit;
                    if idx >= roster_len {
                        return Err(Error::Malformed("bit beyond roster"));
                    }
                    out.push(idx);
                }
            }
        }
        Ok(out)
    }
}

/// Gathers verified contributions for one [`AuditHead`].
pub struct Collector<'a> {
    roster: &'a Roster,
    head: AuditHead,
    got: BTreeMap<usize, Signature>,
}

impl<'a> Collector<'a> {
    #[must_use]
    pub fn new(roster: &'a Roster, head: AuditHead) -> Self {
        Self {
            roster,
            head,
            got: BTreeMap::new(),
        }
    }

    /// Verify and add one member's signature. A duplicate from the same
    /// member is an error, not a silent second count.
    pub fn add(&mut self, index: usize, sig: &[u8; SIG_LEN]) -> Result<(), Error> {
        if self.got.contains_key(&index) {
            return Err(Error::InvalidParameter("member already contributed"));
        }
        self.roster.verify_one(index, &self.head, sig)?;
        let s = Signature::from_bytes(sig).map_err(|_| Error::Malformed("signature"))?;
        self.got.insert(index, s);
        Ok(())
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.got.len()
    }

    pub fn finish(self) -> Result<Attestation, Error> {
        if self.got.is_empty() {
            return Err(Error::InvalidParameter("no contributions"));
        }
        let sigs: Vec<&Signature> = self.got.values().collect();
        let agg = AggregateSignature::aggregate(&sigs, false).map_err(bls)?;
        let mut bitmap = vec![0u8; self.roster.len().div_ceil(8)];
        for i in self.got.keys() {
            bitmap[i / 8] |= 1 << (i % 8);
        }
        Ok(Attestation {
            bitmap,
            signature: agg.to_signature().to_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(epoch: u64) -> AuditHead {
        AuditHead {
            group: [1; 32],
            epoch,
            log_head: [2; 32],
        }
    }

    fn group(n: usize) -> (Roster, Vec<AuditKey>) {
        let mut r = Roster::new();
        let mut ks = Vec::new();
        for i in 0..n {
            let k = AuditKey::generate().unwrap();
            assert_eq!(
                r.register(&k.public_key(), &k.proof_of_possession())
                    .unwrap(),
                i
            );
            ks.push(k);
        }
        (r, ks)
    }

    fn attest(r: &Roster, ks: &[AuditKey], who: &[usize], h: &AuditHead) -> Attestation {
        let mut c = Collector::new(r, *h);
        for i in who {
            c.add(*i, &ks[*i].sign_head(h)).unwrap();
        }
        c.finish().unwrap()
    }

    #[test]
    fn many_signers_collapse_to_one_signature_and_a_bitmap() {
        let (r, ks) = group(200);
        let h = head(7);
        let who: Vec<usize> = (0..200).filter(|i| i % 4 != 0).collect(); // 150
        let a = attest(&r, &ks, &who, &h);
        assert_eq!(a.signature.len(), SIG_LEN);
        assert_eq!(a.bitmap.len(), 25);
        assert_eq!(r.verify(&h, &a, 134).unwrap(), 150); // two-thirds of 200
    }

    #[test]
    fn threshold_is_the_verifiers_policy() {
        let (r, ks) = group(10);
        let h = head(1);
        let a = attest(&r, &ks, &[0, 1, 2, 3, 4], &h);
        assert_eq!(r.verify(&h, &a, 5).unwrap(), 5);
        assert!(r.verify(&h, &a, 6).is_err());
        assert!(r.verify(&h, &a, 0).is_err(), "0 would accept anything");
    }

    #[test]
    fn attestation_is_bound_to_group_epoch_and_head() {
        let (r, ks) = group(5);
        let h = head(1);
        let a = attest(&r, &ks, &[0, 1, 2], &h);
        r.verify(&h, &a, 3).unwrap();
        for other in [
            AuditHead { epoch: 2, ..h },
            AuditHead {
                group: [9; 32],
                ..h
            },
            AuditHead {
                log_head: [9; 32],
                ..h
            },
        ] {
            assert!(r.verify(&other, &a, 3).is_err());
        }
    }

    #[test]
    fn forged_bitmap_claims_do_not_verify() {
        let (r, ks) = group(8);
        let h = head(1);
        let a = attest(&r, &ks, &[0, 1, 2], &h);
        // Claim member 5 also signed.
        let mut b = a.clone();
        b.bitmap[0] |= 1 << 5;
        assert!(r.verify(&h, &b, 1).is_err());
        // Remove a real signer.
        let mut b = a.clone();
        b.bitmap[0] &= !(1 << 0);
        assert!(r.verify(&h, &b, 1).is_err());
    }

    #[test]
    fn bitmap_must_be_canonical() {
        let (r, ks) = group(10); // 2 bytes, bits 10..15 must be zero
        let h = head(1);
        let a = attest(&r, &ks, &[0, 1], &h);
        let mut stray = a.clone();
        stray.bitmap[1] |= 1 << 7;
        assert!(r.verify(&h, &stray, 1).is_err());
        let mut long = a.clone();
        long.bitmap.push(0);
        assert!(r.verify(&h, &long, 1).is_err());
        let mut short = a;
        short.bitmap.pop();
        assert!(r.verify(&h, &short, 1).is_err());
    }

    #[test]
    fn a_duplicate_contribution_is_rejected_not_double_counted() {
        let (r, ks) = group(4);
        let h = head(1);
        let mut c = Collector::new(&r, h);
        c.add(2, &ks[2].sign_head(&h)).unwrap();
        assert!(c.add(2, &ks[2].sign_head(&h)).is_err());
        assert_eq!(c.count(), 1);
    }

    #[test]
    fn a_bad_contribution_is_caught_at_collection_time() {
        let (r, ks) = group(4);
        let h = head(1);
        let mut c = Collector::new(&r, h);
        // Member 1's signature presented as member 0's.
        assert!(c.add(0, &ks[1].sign_head(&h)).is_err());
        // Signature over a different head.
        assert!(c.add(0, &ks[0].sign_head(&head(99))).is_err());
        assert!(c.add(99, &ks[0].sign_head(&h)).is_err(), "no such member");
        assert_eq!(c.count(), 0);
        assert!(c.finish().is_err(), "empty attestation refused");
    }

    #[test]
    fn registration_demands_a_valid_proof_of_possession() {
        let mut r = Roster::new();
        let a = AuditKey::generate().unwrap();
        let b = AuditKey::generate().unwrap();
        // Someone else's PoP.
        assert!(r
            .register(&a.public_key(), &b.proof_of_possession())
            .is_err());
        // A plain signature over the key under the *signature* domain is not a PoP.
        let wrong_domain = a.sk.sign(&a.public_key(), DST_SIG, &[]).to_bytes();
        assert!(r.register(&a.public_key(), &wrong_domain).is_err());
        // Garbage key / garbage PoP.
        assert!(r.register(&[0; PK_LEN], &a.proof_of_possession()).is_err());
        assert!(r.register(&a.public_key(), &[0; SIG_LEN]).is_err());
        assert!(r.is_empty(), "nothing was admitted");
        // The honest registration works once, and only once.
        r.register(&a.public_key(), &a.proof_of_possession())
            .unwrap();
        assert!(r
            .register(&a.public_key(), &a.proof_of_possession())
            .is_err());
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn the_point_at_infinity_is_not_a_key_or_a_signature() {
        // Compressed encoding of infinity in G1/G2: 0xc0 then zeros.
        let mut inf_pk = [0u8; PK_LEN];
        inf_pk[0] = 0xc0;
        let mut inf_sig = [0u8; SIG_LEN];
        inf_sig[0] = 0xc0;
        let mut r = Roster::new();
        assert!(r.register(&inf_pk, &inf_sig).is_err());

        let (r, _ks) = group(3);
        let a = Attestation {
            bitmap: vec![0b111],
            signature: inf_sig,
        };
        assert!(r.verify(&head(1), &a, 1).is_err());
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| self.next() as u8).collect()
        }
        fn mutate(&mut self, src: &[u8]) -> Vec<u8> {
            let mut v = src.to_vec();
            for _ in 0..=self.below(4) {
                match self.below(4) {
                    0 if !v.is_empty() => {
                        let i = self.below(v.len());
                        v[i] ^= 1 << self.below(8);
                    }
                    1 if !v.is_empty() => {
                        let i = self.below(v.len());
                        v[i] = self.next() as u8;
                    }
                    2 => {
                        let i = self.below(v.len() + 1);
                        v.insert(i, self.next() as u8);
                    }
                    _ if !v.is_empty() => {
                        let i = self.below(v.len());
                        v.remove(i);
                    }
                    _ => {}
                }
            }
            v
        }
    }

    /// Attestations arrive from group members (or anyone): random bitmaps and
    /// signatures, and mutations of a real one, must never panic and never
    /// verify as something they are not.
    #[test]
    fn hostile_attestations_never_panic_and_only_the_genuine_one_verifies() {
        let (r, ks) = group(12);
        let h = head(3);
        let genuine = attest(&r, &ks, &[0, 1, 2, 3, 4, 5, 6, 7], &h);
        assert_eq!(r.verify(&h, &genuine, 8).unwrap(), 8);
        let mut rng = Rng(0xB15_B175);
        for round in 0..400 {
            let att = match round % 3 {
                0 => Attestation {
                    bitmap: genuine
                        .bitmap
                        .iter()
                        .map(|b| b ^ ((1 << rng.below(8)) * u8::from(rng.below(2) == 0)))
                        .collect(),
                    signature: genuine.signature,
                },
                1 => {
                    let mut sig = genuine.signature;
                    let m = rng.mutate(&sig);
                    for (d, s) in sig.iter_mut().zip(m.iter()) {
                        *d = *s;
                    }
                    Attestation {
                        bitmap: genuine.bitmap.clone(),
                        signature: sig,
                    }
                }
                _ => {
                    let n = rng.below(6);
                    let mut sig = [0u8; SIG_LEN];
                    for b in &mut sig {
                        *b = rng.next() as u8;
                    }
                    Attestation {
                        bitmap: rng.bytes(n),
                        signature: sig,
                    }
                }
            };
            let res =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.verify(&h, &att, 1)));
            let res = res.unwrap_or_else(|_| panic!("verify PANICKED (round {round})"));
            if att == genuine {
                continue; // an unchanged mutation draw
            }
            // Every draw that differs from the genuine attestation is either a
            // bitmap that names the wrong signers or a signature that is not
            // their aggregate: neither may verify.
            assert!(
                res.is_err(),
                "a modified attestation VERIFIED (round {round})"
            );
        }
        // Registration inputs: random keys/PoPs are rejected, not crashed on.
        let mut roster = Roster::new();
        for _ in 0..200 {
            let mut pk = [0u8; PK_LEN];
            let mut pop = [0u8; SIG_LEN];
            for b in pk.iter_mut().chain(pop.iter_mut()) {
                *b = rng.next() as u8;
            }
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                roster.register(&pk, &pop)
            }));
            assert!(res.unwrap_or_else(|_| panic!("register PANICKED")).is_err());
        }
        assert!(roster.is_empty());
    }
}
