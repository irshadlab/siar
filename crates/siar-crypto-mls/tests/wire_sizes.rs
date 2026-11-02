//! Measures what each candidate 1:1 suite actually puts on the wire.
//! Run: cargo test -p siar-crypto-mls --test wire_sizes -- --nocapture
use openmls::prelude::*;
use openmls_rust_crypto::OpenMlsRustCrypto;
use siar_crypto_mls::*;
use siar_domain::DeviceId;

fn measure(name: &str, suite: Ciphersuite) {
    let bp = OpenMlsRustCrypto::default();
    let bob = generate_identity_with_suite(DeviceId::new(), &bp, suite).unwrap();
    let kp = encode_key_package(bob.key_package.key_package()).unwrap();
    let ap = OpenMlsRustCrypto::default();
    let alice = generate_identity_with_suite(DeviceId::new(), &ap, suite).unwrap();
    let params = GroupParams {
        ciphersuite: suite,
        ..GroupParams::pairwise_dtn()
    };
    let mut a = MlsGroupSession::create_with(ap, &alice, params).unwrap();
    let kpi = decode_key_package(&kp).unwrap();
    let (commit0, welcome) = a.add_member(&kpi).unwrap();
    let msg = a.encrypt(&[0u8; 100]).unwrap();
    let upd = a.self_update().unwrap();
    println!(
        "{name:<34} key_package={:>5}  welcome={:>5}  add_commit={:>5}  100B_msg={:>5}  self_update={:>5}",
        kp.len(), welcome.len(), commit0.len(), msg.len(), upd.len()
    );
}

#[test]
fn print_sizes() {
    measure("classical X25519+Ed25519", CIPHERSUITE);
    measure(
        "hybrid KEM + Ed25519 (compact)",
        Ciphersuite::MLS_128_MLKEM768X25519_AES128GCM_SHA256_Ed25519,
    );
    measure("hybrid KEM + ML-DSA-44 (full PQ)", PQ_FULL_CIPHERSUITE);
}
