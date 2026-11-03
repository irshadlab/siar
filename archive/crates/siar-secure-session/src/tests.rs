//! Behavioural tests for `SecureSessions`: every rule its docs promise.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use siar_crypto::DeviceIdentity;
use siar_ratchet::handshake::identity_dh_public;

use crate::cipher::AtRestKey;
use crate::prekeys::{PrekeyManager, PrekeyPolicy};
use crate::sessions::*;
use crate::store::{FaultyStore, MemBlobStore};
use crate::vault::SessionVault;

const DAY: u64 = 86_400_000;

/// One device with its three stores; `restart()` rebuilds the facade over the
/// same storage, exactly like a process restart.
struct Node {
    id: Arc<DeviceIdentity>,
    states: Arc<FaultyStore<MemBlobStore>>,
    marks: Arc<MemBlobStore>,
    prekey_store: Arc<MemBlobStore>,
    s: Arc<SecureSessions>,
}

fn build(
    id: &Arc<DeviceIdentity>,
    states: &Arc<FaultyStore<MemBlobStore>>,
    marks: &Arc<MemBlobStore>,
    pk: &Arc<MemBlobStore>,
    now: u64,
) -> Arc<SecureSessions> {
    let vault = SessionVault::new(
        states.clone(),
        marks.clone(),
        AtRestKey::derive(id, "session-state"),
    );
    let prekeys = PrekeyManager::open(
        pk.clone(),
        AtRestKey::derive(id, "prekeys"),
        id,
        PrekeyPolicy::default(),
        now,
        &mut rand_core::OsRng,
    )
    .unwrap();
    Arc::new(SecureSessions::new(id.clone(), vault, prekeys))
}

impl Node {
    fn new() -> Self {
        let id = Arc::new(DeviceIdentity::generate());
        let states = Arc::new(FaultyStore::new(MemBlobStore::new(), "state"));
        let (marks, pk) = (Arc::new(MemBlobStore::new()), Arc::new(MemBlobStore::new()));
        let s = build(&id, &states, &marks, &pk, 0);
        Self {
            id,
            states,
            marks,
            prekey_store: pk,
            s,
        }
    }
    fn restart(&mut self) {
        self.s = build(&self.id, &self.states, &self.marks, &self.prekey_store, 0);
    }
    fn peer_ref(&self) -> PeerRef {
        PeerRef {
            identity_dh_public: identity_dh_public(&self.id),
        }
    }
    fn bundle(&self) -> siar_ratchet::PrekeyBundle {
        self.s.prekeys().current_bundle()
    }
}

fn ok(_: &[u8]) -> Result<(), CommitError> {
    Ok(())
}

async fn a_to_b(a: &Node, b: &Node, msg: &[u8]) -> Vec<u8> {
    let bundle = b.bundle();
    let signer = b.id.verifying_key();
    a.s.encrypt(
        &b.peer_ref(),
        Some(Introduction {
            bundle: &bundle,
            trusted_signer: &signer,
        }),
        msg,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_whole_conversation_survives_restarts_of_both_sides() {
    let (mut a, mut b) = (Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"hello").await;
    let r = b.s.receive(&a.peer_ref(), &w, ok).await.unwrap();
    assert_eq!(
        (&**r.plaintext, r.kind),
        (&b"hello"[..], ReceiveKind::Established)
    );
    for i in 0..30u32 {
        a.restart();
        b.restart();
        let m = format!("b→a {i}");
        let w =
            b.s.encrypt(&a.peer_ref(), None, m.as_bytes())
                .await
                .unwrap();
        assert_eq!(
            &**a.s.receive(&b.peer_ref(), &w, ok).await.unwrap().plaintext,
            m.as_bytes()
        );
        let m = format!("a→b {i}");
        let w =
            a.s.encrypt(&b.peer_ref(), None, m.as_bytes())
                .await
                .unwrap();
        let r = b.s.receive(&a.peer_ref(), &w, ok).await.unwrap();
        assert_eq!(
            (&**r.plaintext, r.kind),
            (m.as_bytes(), ReceiveKind::Continued)
        );
    }
}

#[tokio::test]
async fn a_lost_first_flight_does_not_strand_the_session() {
    let (a, b) = (Node::new(), Node::new());
    let m1 = a_to_b(&a, &b, b"one").await;
    let m2 = a.s.encrypt(&b.peer_ref(), None, b"two").await.unwrap();
    assert_eq!(m1[2] & 1, 1);
    assert_eq!(
        m2[2] & 1,
        1,
        "until the peer answers, every message repeats the handshake header"
    );
    // m1 is lost; m2 alone establishes the session…
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &m2, ok).await.unwrap().plaintext,
        b"two"
    );
    // …and m1 turns up later and still decrypts.
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &m1, ok).await.unwrap().plaintext,
        b"one"
    );
    // Once B has answered and A has received it, the header is dropped.
    let reply = b.s.encrypt(&a.peer_ref(), None, b"hi").await.unwrap();
    a.s.receive(&b.peer_ref(), &reply, ok).await.unwrap();
    let m3 = a.s.encrypt(&b.peer_ref(), None, b"three").await.unwrap();
    assert_eq!(m3[2] & 1, 0);
}

