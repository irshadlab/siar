//! §13-15: capability advertisement envelope — authentication binding
//! and replay resistance.
//!
//! The advertised set is bound to (a) the authenticated session, (b) a
//! fresh nonce and a per-session strictly increasing counter, and (c) its
//! canonical [`CapabilitySetHash`]. A recorded advertisement therefore
//! can't be replayed into another session, replayed later in the same
//! session, or paired with a different set. Signature checking is
//! delegated to [`AdvertisementVerifier`] so this crate stays independent
//! of Part 02's device-certificate types (the verifier is where the
//! device key is checked).

use std::collections::VecDeque;

use crate::hash::CapabilitySetHash;
use crate::set::CapabilitySet;
use crate::transcript::HandshakeNonce;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisementEnvelope {
    pub session_binding: [u8; 32],
    pub nonce: HandshakeNonce,
    pub counter: u64,
    pub issued_at_millis: u64,
    pub set_hash: CapabilitySetHash,
    pub signature: Vec<u8>,
}

/// Domain-separated digest the signer signs.
pub fn signing_digest(env: &AdvertisementEnvelope) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("siar capability advertisement v1");
    h.update(&env.session_binding);
    h.update(env.nonce.as_bytes());
    h.update(&env.counter.to_le_bytes());
    h.update(&env.issued_at_millis.to_le_bytes());
    h.update(env.set_hash.as_bytes());
    *h.finalize().as_bytes()
}

pub trait AdvertisementVerifier {
    /// True iff `signature` is a valid device signature over `digest`.
    fn verify(&self, digest: &[u8; 32], signature: &[u8]) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdvertisementError {
    #[error("advertisement is bound to a different session")]
    WrongSession,
    #[error("advertised set does not match its committed hash")]
    SetHashMismatch,
    #[error("advertisement timestamp outside the allowed clock skew")]
    Stale,
    #[error("advertisement replayed (nonce seen or counter not increasing)")]
    Replay,
    #[error("signature invalid")]
    BadSignature,
}

pub const NONCE_WINDOW: usize = 64;

/// Per-session replay state.
#[derive(Debug, Default)]
pub struct ReplayGuard {
    last_counter: Option<u64>,
    recent_nonces: VecDeque<[u8; 16]>,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Checks cheap, attacker-controlled-input properties first and the
/// signature last; state in `guard` only advances on full success, so a
/// forged envelope can't poison the replay window.
pub fn verify_advertisement(
    env: &AdvertisementEnvelope,
    set: &CapabilitySet,
    expected_session: &[u8; 32],
    now_millis: u64,
    max_skew_millis: u64,
    guard: &mut ReplayGuard,
    verifier: &dyn AdvertisementVerifier,
) -> Result<(), AdvertisementError> {
    // Hashes of public values; constant-time compare via blake3::Hash.
    if blake3::Hash::from_bytes(env.session_binding) != blake3::Hash::from_bytes(*expected_session)
    {
        return Err(AdvertisementError::WrongSession);
    }
    if CapabilitySetHash::of(set) != env.set_hash {
        return Err(AdvertisementError::SetHashMismatch);
    }
    if env.issued_at_millis.abs_diff(now_millis) > max_skew_millis {
        return Err(AdvertisementError::Stale);
    }
    if guard.last_counter.is_some_and(|c| env.counter <= c)
        || guard.recent_nonces.contains(env.nonce.as_bytes())
    {
        return Err(AdvertisementError::Replay);
    }
    if !verifier.verify(&signing_digest(env), &env.signature) {
        return Err(AdvertisementError::BadSignature);
    }
    guard.last_counter = Some(env.counter);
    if guard.recent_nonces.len() >= NONCE_WINDOW {
        guard.recent_nonces.pop_front();
    }
    guard.recent_nonces.push_back(*env.nonce.as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test verifier: "signature" is the digest itself.
    struct EchoVerifier;
    impl AdvertisementVerifier for EchoVerifier {
        fn verify(&self, digest: &[u8; 32], sig: &[u8]) -> bool {
            sig == digest
        }
    }

    const SESSION: [u8; 32] = [7; 32];

    fn env(counter: u64, at: u64, set: &CapabilitySet) -> AdvertisementEnvelope {
        let mut e = AdvertisementEnvelope {
            session_binding: SESSION,
            nonce: HandshakeNonce::generate(),
            counter,
            issued_at_millis: at,
            set_hash: CapabilitySetHash::of(set),
            signature: vec![],
        };
        e.signature = signing_digest(&e).to_vec();
        e
    }

    fn run(
        e: &AdvertisementEnvelope,
        set: &CapabilitySet,
        now: u64,
        g: &mut ReplayGuard,
    ) -> Result<(), AdvertisementError> {
        verify_advertisement(e, set, &SESSION, now, 1_000, g, &EchoVerifier)
    }

    #[test]
    fn valid_then_replay_is_rejected() {
        let set = CapabilitySet::new();
        let mut g = ReplayGuard::new();
        let e = env(1, 100, &set);
        assert_eq!(run(&e, &set, 100, &mut g), Ok(()));
        assert_eq!(run(&e, &set, 100, &mut g), Err(AdvertisementError::Replay));
        let older = env(1, 100, &set); // fresh nonce, same counter
        assert_eq!(
            run(&older, &set, 100, &mut g),
            Err(AdvertisementError::Replay)
        );
        assert_eq!(run(&env(2, 100, &set), &set, 100, &mut g), Ok(()));
    }

    #[test]
    fn wrong_session_wrong_set_stale_and_bad_signature() {
        let set = CapabilitySet::new();
        let mut g = ReplayGuard::new();
        let mut e = env(1, 100, &set);
        let other = [8u8; 32];
        assert_eq!(
            verify_advertisement(&e, &set, &other, 100, 1_000, &mut g, &EchoVerifier),
            Err(AdvertisementError::WrongSession)
        );
        use crate::descriptor::*;
        use crate::id::*;
        use crate::version::CapabilityVersion;
        let mut different = CapabilitySet::new();
        different
            .insert(CapabilityDescriptor::new(
                CapabilityId::new(CapabilityNamespace::Files, 1),
                CapabilityVersion::new(1, 0),
                CapabilityRequirement::Optional,
                CapabilityParameters::None,
            ))
            .unwrap();
        assert_eq!(
            run(&e, &different, 100, &mut g),
            Err(AdvertisementError::SetHashMismatch)
        );
        assert_eq!(run(&e, &set, 5_000, &mut g), Err(AdvertisementError::Stale));
        e.signature[0] ^= 1;
        assert_eq!(
            run(&e, &set, 100, &mut g),
            Err(AdvertisementError::BadSignature)
        );
    }

    #[test]
    fn forged_envelope_does_not_advance_replay_state() {
        let set = CapabilitySet::new();
        let mut g = ReplayGuard::new();
        let mut forged = env(99, 100, &set);
        forged.signature = vec![0; 32];
        assert_eq!(
            run(&forged, &set, 100, &mut g),
            Err(AdvertisementError::BadSignature)
        );
        assert_eq!(
            run(&env(1, 100, &set), &set, 100, &mut g),
            Ok(()),
            "counter 1 still acceptable"
        );
    }
}
