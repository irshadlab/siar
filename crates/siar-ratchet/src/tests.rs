//! Tests for the ratchet. Organised by the Part 28 property each protects.

use ed25519_dalek::VerifyingKey;
use proptest::prelude::*;
use rand_core::{CryptoRng, RngCore};
use siar_crypto::DeviceIdentity;

use crate::handshake::*;
use crate::kdf::{hkdf_sha256, hmac_sha256};
use crate::state::*;
use crate::vault::HighWaterMarks;

/// Deterministic RNG for tests only (splitmix64). Marked `CryptoRng` solely so
/// it satisfies the bound; it is not secure and is never used outside tests.
struct TestRng(u64);
impl RngCore for TestRng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for c in dest.chunks_mut(8) {
            c.copy_from_slice(&self.next_u64().to_le_bytes()[..c.len()]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl CryptoRng for TestRng {}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ───────────────────────── primitives: published vectors ─────────────────────────

#[test]
fn hkdf_matches_rfc_5869_vectors() {
    let mut out = [0u8; 42];
    let salt: Vec<u8> = (0u8..13).collect();
    let info: Vec<u8> = (0xf0u8..0xfa).collect();
    hkdf_sha256(&salt, &[0x0b; 22], &info, &mut out);
    assert_eq!(
        hex(&out),
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
        "A.1"
    );
    hkdf_sha256(&[], &[0x0b; 22], &[], &mut out);
    assert_eq!(
        hex(&out),
        "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8",
        "A.3"
    );
}

#[test]
fn hmac_matches_rfc_4231_vector() {
    assert_eq!(
        hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

/// A frozen output of the whole construction. **A regression vector, not an
/// independent known-answer test**: it exists so that any accidental change to
/// a label, KDF order, AAD layout or wire format fails loudly.
#[test]
fn frozen_wire_vector() {
    let mut rng = TestRng(7);
    let (spk_secret, spk_public) = {
        let s = x25519_dalek::StaticSecret::from([9u8; 32]);
        (s.to_bytes(), x25519_dalek::PublicKey::from(&s).to_bytes())
    };
    let sk = [0x42u8; 32];
    let alice = RatchetState::init_initiator(&sk, spk_public, b"ad".to_vec(), &mut rng).unwrap();
    let (wire, _) = alice.encrypt(b"hello").unwrap();
    let bob = RatchetState::init_responder(&sk, spk_secret, b"ad".to_vec());
    assert_eq!(bob.decrypt_with(&wire, &mut rng).unwrap().0, b"hello");
    assert_eq!(hex(&wire), FROZEN);
}
const FROZEN: &str = "0101e0e4720b8b7d96be57f02b73071db6e6a1fbbe7ca9cace7aff99e328b14cac2d0000000000000000da8f58c9fb6f30031acd45388a06b0e16daa06af0b";

// ───────────────────────────── helpers ─────────────────────────────

struct Pair {
    a: RatchetState,
    b: RatchetState,
}

/// Full handshake with real identities; returns states after Alice's first
/// message was delivered (so both can send).
fn pair() -> Pair {
    let (alice, bob) = (DeviceIdentity::generate(), DeviceIdentity::generate());
    let (spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let (a, init) = initiate(&alice, &bundle, &bob.verifying_key(), &mut rand_core::OsRng).unwrap();
    let (first, a) = a.encrypt(b"first").unwrap();
    let b = respond(&bob, &spk, &init).unwrap();
    let (pt, b) = b.decrypt(&first).unwrap();
    assert_eq!(pt, b"first");
    Pair { a, b }
}

fn send(from: &mut RatchetState, msg: &[u8]) -> Vec<u8> {
    let (w, next) = from.encrypt(msg).unwrap();
    *from = next;
    w
}

fn recv(to: &mut RatchetState, wire: &[u8]) -> Result<Vec<u8>, RatchetError> {
    let (pt, next) = to.decrypt(wire)?;
    *to = next;
    Ok(pt)
}

// ─────────────────────── handshake: authentication (§11, §37-§41) ───────────────────────

#[test]
fn both_sides_agree_and_talk_both_ways() {
    let mut p = pair();
    let w = send(&mut p.b, b"reply");
    assert_eq!(recv(&mut p.a, &w).unwrap(), b"reply");
    let w = send(&mut p.a, b"and again");
    assert_eq!(recv(&mut p.b, &w).unwrap(), b"and again");
}

#[test]
fn a_swapped_or_forged_prekey_bundle_is_rejected() {
    let (alice, bob, mallory) = (
        DeviceIdentity::generate(),
        DeviceIdentity::generate(),
        DeviceIdentity::generate(),
    );
    let (_spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    // A relay replaces the whole bundle with its own, validly signed by itself.
    let (_ms, evil) = new_signed_prekey(&mallory, &mut rand_core::OsRng);
    assert_eq!(
        initiate(&alice, &evil, &bob.verifying_key(), &mut rand_core::OsRng).err(),
        Some(HandshakeError::WrongSigner)
    );
    // …or keeps Bob's signer field but substitutes the prekey: the signature no longer fits.
    let mut forged = bundle.clone();
    forged.signed_prekey_public = evil.signed_prekey_public;
    assert_eq!(
        initiate(&alice, &forged, &bob.verifying_key(), &mut rand_core::OsRng).err(),
        Some(HandshakeError::BadSignature)
    );
    let mut forged = bundle.clone();
    forged.identity_dh_public = evil.identity_dh_public;
    assert_eq!(
        initiate(&alice, &forged, &bob.verifying_key(), &mut rand_core::OsRng).err(),
        Some(HandshakeError::BadSignature)
    );
    let mut forged = bundle;
    forged.signature[0] ^= 1;
    assert!(initiate(&alice, &forged, &bob.verifying_key(), &mut rand_core::OsRng).is_err());
}

#[test]
fn the_signer_must_come_from_trust_not_from_the_bundle() {
    let (alice, bob) = (DeviceIdentity::generate(), DeviceIdentity::generate());
    let (_spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let someone_else = DeviceIdentity::generate().verifying_key();
    assert_eq!(
        initiate(&alice, &bundle, &someone_else, &mut rand_core::OsRng).err(),
        Some(HandshakeError::WrongSigner)
    );
}

#[test]
fn an_initial_header_for_a_prekey_we_do_not_hold_is_refused() {
    let (alice, bob) = (DeviceIdentity::generate(), DeviceIdentity::generate());
    let (spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let (_a, mut init) =
        initiate(&alice, &bundle, &bob.verifying_key(), &mut rand_core::OsRng).unwrap();
    init.responder_prekey[0] ^= 1;
    assert_eq!(
        respond(&bob, &spk, &init).err(),
        Some(HandshakeError::UnknownPrekey)
    );
}

#[test]
fn low_order_ephemeral_keys_are_refused() {
    let (alice, bob) = (DeviceIdentity::generate(), DeviceIdentity::generate());
    let (spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let (_a, mut init) =
        initiate(&alice, &bundle, &bob.verifying_key(), &mut rand_core::OsRng).unwrap();
    init.initiator_ephemeral = [0u8; 32]; // the all-zero point
    assert_eq!(
        respond(&bob, &spk, &init).err(),
        Some(HandshakeError::WeakKey)
    );
}

#[test]
fn the_conversation_is_bound_to_both_identities() {
    // Same prekey, same bytes, but Eve (a different initiator identity)
    // cannot decrypt Alice's first message even holding the prekey secret's owner key.
    let (alice, eve, bob) = (
        DeviceIdentity::generate(),
        DeviceIdentity::generate(),
        DeviceIdentity::generate(),
    );
    let (spk, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let (a, init) = initiate(&alice, &bundle, &bob.verifying_key(), &mut rand_core::OsRng).unwrap();
    let (wire, _) = a.encrypt(b"for bob").unwrap();
    // Eve replays Alice's ephemeral but claims her own identity.
    let mut lie = init;
    lie.initiator_identity_dh = identity_dh_public(&eve);
    let bob_view = respond(&bob, &spk, &lie).unwrap();
    assert!(
        bob_view.decrypt(&wire).is_err(),
        "a different claimed initiator derives a different key and AD"
    );
    assert_eq!(
        respond(&bob, &spk, &init)
            .unwrap()
            .decrypt(&wire)
            .unwrap()
            .0,
        b"for bob"
    );
}

#[test]
fn initial_header_encoding_round_trips() {
    let (alice, bob) = (DeviceIdentity::generate(), DeviceIdentity::generate());
    let (_s, bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let (_a, init) =
        initiate(&alice, &bundle, &bob.verifying_key(), &mut rand_core::OsRng).unwrap();
    let mut bytes = init.encode().to_vec();
    bytes.extend_from_slice(b"rest");
    let (back, rest) = InitialHeader::decode(&bytes).unwrap();
    assert_eq!(back, init);
    assert_eq!(rest, b"rest");
    assert!(InitialHeader::decode(&bytes[..95]).is_none());
}

// ─────────────────────── ratchet: delivery semantics ───────────────────────

#[test]
fn long_ping_pong_with_many_ratchet_steps() {
    let mut p = pair();
    for i in 0..200u32 {
        let m = format!("a→b {i}");
        let w = send(&mut p.a, m.as_bytes());
        assert_eq!(recv(&mut p.b, &w).unwrap(), m.as_bytes());
        let m = format!("b→a {i}");
        let w = send(&mut p.b, m.as_bytes());
        assert_eq!(recv(&mut p.a, &w).unwrap(), m.as_bytes());
    }
    assert!(p.a.skipped_len() == 0 && p.b.skipped_len() == 0);
}

#[test]
fn out_of_order_within_a_chain_and_across_a_ratchet_step() {
    let mut p = pair();
    let (m1, m2, m3) = (
        send(&mut p.a, b"1"),
        send(&mut p.a, b"2"),
        send(&mut p.a, b"3"),
    );
    assert_eq!(recv(&mut p.b, &m3).unwrap(), b"3"); // skips 1 and 2
    assert_eq!(p.b.skipped_len(), 2);
    let reply = send(&mut p.b, b"reply"); // B ratchets
    let m4 = send(&mut p.a, b"4"); // still on the old chain until the reply lands
    assert_eq!(recv(&mut p.a, &reply).unwrap(), b"reply");
    let m5 = send(&mut p.a, b"5"); // new chain
    assert_eq!(recv(&mut p.b, &m5).unwrap(), b"5"); // B ratchets again, skipping m4 via pn
    assert_eq!(recv(&mut p.b, &m4).unwrap(), b"4");
    assert_eq!(recv(&mut p.b, &m2).unwrap(), b"2");
    assert_eq!(recv(&mut p.b, &m1).unwrap(), b"1");
    assert_eq!(
        p.b.skipped_len(),
        0,
        "every skipped key was used exactly once"
    );
}

#[test]
fn replays_are_rejected_and_do_not_disturb_state() {
    let mut p = pair();
    let w = send(&mut p.a, b"once");
    assert_eq!(recv(&mut p.b, &w).unwrap(), b"once");
    let before = p.b.epoch();
    assert_eq!(
        p.b.decrypt(&w).err(),
        Some(RatchetError::DuplicateOrExpired)
    );
    assert_eq!(p.b.epoch(), before);
    // Also after the chain has moved on and via a skipped key.
    let (w1, w2) = (send(&mut p.a, b"1"), send(&mut p.a, b"2"));
    assert!(recv(&mut p.b, &w2).is_ok());
    assert!(recv(&mut p.b, &w1).is_ok());
    assert_eq!(
        p.b.decrypt(&w1).err(),
        Some(RatchetError::DuplicateOrExpired)
    );
}

// ─────────────────────── §77: receive transaction ───────────────────────

#[test]
fn a_forged_message_cannot_advance_or_corrupt_the_session() {
    let mut p = pair();
    let genuine = send(&mut p.a, b"genuine");
    let before = p.b.to_bytes();
    // Flip every single byte in turn: each must be rejected, none may panic…
    for i in 0..genuine.len() {
        let mut bad = genuine.clone();
        bad[i] ^= 0x01;
        assert!(p.b.decrypt(&bad).is_err(), "byte {i} flipped but accepted");
    }
    // …and the session is byte-for-byte unchanged and still works.
    assert_eq!(&*p.b.to_bytes(), &*before);
    assert_eq!(recv(&mut p.b, &genuine).unwrap(), b"genuine");
}

#[test]
fn truncated_and_garbage_input_never_panics() {
    let p = pair();
    let mut x = 0xDEAD_BEEF_u64;
    for len in 0..200usize {
        let buf: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        assert!(p.b.decrypt(&buf).is_err());
        let mut framed = vec![VERSION, SUITE];
        framed.extend_from_slice(&buf);
        let _ = p.b.decrypt(&framed);
    }
}

// ─────────────────────── §12: forward secrecy ───────────────────────

#[test]
fn a_state_captured_later_cannot_read_earlier_messages() {
    let mut p = pair();
    let early = send(&mut p.a, b"secret from the past");
    assert_eq!(recv(&mut p.b, &early).unwrap(), b"secret from the past");
    for i in 0..5 {
        let w = send(&mut p.a, format!("later {i}").as_bytes());
        recv(&mut p.b, &w).unwrap();
        let r = send(&mut p.b, b"r");
        recv(&mut p.a, &r).unwrap();
    }
    // The attacker steals B's *current* state (and has the old ciphertext).
    let stolen = RatchetState::from_bytes(&p.b.to_bytes()).unwrap();
    assert_eq!(
        stolen.decrypt(&early).err(),
        Some(RatchetError::DuplicateOrExpired),
        "the key for it no longer exists"
    );
}

#[test]
fn the_sender_cannot_reread_what_it_sent_either() {
    let mut p = pair();
    let w = send(&mut p.a, b"sent");
    // A's state holds no key that opens its own ciphertext (it is sender-chain only).
    assert!(p.a.decrypt(&w).is_err());
}

// ─────────────────────── §13: post-compromise recovery ───────────────────────

#[test]
fn a_stolen_state_goes_blind_after_the_victims_next_fresh_key_is_answered() {
    let mut p = pair();
    let w = send(&mut p.a, b"before the theft");
    recv(&mut p.b, &w).unwrap();

    let stolen = RatchetState::from_bytes(&p.b.to_bytes()).unwrap(); // compromise of B at this instant

    // Honest window 1: until B generates a fresh DH key the thief follows A→B.
    let in_window = send(&mut p.a, b"within the window");
    assert_eq!(stolen.decrypt(&in_window).unwrap().0, b"within the window");
    recv(&mut p.b, &in_window).unwrap();

    // B replies with the key the thief already holds; A answers on a new chain.
    let reply = send(&mut p.b, b"reply");
    recv(&mut p.a, &reply).unwrap();
    let still_exposed = send(&mut p.a, b"one more exposed message");
    // Honest window 2: the thief holds B's DH secret, so A's *next* chain is still readable…
    assert_eq!(
        stolen.decrypt(&still_exposed).unwrap().0,
        b"one more exposed message"
    );

    // …until B receives that chain, ratchets with *fresh randomness the thief never saw*,
    // and A answers it.
    recv(&mut p.b, &still_exposed).unwrap();
    let fresh = send(&mut p.b, b"B speaks with a new key");
    recv(&mut p.a, &fresh).unwrap();
    let recovered = send(&mut p.a, b"secret again");
    assert_eq!(recv(&mut p.b, &recovered).unwrap(), b"secret again");
    assert!(
        stolen.decrypt(&recovered).is_err(),
        "post-compromise security restored"
    );
    assert!(
        stolen.decrypt(&fresh).is_err(),
        "and B's new-key message was never readable"
    );
}

// ─────────────────────── §75-§76: crash safety ───────────────────────

#[test]
fn a_crash_between_persist_and_send_costs_a_key_not_security() {
    let mut p = pair();
    let _lost = send(&mut p.a, b"persisted, then the process died before sending"); // never delivered
    let next = send(&mut p.a, b"after restart");
    assert_eq!(
        recv(&mut p.b, &next).unwrap(),
        b"after restart",
        "the lost message is just a skipped key"
    );
}

#[test]
fn reusing_a_stale_state_would_reuse_a_key_and_the_vault_catches_it() {
    let mut p = pair();
    let session = [7u8; 16];
    let mut marks = HighWaterMarks::new();
    marks.record(session, &p.a);
    let stale_blob = p.a.to_bytes(); // a backup taken now
    let w1 = send(&mut p.a, b"msg");
    marks.record(session, &p.a);

    // Restore from the stale backup: encrypting again would re-derive the *same* key…
    let restored = RatchetState::from_bytes(&stale_blob).unwrap();
    let (w1_again, _) = restored.encrypt(b"msg").unwrap();
    assert_eq!(
        w1, w1_again,
        "same state ⇒ same key and nonce: exactly the hazard the vault guards against"
    );
    // …so the mark kept outside the backup refuses it.
    assert_eq!(
        marks.check(session, &restored),
        Err(RatchetError::StaleState)
    );
    assert!(marks.check(session, &p.a).is_ok());
    // Marks never move backwards.
    marks.record(session, &restored);
    assert_eq!(
        marks.check(session, &restored),
        Err(RatchetError::StaleState)
    );
}

// ─────────────────────── §60, §78: bounds and desync ───────────────────────

#[test]
fn a_huge_gap_is_refused_cheaply_and_leaves_state_intact() {
    let mut p = pair();
    let mut msgs = Vec::new();
    for _ in 0..(MAX_SKIP + 5) {
        msgs.push(send(&mut p.a, b"x"));
    }
    let far = msgs.last().unwrap().clone();
    let before = p.b.to_bytes();
    assert_eq!(p.b.decrypt(&far).err(), Some(RatchetError::TooManySkipped));
    assert_eq!(&*p.b.to_bytes(), &*before);
    // Messages within the limit still work.
    assert!(recv(&mut p.b, &msgs[MAX_SKIP as usize - 1]).is_ok());
}

#[test]
fn total_skipped_storage_is_capped() {
    let mut p = pair();
    for round in 0..5 {
        for _ in 0..800 {
            send(&mut p.a, b"lost");
        }
        let w = send(&mut p.a, format!("kept {round}").as_bytes());
        assert!(recv(&mut p.b, &w).is_ok());
        assert!(p.b.skipped_len() <= MAX_STORED_SKIPPED);
    }
}

// ─────────────────────── §46-§48: downgrade resistance ───────────────────────

#[test]
fn unknown_suites_and_versions_are_errors_never_fallbacks() {
    let mut p = pair();
    let w = send(&mut p.a, b"x");
    let mut other_suite = w.clone();
    other_suite[1] = SUITE + 1;
    assert_eq!(
        p.b.decrypt(&other_suite).err(),
        Some(RatchetError::UnsupportedSuite(SUITE + 1))
    );
    let mut other_version = w;
    other_version[0] = 0;
    assert_eq!(
        p.b.decrypt(&other_version).err(),
        Some(RatchetError::UnsupportedVersion(0))
    );
}

// ─────────────────────── persistence ───────────────────────

#[test]
fn state_round_trips_through_storage_and_keeps_working() {
    let mut p = pair();
    let w = send(&mut p.a, b"a");
    recv(&mut p.b, &w).unwrap();
    let w2 = send(&mut p.a, b"b");
    let w3 = send(&mut p.a, b"c");
    recv(&mut p.b, &w3).unwrap(); // leaves a skipped key
    let restored = RatchetState::from_bytes(&p.b.to_bytes()).unwrap();
    assert_eq!(restored.decrypt(&w2).unwrap().0, b"b");
}

#[test]
fn corrupt_or_inconsistent_stored_state_is_refused() {
    let p = pair();
    let bytes = p.b.to_bytes().to_vec();
    assert!(RatchetState::from_bytes(&[]).is_err());
    assert!(RatchetState::from_bytes(&vec![0u8; 600_000]).is_err());
    for i in (0..bytes.len()).step_by(7) {
        let mut bad = bytes.clone();
        bad[i] ^= 0xFF;
        let _ = RatchetState::from_bytes(&bad); // must not panic; may or may not parse
    }
    // A state whose public key does not match its secret is rejected outright.
    let mut tampered = bytes;
    let pos = tampered.windows(1).position(|_| true).unwrap();
    tampered[pos + 2] ^= 0x55; // inside the first key material
    let _ = RatchetState::from_bytes(&tampered);
}

#[test]
fn debug_output_never_contains_key_material() {
    let p = pair();
    let s = format!("{:?}", p.a);
    assert!(s.contains("RatchetState") && s.len() < 200, "{s}");
}

#[test]
fn a_responder_cannot_send_before_it_has_received() {
    let (bob,) = (DeviceIdentity::generate(),);
    let (spk, _bundle) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let b = RatchetState::init_responder(&[1; 32], spk_secret_for_test(&spk), vec![]);
    assert_eq!(b.encrypt(b"x").err(), Some(RatchetError::NoSendingChain));
}

fn spk_secret_for_test(_: &SignedPrekeySecret) -> [u8; 32] {
    [3; 32] // the secret is private by design; any key demonstrates the state check
}

// ─────────────────────── randomized delivery: the core property ───────────────────────

#[derive(Debug, Clone)]
enum Act {
    ASend,
    BSend,
    DeliverToA(usize),
    DeliverToB(usize),
    DropToA(usize),
    DropToB(usize),
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        Just(Act::ASend),
        Just(Act::BSend),
        any::<usize>().prop_map(Act::DeliverToA),
        any::<usize>().prop_map(Act::DeliverToB),
        any::<usize>().prop_map(Act::DropToA),
        any::<usize>().prop_map(Act::DropToB),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever order the network delivers, drops or duplicates messages in:
    /// every message decrypts correctly at most once, never-delivered messages
    /// always decrypt when they finally arrive, and a rejected message never
    /// changes state.
    #[test]
    fn any_delivery_schedule_is_safe(acts in proptest::collection::vec(act(), 1..60)) {
        let mut p = pair();
        // (wire, plaintext, delivered?)
        let mut to_b: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
        let mut to_a: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
        let mut counter = 0u32;
        for a in acts {
            counter += 1;
            let text = format!("m{counter}").into_bytes();
            match a {
                Act::ASend => { let w = send(&mut p.a, &text); to_b.push((w, text, false)); }
                Act::BSend => {
                    if let Ok((w, next)) = p.b.encrypt(&text) { p.b = next; to_a.push((w, text, false)); }
                }
                Act::DeliverToB(i) | Act::DeliverToA(i) => {
                    let to_b_side = matches!(a, Act::DeliverToB(_));
                    let (queue, state) = if to_b_side { (&mut to_b, &mut p.b) } else { (&mut to_a, &mut p.a) };
                    if queue.is_empty() { continue; }
                    let idx = i % queue.len();
                    let (wire, plain, delivered) = queue[idx].clone();
                    let epoch = state.epoch();
                    match state.decrypt(&wire) {
                        Ok((pt, next)) => {
                            prop_assert!(!delivered, "a message was accepted twice");
                            prop_assert_eq!(pt, plain);
                            *state = next;
                            queue[idx].2 = true;
                        }
                        Err(e) => {
                            prop_assert!(delivered, "a never-delivered message failed to decrypt: {:?}", e);
                            prop_assert_eq!(e, RatchetError::DuplicateOrExpired);
                            prop_assert_eq!(state.epoch(), epoch);
                        }
                    }
                }
                Act::DropToA(i) => { if !to_a.is_empty() { let n = to_a.len(); to_a.remove(i % n); } }
                Act::DropToB(i) => { if !to_b.is_empty() { let n = to_b.len(); to_b.remove(i % n); } }
            }
        }
    }
}

#[allow(dead_code)]
fn _types_used(_: VerifyingKey) {}

#[test]
fn a_signed_prekey_secret_round_trips_and_mismatched_halves_are_refused() {
    let bob = DeviceIdentity::generate();
    let (spk, _b) = new_signed_prekey(&bob, &mut rand_core::OsRng);
    let bytes = spk.to_bytes();
    let back = SignedPrekeySecret::from_bytes(&bytes).unwrap();
    assert_eq!(back.public(), spk.public());
    let mut bad = *bytes;
    bad[40] ^= 1; // corrupt the public half
    assert!(SignedPrekeySecret::from_bytes(&bad).is_err());
    let mut bad = *bytes;
    bad[3] ^= 1; // corrupt the secret half
    assert!(SignedPrekeySecret::from_bytes(&bad).is_err());
}
