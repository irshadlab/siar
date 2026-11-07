//! Hybrid **ML-DSA-65 + Ed25519** signatures, following the composite
//! construction of IETF `draft-ietf-lamps-pq-composite-sigs`
//! (`MLDSA65-Ed25519-SHA512`):
//!
//! ```text
//! M'  = Prefix || Label || len(ctx) || ctx || SHA-512(M)
//! sig = ML-DSA-65.Sign(sk_pq, M', ctx = Label)  ||  Ed25519.Sign(sk_ed, M')
//! ```
//!
//! A hybrid signature is valid only if **both** halves verify, so forging
//! one needs breaking *both* a lattice scheme and Ed25519. That matters
//! because the lattice implementation here (`ml-dsa`) has never been
//! independently audited: a bug in it alone cannot forge a hybrid signature
//! while the Ed25519 half holds.
//!
//! What this is for in Siar:
//!
//! * **Long-term identity certificates.** `DeviceCertificate` has no schema
//!   version (a known open gap), so its signature field is *not* changed.
//!   Instead a hybrid signature is made over
//!   `DeviceCertificate::canonical_signing_bytes()` and carried *beside* the
//!   certificate. Old verifiers ignore it; new ones require it.
//! * **Encrypted backups.** `siar-backup` already encrypts with a passphrase
//!   (Argon2id + XChaCha20-Poly1305): confidentiality is symmetric and needs
//!   no PQ key exchange. A detached hybrid signature over the archive adds
//!   post-quantum-safe *authenticity* ("this came from that account").
//!
//! The Ed25519 half may be a FROST group key: sign
//! [`message_representative`] with [`crate::frost_root`] and
//! [`HybridSignature::assemble`] the result with an ML-DSA signature. The
//! ML-DSA half stays a **single-holder key** — there is no audited threshold
//! ML-DSA — so the threshold property protects the classical half only.
//!
//! **Caveats, stated plainly:** the draft is still moving (the prefix/label
//! are the values used by current implementations of drafts -12…-16); no
//! official test vectors were available offline, so *interoperability with
//! other implementations is untested*. Self-consistency and tamper-rejection
//! are tested here.

use ed25519_dalek::{Signature as EdSignature, Signer, SigningKey as EdSigningKey, VerifyingKey};
use ml_dsa::VerifyingKey as PqVerifyingKey;
use ml_dsa::{ExpandedSigningKey as PqSigningKey, MlDsa65, Seed, Signature as PqSignature};
use sha2::{Digest, Sha512};

use crate::{fill_random, Error};

pub const PREFIX: &[u8] = b"CompositeAlgorithmSignatures2025";
pub const LABEL: &[u8] = b"COMPSIG-MLDSA65-Ed25519-SHA512";

pub const PQ_PK_LEN: usize = 1952;
pub const PQ_SIG_LEN: usize = 3309;
pub const ED_PK_LEN: usize = 32;
pub const ED_SIG_LEN: usize = 64;
/// ML-DSA public key ‖ Ed25519 public key.
pub const PK_LEN: usize = PQ_PK_LEN + ED_PK_LEN;
/// ML-DSA signature ‖ Ed25519 signature.
pub const SIG_LEN: usize = PQ_SIG_LEN + ED_SIG_LEN;

/// `M'` — what both component algorithms actually sign. Public so a
/// threshold Ed25519 signer (FROST) can sign it.
pub fn message_representative(message: &[u8], ctx: &[u8]) -> Result<Vec<u8>, Error> {
    let ctx_len =
        u8::try_from(ctx.len()).map_err(|_| Error::InvalidParameter("ctx > 255 bytes"))?;
    let mut m = Vec::with_capacity(PREFIX.len() + LABEL.len() + 1 + ctx.len() + 64);
    m.extend_from_slice(PREFIX);
    m.extend_from_slice(LABEL);
    m.push(ctx_len);
    m.extend_from_slice(ctx);
    m.extend_from_slice(&Sha512::digest(message));
    Ok(m)
}

pub struct HybridSecretKey {
    pq: PqSigningKey<MlDsa65>,
    ed: EdSigningKey,
}

