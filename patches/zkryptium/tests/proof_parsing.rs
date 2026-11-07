use zkryptium::keys::pair::KeyPair;
use zkryptium::schemes::algorithms::{BBSplus, BbsBls12381Sha256, Scheme};
use zkryptium::schemes::generics::{PoKSignature, Signature};

type S = BBSplus<<BbsBls12381Sha256 as Scheme>::Ciphersuite>;

/// A genuine proof: 3 messages, message index 1 disclosed (2 hidden).
fn valid_proof() -> Vec<u8> {
    let kp = KeyPair::<S>::generate(&[7u8; 32], None, None).unwrap();
    let msgs: Vec<Vec<u8>> = vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()];
    let sig = Signature::<S>::sign(
        Some(&msgs),
        kp.private_key(),
        kp.public_key(),
        Some(b"hdr"),
    )
    .unwrap();
    PoKSignature::<S>::proof_gen(
        kp.public_key(),
        &sig.to_bytes(),
        Some(b"hdr"),
        Some(b"ph"),
        Some(&msgs),
        Some(&[1usize]),
    )
    .unwrap()
    .to_bytes()
}

#[test]
fn from_bytes_never_panics_on_short_input() {
    for len in [0usize, 1, 47, 48, 95, 143, 175, 239] {
        let r = std::panic::catch_unwind(|| {
            PoKSignature::<S>::from_bytes(&vec![0u8; len]).is_err()
        });
        assert!(
            r.is_ok(),
            "PoKSignature::from_bytes PANICKED on a {len}-byte input"
        );
    }
}

#[test]
fn from_bytes_rejects_trailing_bytes() {
    let good = valid_proof();
    assert!(
        PoKSignature::<S>::from_bytes(&good).is_ok(),
        "control: a real proof parses"
    );
    let mut extended = good.clone();
    extended.push(0xAA);
    assert!(
        PoKSignature::<S>::from_bytes(&extended).is_err(),
        "a proof with {} trailing byte(s) was accepted (len {} instead of {})",
        extended.len() - good.len(),
        extended.len(),
        good.len()
    );
}

#[test]
fn from_bytes_rejects_non_canonical_lengths() {
    for len in [240usize, 241, 271, 273, 303, 305, 335] {
        let dummy = vec![0u8; len];
        assert!(
            PoKSignature::<S>::from_bytes(&dummy).is_err(),
            "non-canonical length {len} must be rejected"
        );
    }
}