#[tokio::test]
async fn nothing_is_released_if_the_state_cannot_be_saved() {
    let (a, b) = (Node::new(), Node::new());
    let (bundle, signer) = (b.bundle(), b.id.verifying_key());
    let intro = Introduction {
        bundle: &bundle,
        trusted_signer: &signer,
    };
    a.states.fail_next(1);
    assert!(
        matches!(
            a.s.encrypt(&b.peer_ref(), Some(intro), b"x").await,
            Err(SessionError::Vault(_))
        ),
        "§76: no state, no message"
    );
    assert!(!a.s.has_session(&b.peer_ref()));
    // Retrying works, and the receiver sees exactly one first flight.
    let w = a.s.encrypt(&b.peer_ref(), Some(intro), b"x").await.unwrap();
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &w, ok).await.unwrap().plaintext,
        b"x"
    );
}

#[tokio::test]
async fn a_crash_between_saving_and_sending_costs_a_key_not_security() {
    let (a, b) = (Node::new(), Node::new());
    let w1 = a_to_b(&a, &b, b"first").await;
    b.s.receive(&a.peer_ref(), &w1, ok).await.unwrap();
    let _never_sent =
        a.s.encrypt(&b.peer_ref(), None, b"state saved, process died")
            .await
            .unwrap();
    let w3 =
        a.s.encrypt(&b.peer_ref(), None, b"after restart")
            .await
            .unwrap();
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &w3, ok).await.unwrap().plaintext,
        b"after restart"
    );
}

#[tokio::test]
async fn the_message_is_committed_before_the_state_and_a_failed_save_is_recoverable() {
    let (a, b) = (Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"important").await;
    let stored: Arc<Mutex<HashSet<Vec<u8>>>> = Arc::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let commit = |stored: Arc<Mutex<HashSet<Vec<u8>>>>, calls: Arc<AtomicUsize>| {
        move |pt: &[u8]| {
            calls.fetch_add(1, Ordering::SeqCst);
            stored.lock().unwrap().insert(pt.to_vec()); // idempotent, like insert_if_new
            Ok(())
        }
    };
    b.states.fail_next(1); // the process "dies" after commit, before the state is saved
    assert!(matches!(
        b.s.receive(&a.peer_ref(), &w, commit(stored.clone(), calls.clone()))
            .await,
        Err(SessionError::Vault(_))
    ));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the plaintext was stored first"
    );
    // The sender retransmits; the same message decrypts again and is stored idempotently.
    let r =
        b.s.receive(&a.peer_ref(), &w, commit(stored.clone(), calls.clone()))
            .await
            .unwrap();
    assert_eq!(&**r.plaintext, b"important");
    assert_eq!(
        (calls.load(Ordering::SeqCst), stored.lock().unwrap().len()),
        (2, 1),
        "seen twice, stored once"
    );
}