impl HybridSecretKey {
    pub fn generate() -> Result<Self, Error> {
        let mut pq_seed = [0u8; 32];
        let mut ed_seed = [0u8; 32];
        fill_random(&mut pq_seed)?;
        fill_random(&mut ed_seed)?;
        let k = Self::from_seeds(&pq_seed, &ed_seed);
        zeroize::Zeroize::zeroize(&mut pq_seed);
        zeroize::Zeroize::zeroize(&mut ed_seed);
        Ok(k)
    }

    /// The composite private key is exactly these two 32-byte seeds.
    #[must_use]
    pub fn from_seeds(pq_seed: &[u8; 32], ed_seed: &[u8; 32]) -> Self {
        Self {
            pq: PqSigningKey::<MlDsa65>::from_seed(&Seed::from(*pq_seed)),
            ed: EdSigningKey::from_bytes(ed_seed),
        }
    }

    #[must_use]
    pub fn public_key(&self) -> HybridPublicKey {
        HybridPublicKey {
            pq: self.pq.verifying_key(),
            ed: self.ed.verifying_key(),
        }
    }

    /// Hedged (randomised) ML-DSA signing from the OS CSPRNG; Ed25519 is
    /// deterministic by design.
    pub fn sign(&self, message: &[u8], ctx: &[u8]) -> Result<HybridSignature, Error> {
        let m = message_representative(message, ctx)?;
        let mut rng = getrandom::SysRng;
        let pq = self
            .pq
            .sign_randomized(&m, LABEL, &mut rng)
            .map_err(|e| Error::Upstream(e.to_string()))?;
        let ed = self.ed.sign(&m);
        HybridSignature::assemble(&pq.encode(), &ed.to_bytes())
    }
}

#[derive(Clone, PartialEq)]
pub struct HybridPublicKey {
    pq: PqVerifyingKey<MlDsa65>,
    ed: VerifyingKey,
}

impl std::fmt::Debug for HybridPublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HybridPublicKey(..)")
    }
}

