//! 1:1 sessions as two-member MLS groups, shaped for a delay-tolerant network.
//!
//! Why MLS for two people: one audited protocol gives **forward secrecy**
//! (epoch secrets are deleted as the epoch advances), **post-compromise
//! security** (a member's self-update commit injects fresh randomness) and —
//! with [`crate::PQ_CIPHERSUITE`] — **post-quantum confidentiality and
//! authentication** (hybrid ML-KEM-768 + X25519, ML-DSA-44), with no second
//! hand-written ratchet to audit.
//!
//! What stock MLS assumes and a DTN breaks, and what this module does:
//!
//! * *A delivery service orders commits.* Here two devices could commit in the
//!   same epoch while offline. Fix: **alternating turns** — the creator
//!   (leaf 0) may commit at even epochs, the other member at odd ones. Commits
//!   are then totally ordered by construction; there is nothing to
//!   tie-break or roll back. A commit at the wrong turn is rejected.
//! * *Messages arrive in order.* Messages from an epoch we have not reached
//!   yet are **deferred** (bounded) and replayed when the commit that opens
//!   that epoch arrives; late messages from a recent past epoch still decrypt
//!   ([`GroupParams::pairwise_dtn`]'s `max_past_epochs`), and the sender
//!   ratchet tolerates large reordering and gaps.
//! * *Peers are online to fetch key packages.* The responder publishes a
//!   key package ahead of time; the initiator can start sending immediately
//!   from the Welcome it produces.
//! * *Delivery is at-least-once.* A replayed commit is reported as
//!   [`PairwiseEvent::Duplicate`], not an error.
//!
//! Like a Double Ratchet, healing needs the *other* side's turn to come round,
//! so a silent peer delays (never breaks) post-compromise recovery.
//!
//! Not provided here: the deferred buffer is memory-only (a restart drops
//! it; senders retransmit), and Welcomes that arrive after the first
//! application message are the transport's job to reorder.

use openmls::prelude::OpenMlsProvider;
use thiserror::Error;

use crate::group::{GroupParams, IncomingMlsMessage, MlsGroupError, MlsGroupSession};
use crate::identity::{decode_key_package, MlsIdentity};
use crate::SignatureKeyPair;

/// Maximum messages held while waiting for the commit that opens their epoch.
const MAX_DEFERRED: usize = 256;

#[derive(Debug, Error)]
pub enum PairwiseError {
    #[error(transparent)]
    Group(#[from] MlsGroupError),
    #[error("bad peer key package: {0}")]
    KeyPackage(String),
    #[error("peer committed out of turn (epoch {0}); alternation violated")]
    OutOfTurnCommit(u64),
}

/// Which post-quantum trade-off a session uses. Fixed when the session is
/// created (it is the MLS ciphersuite); changing it means a new session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PairwiseProfile {
    /// Hybrid ML-KEM-768+X25519 key exchange, Ed25519 signatures. Messages
    /// stay ~250 B; fits BLE/DTN. **Default.**
    #[default]
    Compact,
    /// Adds ML-DSA-44 signatures: post-quantum authentication, ~10x larger
    /// messages and commits.
    FullPq,
}

impl PairwiseProfile {
    #[must_use]
    pub fn suite(self) -> openmls_traits::types::Ciphersuite {
        match self {
            Self::Compact => crate::PQ_CIPHERSUITE,
            Self::FullPq => crate::PQ_FULL_CIPHERSUITE,
        }
    }
}

/// Generate the identity (and publishable key package) for a 1:1 profile.
pub fn pairwise_identity<P: OpenMlsProvider>(
    device: siar_domain::DeviceId,
    provider: &P,
    profile: PairwiseProfile,
) -> Result<MlsIdentity, crate::MlsIdentityError> {
    crate::generate_identity_with_suite(device, provider, profile.suite())
}

/// When this side refreshes its keys.
#[derive(Debug, Clone, Copy)]
pub struct PairwisePolicy {
    /// Suite family; only read by the side that *starts* the session (the
    /// responder learns it from the Welcome).
    pub profile: PairwiseProfile,
    /// On its turn, commit before sending once this many messages have been
    /// sent since the last own commit. `1` = every first message of a turn.
    pub update_after_sent: u32,
}

