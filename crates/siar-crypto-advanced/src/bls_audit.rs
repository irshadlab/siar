//! BLS12-381 aggregate attestations over an **audit-log head**, for large
//! groups.
//!
//! In a group of N members, "N people each checked the audit log and agree
//! its head at epoch E is H" would cost N × 97 bytes as separate signatures.
//! With BLS the same statement is **one 97-byte signature plus an N-bit
//! bitmap**, verified with one pairing check no matter how many signed.
//!
//! Primitive: `blsful` (Michael Lodder / LIT Protocol; **audited by Kudelski
//! Security**, no significant findings) with its **pure-Rust** backend
//! (`bls12_381_plus`; the default `blst` backend, which wraps C, is switched
//! off — this workspace is pure Rust outside Slint). Signatures in G2, keys
//! in G1 (48-byte keys), the **proof-of-possession** scheme only. This module
//! adds only the rules that make aggregation *safe*:
//!
//! * **Proof of possession on registration.** Same-message aggregation is
//!   only sound if no member can choose their key as a function of another's
//!   (the *rogue-key attack*). [`Roster`] therefore admits a key only
//!   together with a valid proof of possession, so a [`Roster`] simply cannot
//!   contain an unproven key.
//! * **The point at infinity is rejected explicitly** — as a key, a proof, a
//!   signature or an aggregate. `blsful` (unlike `blst`'s `key_validate`)
//!   *accepts* an infinity public key on decode; left alone, an infinity key
//!   with an infinity signature verifies **any** message. Found by probing
//!   the library, guarded by a test.
//! * Only the proof-of-possession *scheme variant* is accepted on decode: a
//!   `Basic`-scheme signature uses a different domain tag and is not safe to
//!   aggregate, so it is refused even though it parses.
//! * **No double counting:** one slot per member, duplicates rejected both
//!   when collecting and when decoding a bitmap.
//! * **Canonical bitmap:** exact length, no stray bits past the roster size.
//! * **Threshold is the verifier's choice,** never read from the attestation.
//! * A signed [`AuditHead`] carries its own domain tag, group id, epoch and
//!   head hash, so an attestation for one group/epoch cannot be replayed for
//!   another.
//!
//! Limits: `blsful`'s `SecretKey` is not zeroized on drop by upstream, so the
//! key scalar may linger in freed memory; and decoding relies on upstream for
//! the subgroup check (it rejects junk encodings; a crafted on-curve
//! non-subgroup point is not constructed in our tests).

use std::collections::BTreeMap;

use blsful::{
    Bls12381G2Impl, MultiPublicKey, MultiSignature, ProofOfPossession, PublicKey, SecretKey,
    Signature, SignatureSchemes,
};

use crate::Error;

type C = Bls12381G2Impl;

const HEAD_TAG: &[u8] = b"siar-audit-head-v1";

/// Compressed G1 public key.
pub const PK_LEN: usize = 48;
/// A tagged proof-of-possession signature: 1 scheme byte + a compressed G2 point.
pub const SIG_LEN: usize = 97;
/// A proof of possession: a compressed G2 point.
pub const POP_LEN: usize = 96;
/// Upper bound on roster size (bitmap ≤ 12.5 KB, verification linear in
/// signers).
pub const MAX_ROSTER: usize = 100_000;

/// The "point at infinity" flag in a compressed BLS12-381 encoding.
const INFINITY_FLAG: u8 = 0x40;

fn up<E: std::fmt::Debug>(e: E) -> Error {
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
    sk: SecretKey<C>,
}

fn to_array<const N: usize>(v: Vec<u8>) -> Result<[u8; N], Error> {
    v.try_into()
        .map_err(|_| Error::Malformed("unexpected encoded length"))
}

impl AuditKey {
    pub fn generate() -> Result<Self, Error> {
        Ok(Self {
            sk: SecretKey::<C>::new(),
        })
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; PK_LEN] {
        let v: Vec<u8> = (&self.sk.public_key()).into();
        // A fixed-size encoding by construction; an empty fallback would be
        // rejected by `register`.
        to_array(v).unwrap_or([0; PK_LEN])
    }

    /// Proof of possession (the scheme's own, separately domain-tagged).
    #[must_use]
    pub fn proof_of_possession(&self) -> [u8; POP_LEN] {
        self.sk
            .proof_of_possession()
            .ok()
            .and_then(|p| to_array(Vec::<u8>::from(&p)).ok())
            .unwrap_or([0; POP_LEN])
    }