#[tokio::test]
async fn a_failing_commit_leaves_the_session_untouched_so_the_retransmission_works() {
    let (a, b) = (Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"x").await;
    let r =
        b.s.receive(&a.peer_ref(), &w, |_| Err(CommitError::from("disk full")))
            .await;
    assert!(matches!(r, Err(SessionError::Commit(_))));
    assert!(!b.s.has_session(&a.peer_ref()));
    assert!(b.s.receive(&a.peer_ref(), &w, ok).await.is_ok());
}

#[tokio::test]
async fn restoring_an_old_backup_of_the_state_is_refused_not_obeyed() {
    let (a, b) = (Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"1").await;
    b.s.receive(&a.peer_ref(), &w, ok).await.unwrap();
    let backup = a.states.inner.snapshot(); // taken after message 1
    for i in 0..3 {
        a.s.encrypt(&b.peer_ref(), None, format!("{i}").as_bytes())
            .await
            .unwrap();
    }
    a.states.inner.restore(backup); // device restored from backup; the mark store was not
    let r = a.s.encrypt(&b.peer_ref(), None, b"would reuse a key").await;
    assert!(
        matches!(r, Err(SessionError::Vault(crate::VaultError::Stale))),
        "{:?}",
        r.err()
    );
}

#[tokio::test]
async fn concurrent_sends_never_share_a_key() {
    let (a, b) = (Node::new(), Node::new());
    let first = a_to_b(&a, &b, b"first").await;
    b.s.receive(&a.peer_ref(), &first, ok).await.unwrap();
    let (a_s, peer) = (a.s.clone(), b.peer_ref());
    let tasks: Vec<_> = (0..40)
        .map(|i| {
            let (s, p) = (a_s.clone(), peer);
            tokio::spawn(async move {
                s.encrypt(&p, None, format!("m{i}").as_bytes())
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut wires = Vec::new();
    for t in tasks {
        wires.push(t.await.unwrap());
    }
    // Deliver in reverse order: every one still decrypts, each exactly once.
    let mut got = HashSet::new();
    for w in wires.iter().rev() {
        let r = b.s.receive(&a.peer_ref(), w, ok).await.unwrap();
        assert!(got.insert(r.plaintext.to_vec()));
    }
    assert_eq!(got.len(), 40);
}

#[tokio::test]
async fn replays_and_mismatched_identities_are_refused() {
    let (a, b, mallory) = (Node::new(), Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"once").await;
    b.s.receive(&a.peer_ref(), &w, ok).await.unwrap();
    assert!(matches!(
        b.s.receive(&a.peer_ref(), &w, ok).await,
        Err(SessionError::Duplicate)
    ));
    // A first flight whose header claims A but is processed as if from Mallory (or vice versa).
    let c = Node::new();
    let w2 = a_to_b(&c, &b, b"from c").await;
    assert!(matches!(
        b.s.receive(&mallory.peer_ref(), &w2, ok).await,
        Err(SessionError::PeerIdentityMismatch)
    ));
}

#[tokio::test]
async fn a_replayed_first_flight_cannot_roll_back_a_healthy_session() {
    let (a, b) = (Node::new(), Node::new());
    let w = a_to_b(&a, &b, b"hello").await;
    b.s.receive(&a.peer_ref(), &w, ok).await.unwrap();
    let reply = b.s.encrypt(&a.peer_ref(), None, b"hi").await.unwrap();
    a.s.receive(&b.peer_ref(), &reply, ok).await.unwrap();

    // An attacker (or a confused peer) delivers a *different* first flight claiming to be A.
    let a_wiped = Node {
        s: build(
            &a.id,
            &Arc::new(FaultyStore::new(MemBlobStore::new(), "state")),
            &Arc::new(MemBlobStore::new()),
            &a.prekey_store,
            0,
        ),
        ..clone_shell(&a)
    };
    let stranger_start = a_to_b(&a_wiped, &b, b"new session").await;
    let r = b.s.receive(&a.peer_ref(), &stranger_start, ok).await;
    assert!(
        matches!(r, Err(SessionError::ResetNeeded)),
        "must be reported, not obeyed"
    );

    // The original conversation is untouched.
    let m =
        a.s.encrypt(&b.peer_ref(), None, b"still here")
            .await
            .unwrap();
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &m, ok).await.unwrap().plaintext,
        b"still here"
    );

    // Only an explicit decision replaces it.
    let r =
        b.s.accept_reset(&a.peer_ref(), &stranger_start, ok)
            .await
            .unwrap();
    assert_eq!(
        (&**r.plaintext, r.kind),
        (&b"new session"[..], ReceiveKind::Reestablished)
    );
    let m2 = a_wiped
        .s
        .encrypt(&b.peer_ref(), None, b"on the new session")
        .await
        .unwrap();
    assert_eq!(
        &**b.s.receive(&a.peer_ref(), &m2, ok).await.unwrap().plaintext,
        b"on the new session"
    );
}

fn clone_shell(n: &Node) -> Node {
    Node {
        id: n.id.clone(),
        states: n.states.clone(),
        marks: n.marks.clone(),
        prekey_store: n.prekey_store.clone(),
        s: n.s.clone(),
    }
}

#[tokio::test]
async fn a_stale_prekey_gets_a_clear_error_and_a_fresh_bundle_works() {
    let a = Node::new();
    let b = Node::new();
    let old_bundle = b.bundle();
    // B rotates well past the retention window.
    b.s.prekeys()
        .rotate_if_due(&b.id, 8 * DAY, &mut rand_core::OsRng)
        .unwrap();
    b.s.prekeys()
        .rotate_if_due(&b.id, 40 * DAY, &mut rand_core::OsRng)
        .unwrap();
    let signer = b.id.verifying_key();
    let w =
        a.s.encrypt(
            &b.peer_ref(),
            Some(Introduction {
                bundle: &old_bundle,
                trusted_signer: &signer,
            }),
            b"late",
        )
        .await
        .unwrap();
    assert!(matches!(
        b.s.receive(&a.peer_ref(), &w, ok).await,
        Err(SessionError::PrekeyExpired)
    ));
    // A, having refreshed the bundle, starts over.
    a.s.forget(&b.peer_ref()).unwrap();
    let fresh = b.bundle();
    let w =
        a.s.encrypt(
            &b.peer_ref(),
            Some(Introduction {
                bundle: &fresh,
                trusted_signer: &signer,
            }),
            b"late",
        )
        .await
        .unwrap();
    assert!(b.s.receive(&a.peer_ref(), &w, ok).await.is_ok());
}

#[tokio::test]
async fn starting_a_session_needs_a_trusted_bundle_for_the_right_peer() {
    let (a, b, c) = (Node::new(), Node::new(), Node::new());
    assert!(matches!(
        a.s.encrypt(&b.peer_ref(), None, b"x").await,
        Err(SessionError::NeedsIntroduction)
    ));
    // C's bundle offered as if it were B's.
    let (bundle, signer) = (c.bundle(), c.id.verifying_key());
    let r =
        a.s.encrypt(
            &b.peer_ref(),
            Some(Introduction {
                bundle: &bundle,
                trusted_signer: &signer,
            }),
            b"x",
        )
        .await;
    assert!(matches!(r, Err(SessionError::PeerIdentityMismatch)));
    // A genuine bundle but checked against the wrong trusted signer.
    let (bundle, other_signer) = (b.bundle(), c.id.verifying_key());
    let r =
        a.s.encrypt(
            &b.peer_ref(),
            Some(Introduction {
                bundle: &bundle,
                trusted_signer: &other_signer,
            }),
            b"x",
        )
        .await;
    assert!(matches!(r, Err(SessionError::Handshake(_))));
}

#[tokio::test]
async fn garbage_on_the_wire_is_malformed_never_a_panic() {
    let (a, b) = (Node::new(), Node::new());
    let mut x = 0xABCD_u64;
    for len in 0..300usize {
        let buf: Vec<u8> = (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        assert!(b.s.receive(&a.peer_ref(), &buf, ok).await.is_err());
        let mut framed = vec![0x53, 1, 1];
        framed.extend_from_slice(&buf);
        assert!(b.s.receive(&a.peer_ref(), &framed, ok).await.is_err());
    }
}