impl HybridPublicKey {
    /// Combine independently produced halves — e.g. a FROST group key as
    /// the Ed25519 half.
    pub fn from_parts(pq: &[u8], ed: &[u8; 32]) -> Result<Self, Error> {
        let pq: &[u8; PQ_PK_LEN] = pq
            .try_into()
            .map_err(|_| Error::Malformed("ML-DSA public key length"))?;
        Ok(Self {
            pq: PqVerifyingKey::<MlDsa65>::decode(&(*pq).into()),
            ed: VerifyingKey::from_bytes(ed).map_err(|_| Error::Malformed("Ed25519 key"))?,
        })
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, Error> {
        if b.len() != PK_LEN {
            return Err(Error::Malformed("hybrid public key length"));
        }
        let (pq, ed) = b.split_at(PQ_PK_LEN);
        let ed: [u8; 32] = ed.try_into().map_err(|_| Error::Malformed("ed key"))?;
        Self::from_parts(pq, &ed)
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(PK_LEN);
        v.extend_from_slice(&self.pq.encode());
        v.extend_from_slice(self.ed.as_bytes());
        v
    }

    /// The Ed25519 half, e.g. to compare against a `RootPublicKey`.
    #[must_use]
    pub fn ed25519_bytes(&self) -> [u8; 32] {
        self.ed.to_bytes()
    }

    /// Both halves must verify. Both are always evaluated (no early exit on
    /// the first failure) and the result is their conjunction.
    pub fn verify(&self, message: &[u8], ctx: &[u8], sig: &HybridSignature) -> Result<(), Error> {
        let m = message_representative(message, ctx)?;
        let (pq_bytes, ed_bytes) = sig.0.split_at(PQ_SIG_LEN);

        let pq_ok = PqSignature::<MlDsa65>::try_from(pq_bytes)
            .map(|s| self.pq.verify_with_context(&m, LABEL, &s))
            .unwrap_or(false);
        let ed_ok = <[u8; ED_SIG_LEN]>::try_from(ed_bytes)
            .map(|b| {
                self.ed
                    .verify_strict(&m, &EdSignature::from_bytes(&b))
                    .is_ok()
            })
            .unwrap_or(false);

        if pq_ok & ed_ok {
            Ok(())
        } else {
            Err(Error::VerificationFailed)
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct HybridSignature(Vec<u8>);

impl std::fmt::Debug for HybridSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HybridSignature({} bytes)", self.0.len())
    }
}

impl HybridSignature {
    /// Join an ML-DSA-65 signature and an Ed25519 signature (the latter
    /// possibly from FROST) over the same [`message_representative`].
    pub fn assemble(pq: &[u8], ed: &[u8; 64]) -> Result<Self, Error> {
        if pq.len() != PQ_SIG_LEN {
            return Err(Error::Malformed("ML-DSA signature length"));
        }
        let mut v = Vec::with_capacity(SIG_LEN);
        v.extend_from_slice(pq);
        v.extend_from_slice(ed);
        Ok(Self(v))
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, Error> {
        if b.len() != SIG_LEN {
            return Err(Error::Malformed("hybrid signature length"));
        }
        Ok(Self(b.to_vec()))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: &[u8] = b"siar-device-cert-v1";

    #[test]
    fn sizes_match_the_composite_encoding() {
        let k = HybridSecretKey::generate().unwrap();
        assert_eq!(k.public_key().to_bytes().len(), PK_LEN);
        assert_eq!(k.sign(b"m", CTX).unwrap().as_bytes().len(), SIG_LEN);
        assert_eq!((PK_LEN, SIG_LEN), (1984, 3373));
    }

    #[test]
    fn sign_verify_round_trip_and_key_serialisation() {
        let k = HybridSecretKey::generate().unwrap();
        let pk = k.public_key();
        let sig = k.sign(b"hello", CTX).unwrap();
        pk.verify(b"hello", CTX, &sig).unwrap();
        let pk2 = HybridPublicKey::from_bytes(&pk.to_bytes()).unwrap();
        pk2.verify(
            b"hello",
            CTX,
            &HybridSignature::from_bytes(sig.as_bytes()).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn message_context_and_key_are_all_bound() {
        let k = HybridSecretKey::generate().unwrap();
        let other = HybridSecretKey::generate().unwrap();
        let pk = k.public_key();
        let sig = k.sign(b"hello", CTX).unwrap();
        assert!(pk.verify(b"hellp", CTX, &sig).is_err(), "message");
        assert!(pk.verify(b"hello", b"other-ctx", &sig).is_err(), "context");
        assert!(
            other.public_key().verify(b"hello", CTX, &sig).is_err(),
            "key"
        );
    }

    #[test]
    fn both_halves_are_required() {
        let k = HybridSecretKey::generate().unwrap();
        let pk = k.public_key();
        let sig = k.sign(b"m", CTX).unwrap();

        // Corrupt only the ML-DSA half → reject (Ed25519 half still valid).
        let mut b = sig.as_bytes().to_vec();
        b[10] ^= 0x01;
        assert!(pk
            .verify(b"m", CTX, &HybridSignature::from_bytes(&b).unwrap())
            .is_err());

        // Corrupt only the Ed25519 half → reject (ML-DSA half still valid).
        let mut b = sig.as_bytes().to_vec();
        b[PQ_SIG_LEN + 5] ^= 0x01;
        assert!(pk
            .verify(b"m", CTX, &HybridSignature::from_bytes(&b).unwrap())
            .is_err());

        // A valid signature from ANOTHER key in either half does not splice in.
        let other = HybridSecretKey::generate().unwrap();
        let osig = other.sign(b"m", CTX).unwrap();
        let spliced_ed = HybridSignature::assemble(
            &sig.as_bytes()[..PQ_SIG_LEN],
            osig.as_bytes()[PQ_SIG_LEN..].try_into().unwrap(),
        )
        .unwrap();
        assert!(pk.verify(b"m", CTX, &spliced_ed).is_err());
        let spliced_pq = HybridSignature::assemble(
            &osig.as_bytes()[..PQ_SIG_LEN],
            sig.as_bytes()[PQ_SIG_LEN..].try_into().unwrap(),
        )
        .unwrap();
        assert!(pk.verify(b"m", CTX, &spliced_pq).is_err());
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        assert!(HybridSignature::from_bytes(&[0; SIG_LEN - 1]).is_err());
        assert!(HybridSignature::from_bytes(&[]).is_err());
        assert!(HybridPublicKey::from_bytes(&[0; PK_LEN + 1]).is_err());
        assert!(HybridPublicKey::from_bytes(&[]).is_err());
        let k = HybridSecretKey::generate().unwrap();
        let pk = k.public_key();
        // All-zero signature bytes of the right length must just fail.
        let zero = HybridSignature::from_bytes(&[0; SIG_LEN]).unwrap();
        assert!(pk.verify(b"m", CTX, &zero).is_err());
        assert!(k.sign(b"m", &[0u8; 256]).is_err(), "ctx limited to 255");
        assert!(k.sign(b"m", &[0u8; 255]).is_ok());
    }

    #[test]
    fn message_representative_has_the_draft_layout() {
        let m = message_representative(b"abc", b"xy").unwrap();
        assert!(m.starts_with(PREFIX));
        assert_eq!(&m[PREFIX.len()..PREFIX.len() + LABEL.len()], LABEL);
        let off = PREFIX.len() + LABEL.len();
        assert_eq!(m[off], 2);
        assert_eq!(&m[off + 1..off + 3], b"xy");
        assert_eq!(&m[off + 3..], Sha512::digest(b"abc").as_slice());
        assert_eq!(m.len(), off + 3 + 64);
    }

    #[test]
    fn seeds_are_the_whole_private_key() {
        let a = HybridSecretKey::from_seeds(&[1; 32], &[2; 32]);
        let b = HybridSecretKey::from_seeds(&[1; 32], &[2; 32]);
        assert_eq!(a.public_key().to_bytes(), b.public_key().to_bytes());
        let c = HybridSecretKey::from_seeds(&[1; 32], &[3; 32]);
        assert_ne!(a.public_key().to_bytes(), c.public_key().to_bytes());
    }

    #[test]
    fn identity_certificate_countersignature_without_touching_the_certificate() {
        use siar_domain::{AccountId, DeviceId};
        use siar_identity_multidevice::certificate::DeviceCertificate;
        use siar_identity_multidevice::root_key::RootIdentityKey;
        use siar_identity_multidevice::DeviceCapabilitySet;

        let root = RootIdentityKey::generate();
        let cert = DeviceCertificate::issue(
            &root,
            AccountId::new(),
            DeviceId::new(),
            [7; 32],
            1_000,
            Some(9_000),
            DeviceCapabilitySet::SEND_MESSAGE,
            1,
        );
        // The classical certificate is exactly as before and still verifies.
        cert.verify_signature(&root.root_public_key()).unwrap();

        // Hybrid countersignature over the identical signed bytes.
        let hk = HybridSecretKey::generate().unwrap();
        let counter = hk
            .sign(&cert.canonical_signing_bytes(), b"siar-device-cert-v1")
            .unwrap();
        hk.public_key()
            .verify(
                &cert.canonical_signing_bytes(),
                b"siar-device-cert-v1",
                &counter,
            )
            .unwrap();

        // Changing any certificate field invalidates the countersignature.
        let mut tampered = cert.clone();
        tampered.generation = 2;
        assert!(hk
            .public_key()
            .verify(
                &tampered.canonical_signing_bytes(),
                b"siar-device-cert-v1",
                &counter
            )
            .is_err());
    }

    #[test]
    fn a_frost_group_can_be_the_ed25519_half() {
        use crate::frost_root::*;
        use std::collections::BTreeMap;

        // 2-of-3 FROST DKG (reuse the real ceremony).
        let t = Threshold::new(2, 3).unwrap();
        let ids: Vec<_> = (1..=3).map(|n| identifier(n).unwrap()).collect();
        let mut s1 = BTreeMap::new();
        let mut p1 = BTreeMap::new();
        for id in &ids {
            let (s, p) = dkg_part1(*id, t).unwrap();
            s1.insert(*id, s);
            p1.insert(*id, p);
        }
        let mut s2 = BTreeMap::new();
        let mut inbox: BTreeMap<_, BTreeMap<_, _>> = BTreeMap::new();
        for id in &ids {
            let others = p1
                .iter()
                .filter(|(k, _)| *k != id)
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            let (s, out) = dkg_part2(s1.remove(id).unwrap(), &others).unwrap();
            s2.insert(*id, s);
            for (to, pkg) in out {
                inbox.entry(to).or_default().insert(*id, pkg);
            }
        }
        let mut keys = BTreeMap::new();
        let mut public = None;
        for id in &ids {
            let others = p1
                .iter()
                .filter(|(k, _)| *k != id)
                .map(|(k, v)| (*k, v.clone()))
                .collect();
            let (kp, pp) = dkg_part3(&s2[id], &others, &inbox[id]).unwrap();
            keys.insert(*id, kp);
            public = Some(pp);
        }
        let public = public.unwrap();
        let ed_pk = root_public_key(&public).unwrap();

        // Single-holder ML-DSA half + FROST group as the Ed25519 half.
        let pq_only = HybridSecretKey::from_seeds(&[5; 32], &[6; 32]);
        let pq_pk = pq_only.public_key().to_bytes()[..PQ_PK_LEN].to_vec();
        let pk = HybridPublicKey::from_parts(&pq_pk, &ed_pk).unwrap();

        let msg = b"root-attested statement";
        let m_prime = message_representative(msg, b"ctx").unwrap();

        // Ed25519 half: 2-of-3 FROST signs M'.
        let mut nonces = BTreeMap::new();
        let mut commits = BTreeMap::new();
        for n in [1u16, 3] {
            let id = identifier(n).unwrap();
            let (nn, cc) = sign_round1(&keys[&id]);
            nonces.insert(id, nn);
            commits.insert(id, cc);
        }
        let pkg = signing_package(commits, &m_prime);
        let mut shares = BTreeMap::new();
        for n in [1u16, 3] {
            let id = identifier(n).unwrap();
            shares.insert(
                id,
                sign_round2(&pkg, nonces.remove(&id).unwrap(), &keys[&id]).unwrap(),
            );
        }
        let ed_sig = aggregate(&pkg, &shares, &public).unwrap();

        // ML-DSA half: the single-holder key signs the same M'.
        let full = pq_only.sign(msg, b"ctx").unwrap();
        let hybrid = HybridSignature::assemble(&full.as_bytes()[..PQ_SIG_LEN], &ed_sig).unwrap();
        pk.verify(msg, b"ctx", &hybrid).unwrap();
        assert!(pk.verify(b"different", b"ctx", &hybrid).is_err());
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

    #[test]
    fn hostile_hybrid_keys_and_signatures_never_panic_and_only_the_genuine_one_verifies() {
        let k = HybridSecretKey::from_seeds(&[3; 32], &[4; 32]);
        let pk = k.public_key();
        let sig = k.sign(b"msg", b"ctx").unwrap();
        pk.verify(b"msg", b"ctx", &sig).unwrap();
        let pk_bytes = pk.to_bytes();
        let mut rng = Rng(0x41B1D);
        for round in 0..120 {
            let bytes = if round % 2 == 0 {
                rng.mutate(sig.as_bytes())
            } else {
                rng.bytes(SIG_LEN)
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                HybridSignature::from_bytes(&bytes).and_then(|s| pk.verify(b"msg", b"ctx", &s))
            }));
            let r = r.unwrap_or_else(|_| panic!("hybrid verify PANICKED (round {round})"));
            if bytes != sig.as_bytes() {
                assert!(
                    r.is_err(),
                    "a modified hybrid signature verified (round {round})"
                );
            }
        }
        for _ in 0..60 {
            let bytes = if rng.below(2) == 0 {
                rng.mutate(&pk_bytes)
            } else {
                rng.bytes(PK_LEN)
            };
            let r = std::panic::catch_unwind(|| {
                HybridPublicKey::from_bytes(&bytes).map(|p| p.to_bytes().len())
            });
            let r = r.unwrap_or_else(|_| panic!("hybrid key decode PANICKED"));
            if let Ok(len) = r {
                assert_eq!(len, PK_LEN);
            }
        }
    }
}