    #[must_use]
    pub fn sign_head(&self, head: &AuditHead) -> [u8; SIG_LEN] {
        self.sk
            .sign(SignatureSchemes::ProofOfPossession, &head.signing_bytes())
            .ok()
            .and_then(|s| Vec::<u8>::try_from(&s).ok())
            .and_then(|v| to_array(v).ok())
            .unwrap_or([0; SIG_LEN])
    }
}

/// `true` if a compressed point has the infinity flag set.
fn is_infinity(point_bytes: &[u8]) -> bool {
    point_bytes.first().is_some_and(|b| b & INFINITY_FLAG != 0)
}

fn decode_key(pk: &[u8; PK_LEN]) -> Result<PublicKey<C>, Error> {
    if is_infinity(pk) {
        return Err(Error::Malformed("public key is the point at infinity"));
    }
    PublicKey::<C>::try_from(&pk[..]).map_err(|_| Error::Malformed("public key"))
}

/// Decode a member signature: proof-of-possession variant only, never infinity.
fn decode_sig(sig: &[u8; SIG_LEN]) -> Result<Signature<C>, Error> {
    if is_infinity(&sig[1..]) {
        return Err(Error::Malformed("signature is the point at infinity"));
    }
    let s = Signature::<C>::try_from(&sig[..]).map_err(|_| Error::Malformed("signature"))?;
    if !matches!(s, Signature::ProofOfPossession(_)) {
        return Err(Error::Malformed("wrong signature scheme"));
    }
    Ok(s)
}

fn decode_aggregate(sig: &[u8; SIG_LEN]) -> Result<MultiSignature<C>, Error> {
    if is_infinity(&sig[1..]) {
        return Err(Error::Malformed("aggregate is the point at infinity"));
    }
    let m = MultiSignature::<C>::try_from(&sig[..])
        .map_err(|_| Error::Malformed("aggregate signature"))?;
    if !matches!(m, MultiSignature::ProofOfPossession(_)) {
        return Err(Error::Malformed("wrong signature scheme"));
    }
    Ok(m)
}

/// An ordered set of members whose keys have all proven possession. A
/// member's *index* is their slot in attestation bitmaps.
#[derive(Default)]
pub struct Roster {
    keys: Vec<PublicKey<C>>,
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