impl Default for PairwisePolicy {
    fn default() -> Self {
        Self {
            profile: PairwiseProfile::Compact,
            update_after_sent: 32,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairwiseEvent {
    /// A decrypted application message.
    Message(Vec<u8>),
    /// The peer's commit was applied; keys moved to a new epoch.
    PeerCommit,
    /// From an epoch we have not reached; held until its commit arrives.
    Deferred,
    /// A commit for an epoch we already left (at-least-once delivery).
    Duplicate,
    /// A deferred message turned out to be undecryptable and was discarded.
    Dropped,
}

pub struct PairwiseSession<P: OpenMlsProvider> {
    inner: MlsGroupSession<P>,
    policy: PairwisePolicy,
    sent_since_commit: u32,
    deferred: Vec<(u64, Vec<u8>)>,
}

impl<P: OpenMlsProvider> PairwiseSession<P> {
    /// Initiator: open a session from the peer's published (PQ) key package.
    /// Returns the Welcome to deliver to the peer. `me` must come from
    /// [`crate::generate_identity_with_suite`] with [`crate::PQ_CIPHERSUITE`].
    pub fn start(
        provider: P,
        me: &MlsIdentity,
        peer_key_package: &[u8],
        policy: PairwisePolicy,
    ) -> Result<(Self, Vec<u8>), PairwiseError> {
        let kp = decode_key_package(peer_key_package)
            .map_err(|e| PairwiseError::KeyPackage(e.to_string()))?;
        let mut inner = MlsGroupSession::create_with(
            provider,
            me,
            GroupParams::pairwise_dtn_with(policy.profile.suite()),
        )?;
        let (_commit, welcome) = inner.add_member(&kp)?;
        Ok((Self::from_session(inner, policy), welcome))
    }

    /// Responder: join from the initiator's Welcome. `keys` and `provider`
    /// are the pair the published key package was generated with.
    pub fn accept(
        provider: P,
        keys: SignatureKeyPair,
        welcome: &[u8],
        policy: PairwisePolicy,
    ) -> Result<Self, PairwiseError> {
        let inner = MlsGroupSession::join_from_welcome_with(
            provider,
            keys,
            welcome,
            GroupParams::pairwise_dtn_with(policy.profile.suite()),
        )?;
        Ok(Self::from_session(inner, policy))
    }

    /// Wrap an already-open (e.g. reloaded from storage) two-member group.
    pub fn from_session(inner: MlsGroupSession<P>, policy: PairwisePolicy) -> Self {
        Self {
            inner,
            policy,
            sent_since_commit: 0,
            deferred: Vec::new(),
        }
    }

    #[must_use]
    pub fn session(&self) -> &MlsGroupSession<P> {
        &self.inner
    }

    fn is_initiator(&self) -> bool {
        self.inner.own_leaf_index() == 0
    }

    /// Whose commit is legal at `epoch`: creator at even, other at odd.
    fn my_turn_at(&self, epoch: u64) -> bool {
        epoch.is_multiple_of(2) == self.is_initiator()
    }

    /// Is it this side's turn to commit?
    #[must_use]
    pub fn my_turn(&self) -> bool {
        self.my_turn_at(self.inner.epoch())
    }

    /// Commit now if it is our turn (e.g. from a timer). `None` otherwise.
    /// The returned commit must be delivered **before** later messages, but
    /// the peer tolerates the opposite order.
    pub fn request_update(&mut self) -> Result<Option<Vec<u8>>, PairwiseError> {
        if !self.my_turn() {
            return Ok(None);
        }
        let c = self.inner.self_update()?;
        self.sent_since_commit = 0;
        Ok(Some(c))
    }

    /// Encrypt `plaintext`. Returns the wire messages to send, in order: an
    /// own commit first when one is due, then the message.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<Vec<u8>>, PairwiseError> {
        let mut out = Vec::with_capacity(2);
        if self.my_turn() && self.sent_since_commit >= self.policy.update_after_sent {
            out.extend(self.request_update()?);
        }
        out.push(self.inner.encrypt(plaintext)?);
        self.sent_since_commit = self.sent_since_commit.saturating_add(1);
        Ok(out)
    }

    /// Process one wire message from the peer.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Vec<PairwiseEvent>, PairwiseError> {
        let (epoch, is_commit) = MlsGroupSession::<P>::peek(bytes)?;
        let cur = self.inner.epoch();
        if epoch > cur {
            if self.deferred.len() >= MAX_DEFERRED {
                self.deferred.remove(0); // oldest first: the sender retransmits
            }
            self.deferred.push((epoch, bytes.to_vec()));
            return Ok(vec![PairwiseEvent::Deferred]);
        }
        if epoch < cur && is_commit {
            return Ok(vec![PairwiseEvent::Duplicate]);
        }
        let mut events = Vec::new();
        self.process(bytes, epoch, is_commit, &mut events)?;
        self.drain_deferred(&mut events);
        Ok(events)
    }

    fn process(
        &mut self,
        bytes: &[u8],
        epoch: u64,
        is_commit: bool,
        events: &mut Vec<PairwiseEvent>,
    ) -> Result<(), PairwiseError> {
        if is_commit && self.my_turn_at(epoch) {
            return Err(PairwiseError::OutOfTurnCommit(epoch));
        }
        match self.inner.process_incoming(bytes)? {
            IncomingMlsMessage::Application(b) => events.push(PairwiseEvent::Message(b)),
            IncomingMlsMessage::CommitMerged => events.push(PairwiseEvent::PeerCommit),
            IncomingMlsMessage::Ignored => {}
        }
        Ok(())
    }

    /// Replay held messages whose epoch has now been reached, oldest epoch
    /// first (a replayed commit can open the next batch).
    fn drain_deferred(&mut self, events: &mut Vec<PairwiseEvent>) {
        loop {
            let cur = self.inner.epoch();
            let Some(i) = self
                .deferred
                .iter()
                .enumerate()
                .filter(|(_, (e, _))| *e <= cur)
                .min_by_key(|(_, (e, _))| *e)
                .map(|(i, _)| i)
            else {
                return;
            };
            let (epoch, bytes) = self.deferred.remove(i);
            let is_commit = MlsGroupSession::<P>::peek(&bytes).is_ok_and(|(_, c)| c);
            if self.process(&bytes, epoch, is_commit, events).is_err() {
                events.push(PairwiseEvent::Dropped);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpenMlsRustCrypto, PQ_CIPHERSUITE, PQ_FULL_CIPHERSUITE};
    use siar_domain::DeviceId;

    type S = PairwiseSession<OpenMlsRustCrypto>;

    fn pair(policy: PairwisePolicy) -> (S, S) {
        let bob_provider = OpenMlsRustCrypto::default();
        let bob = pairwise_identity(DeviceId::new(), &bob_provider, policy.profile).unwrap();
        let kp = crate::encode_key_package(bob.key_package.key_package()).unwrap();
        let alice_provider = OpenMlsRustCrypto::default();
        let alice = pairwise_identity(DeviceId::new(), &alice_provider, policy.profile).unwrap();
        let (a, welcome) = S::start(alice_provider, &alice, &kp, policy).unwrap();
        let b = S::accept(bob_provider, bob.signature_keys.clone(), &welcome, policy).unwrap();
        (a, b)
    }

    fn msgs(ev: Vec<PairwiseEvent>) -> Vec<Vec<u8>> {
        ev.into_iter()
            .filter_map(|e| match e {
                PairwiseEvent::Message(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    fn deliver(to: &mut S, wire: &[Vec<u8>]) -> Vec<PairwiseEvent> {
        wire.iter().flat_map(|w| to.receive(w).unwrap()).collect()
    }

    #[test]
    fn the_session_runs_the_post_quantum_hybrid_suite() {
        let (a, b) = pair(PairwisePolicy::default());
        assert_eq!(a.session().ciphersuite(), PQ_CIPHERSUITE);
        assert_eq!(b.session().ciphersuite(), PQ_CIPHERSUITE);
        assert_eq!(a.session().member_count(), 2);
    }

    #[test]
    fn the_full_pq_profile_signs_with_ml_dsa_and_still_works_end_to_end() {
        let full = PairwisePolicy {
            profile: PairwiseProfile::FullPq,
            update_after_sent: 0,
        };
        let (mut a, mut b) = pair(full);
        assert_eq!(a.session().ciphersuite(), PQ_FULL_CIPHERSUITE);
        assert_eq!(
            PQ_FULL_CIPHERSUITE.signature_algorithm(),
            openmls_traits::types::SignatureScheme::MLDSA44
        );
        let w = a.seal(b"pq-signed").unwrap();
        assert_eq!(msgs(deliver(&mut b, &w)), vec![b"pq-signed".to_vec()]);
        let w = b.seal(b"reply").unwrap(); // b's turn: commit + message
        assert_eq!(msgs(deliver(&mut a, &w)), vec![b"reply".to_vec()]);
    }

    #[test]
    fn the_compact_profile_keeps_messages_small() {
        let (mut a, _b) = pair(PairwisePolicy::default());
        let w = a.seal(&[0u8; 100]).unwrap();
        assert!(w[0].len() < 400, "{} bytes", w[0].len());
    }

    #[test]
    fn the_initiator_can_send_before_the_responder_ever_replies() {
        let (mut a, mut b) = pair(PairwisePolicy::default());
        for i in 0..5u8 {
            let w = a.seal(&[i]).unwrap();
            assert_eq!(msgs(deliver(&mut b, &w)), vec![vec![i]]);
        }
    }

    #[test]
    fn commits_alternate_and_each_one_moves_both_sides_to_a_fresh_epoch() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 1,
            ..PairwisePolicy::default()
        });
        assert!(
            !a.my_turn() && b.my_turn(),
            "epoch 1 belongs to the responder"
        );
        assert!(
            a.request_update().unwrap().is_none(),
            "out of turn: no commit"
        );
        let c1 = b.request_update().unwrap().unwrap();
        let before = a.session().own_epoch_authenticator();
        deliver(&mut a, &[c1]);
        assert_ne!(before, a.session().own_epoch_authenticator());
        assert_eq!(
            a.session().own_epoch_authenticator(),
            b.session().own_epoch_authenticator()
        );
        assert!(a.my_turn() && !b.my_turn());
        let c2 = a.request_update().unwrap().unwrap();
        deliver(&mut b, &[c2]);
        assert_eq!(a.session().epoch(), b.session().epoch());
    }

    #[test]
    fn seal_puts_an_own_commit_first_once_the_policy_says_so() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 2,
            ..PairwisePolicy::default()
        });
        assert_eq!(b.seal(b"1").unwrap().len(), 1);
        assert_eq!(b.seal(b"2").unwrap().len(), 1);
        let third = b.seal(b"3").unwrap();
        assert_eq!(
            third.len(),
            2,
            "two sent since the last commit: commit, then message"
        );
        assert!(
            MlsGroupSession::<OpenMlsRustCrypto>::peek(&third[0])
                .unwrap()
                .1
        );
        let _ = &mut a;
    }

    #[test]
    fn a_message_that_overtakes_its_commit_is_held_then_delivered() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 0,
            ..PairwisePolicy::default()
        });
        // b's turn: seal → [commit, message-in-new-epoch]
        let wire = b.seal(b"after-commit").unwrap();
        assert_eq!(wire.len(), 2);
        // Deliver in the wrong order.
        assert_eq!(a.receive(&wire[1]).unwrap(), vec![PairwiseEvent::Deferred]);
        let ev = a.receive(&wire[0]).unwrap();
        assert_eq!(
            ev,
            vec![
                PairwiseEvent::PeerCommit,
                PairwiseEvent::Message(b"after-commit".to_vec())
            ]
        );
    }

    #[test]
    fn a_late_message_from_the_previous_epoch_still_decrypts() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 0,
            ..PairwisePolicy::default()
        });
        let old = a.seal(b"sent-before-the-commit").unwrap(); // epoch 1
        let new = b.seal(b"b-commit").unwrap(); // commit → epoch 2
        deliver(&mut a, &new);
        // b only now receives a's old-epoch message, after its own commit.
        assert_eq!(
            msgs(deliver(&mut b, &old)),
            vec![b"sent-before-the-commit".to_vec()]
        );
    }

    #[test]
    fn a_replayed_commit_is_a_duplicate_not_an_error() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 0,
            ..PairwisePolicy::default()
        });
        let wire = b.seal(b"m").unwrap();
        deliver(&mut a, &wire);
        assert_eq!(a.receive(&wire[0]).unwrap(), vec![PairwiseEvent::Duplicate]);
    }

    #[test]
    fn a_commit_out_of_turn_is_rejected() {
        let (mut a, mut b) = pair(PairwisePolicy::default());
        // Epoch 1 is b's turn; make *a* commit anyway (a buggy/malicious peer).
        let rogue = a.inner.self_update().unwrap();
        let err = b.receive(&rogue).unwrap_err();
        assert!(matches!(err, PairwiseError::OutOfTurnCommit(1)), "{err:?}");
    }

    #[test]
    fn heavy_reordering_within_the_dtn_window_loses_nothing() {
        let (mut a, mut b) = pair(PairwisePolicy::default());
        let wire: Vec<Vec<u8>> = (0..200u32)
            .map(|i| a.seal(&i.to_be_bytes()).unwrap().remove(0))
            .collect();
        let mut got = Vec::new();
        for w in wire.iter().rev() {
            got.extend(msgs(b.receive(w).unwrap()));
        }
        assert_eq!(got.len(), 200);
    }

    #[test]
    fn deferred_messages_are_bounded() {
        let (mut a, mut b) = pair(PairwisePolicy {
            update_after_sent: 0,
            ..PairwisePolicy::default()
        });
        let wire = b.seal(b"x").unwrap();
        deliver(&mut a, &wire); // a now at epoch 2
                                // a's turn at epoch 2: build future-epoch traffic a can't open yet.
        let mut c = a.request_update().unwrap().unwrap();
        let future = a.seal(b"later").unwrap().pop().unwrap();
        for _ in 0..(MAX_DEFERRED + 10) {
            assert_eq!(b.receive(&future).unwrap(), vec![PairwiseEvent::Deferred]);
        }
        assert_eq!(b.deferred.len(), MAX_DEFERRED);
        c.clear();
    }
}