    /// Admit a member. Returns their index. Rejects: malformed / infinity /
    /// wrong-subgroup keys, an invalid proof of possession, and a key that is
    /// already registered.
    pub fn register(&mut self, pk: &[u8; PK_LEN], pop: &[u8; POP_LEN]) -> Result<usize, Error> {
        if self.keys.len() >= MAX_ROSTER {
            return Err(Error::InvalidParameter("roster full"));
        }
        let key = decode_key(pk)?;
        if is_infinity(pop) {
            return Err(Error::Malformed(
                "proof of possession is the point at infinity",
            ));
        }
        let pop = ProofOfPossession::<C>::try_from(&pop[..])
            .map_err(|_| Error::Malformed("proof of possession"))?;
        pop.verify(&key).map_err(|_| Error::VerificationFailed)?;
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
        decode_sig(sig)?
            .verify(key, head.signing_bytes())
            .map_err(|_| Error::VerificationFailed)
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
        let pks: Vec<PublicKey<C>> = signers.iter().map(|i| self.keys[*i]).collect();
        let mpk =
            MultiPublicKey::<C>::from_public_keys(&pks).map_err(|_| Error::VerificationFailed)?;
        decode_aggregate(&att.signature)?
            .verify(&mpk, head.signing_bytes())
            .map_err(|_| Error::VerificationFailed)?;
        Ok(signers.len())
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
    got: BTreeMap<usize, Signature<C>>,
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
        self.got.insert(index, decode_sig(sig)?);
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
        let sigs: Vec<Signature<C>> = self.got.values().copied().collect();
        let agg = MultiSignature::<C>::from_signatures(&sigs).map_err(up)?;
        let mut bitmap = vec![0u8; self.roster.len().div_ceil(8)];
        for i in self.got.keys() {
            bitmap[i / 8] |= 1 << (i % 8);
        }
        let bytes = Vec::<u8>::try_from(&agg).map_err(up)?;
        Ok(Attestation {
            bitmap,
            signature: to_array(bytes)?,
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
        let tagged = Vec::<u8>::try_from(
            &a.sk
                .sign(SignatureSchemes::ProofOfPossession, &a.public_key())
                .unwrap(),
        )
        .unwrap();
        let mut wrong_domain = [0u8; POP_LEN];
        wrong_domain.copy_from_slice(&tagged[1..]); // strip the scheme tag
        assert!(r.register(&a.public_key(), &wrong_domain).is_err());
        // Garbage key / garbage PoP.
        assert!(r.register(&[0; PK_LEN], &a.proof_of_possession()).is_err());
        assert!(r.register(&a.public_key(), &[0; POP_LEN]).is_err());
        assert!(r.is_empty(), "nothing was admitted");
        // The honest registration works once, and only once.
        r.register(&a.public_key(), &a.proof_of_possession())
            .unwrap();
        assert!(r
            .register(&a.public_key(), &a.proof_of_possession())
            .is_err());
        assert_eq!(r.len(), 1);
    }

    /// Each infinity guard on its own, so removing either one is caught (in
    /// `register` the two overlap and would hide each other).
    #[test]
    fn each_infinity_check_holds_on_its_own() {
        let mut inf_pk = [0u8; PK_LEN];
        inf_pk[0] = 0xc0;
        assert!(
            decode_key(&inf_pk).is_err(),
            "an infinity public key must not decode"
        );
        let mut inf_sig = [0u8; SIG_LEN];
        inf_sig[0] = 0x02;
        inf_sig[1] = 0xc0;
        assert!(
            decode_sig(&inf_sig).is_err(),
            "an infinity signature must not decode"
        );
        assert!(
            decode_aggregate(&inf_sig).is_err(),
            "an infinity aggregate must not decode"
        );
        // The library itself would have accepted the key — that is why the guard exists.
        assert!(PublicKey::<C>::try_from(&inf_pk[..]).is_ok());
        // And the proof of possession: infinity proof + a REAL key is refused.
        let mut r = Roster::new();
        let a = AuditKey::generate().unwrap();
        let mut inf_pop = [0u8; POP_LEN];
        inf_pop[0] = 0xc0;
        let e = r.register(&a.public_key(), &inf_pop).unwrap_err();
        assert!(
            matches!(e, Error::Malformed(m) if m.contains("infinity")),
            "{e:?}"
        );
    }

    #[test]
    fn a_signature_from_another_scheme_is_refused_even_though_it_parses() {
        let (r, ks) = group(2);
        let h = head(1);
        // A `Basic`-scheme signature by member 0 over the same bytes.
        let basic = ks[0]
            .sk
            .sign(SignatureSchemes::Basic, &h.signing_bytes())
            .unwrap();
        let bytes: [u8; SIG_LEN] = Vec::<u8>::try_from(&basic).unwrap().try_into().unwrap();
        assert!(
            Signature::<C>::try_from(&bytes[..]).is_ok(),
            "it does parse"
        );
        let e = r.verify_one(0, &h, &bytes).unwrap_err();
        assert!(
            matches!(e, Error::Malformed(m) if m.contains("scheme")),
            "{e:?}"
        );
    }

    #[test]
    fn the_point_at_infinity_is_not_a_key_or_a_signature() {
        // Compressed encoding of infinity in G1/G2: 0xc0 then zeros.
        let mut inf_pk = [0u8; PK_LEN];
        inf_pk[0] = 0xc0;
        let mut inf_pop = [0u8; POP_LEN];
        inf_pop[0] = 0xc0;
        // A tagged signature/aggregate whose point is infinity.
        let mut inf_sig = [0u8; SIG_LEN];
        inf_sig[0] = 0x02; // the proof-of-possession scheme tag
        inf_sig[1] = 0xc0;
        let mut r = Roster::new();
        // Rejected for the RIGHT reason: our explicit infinity check
        // (`blsful` itself would accept this key on decode).
        let e = r.register(&inf_pk, &inf_pop).unwrap_err();
        assert!(
            matches!(e, Error::Malformed(m) if m.contains("infinity")),
            "{e:?}"
        );
        assert!(r.is_empty());

        let (r, ks) = group(3);
        let a = Attestation {
            bitmap: vec![0b111],
            signature: inf_sig,
        };
        let e = r.verify(&head(1), &a, 1).unwrap_err();
        assert!(
            matches!(e, Error::Malformed(m) if m.contains("infinity")),
            "{e:?}"
        );
        // Neither can an infinity signature be contributed as one member's.
        assert!(r.verify_one(0, &head(1), &inf_sig).is_err());
        let _ = ks;
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
            let mut pop = [0u8; POP_LEN];
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
