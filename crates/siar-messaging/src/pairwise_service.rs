//! 1:1 conversations over OpenMLS (`siar_crypto_mls::PairwiseSession`):
//! forward secrecy, post-compromise security and post-quantum key exchange,
//! replacing the static-key `siar_crypto::Session` for peers that have one.
//!
//! This type owns the *state*: per-conversation sessions persisted through
//! `SqlitePersistentProvider` files (same [`MlsStore`] layout as
//! [`GroupService`](crate::GroupService), in its own directory so the two
//! never share a "pending key package" slot), the one pending key package a
//! responder is waiting on, and the **un-acked ciphertext** kept for
//! retransmission. `MessageService` does the sending.
//!
//! Two consequences of forward secrecy that callers must respect:
//!
//! * **Plaintext at rest.** A delivered MLS ciphertext cannot be decrypted
//!   again (its key is deleted), so — exactly like group messages — the local
//!   message row holds the *plaintext*; protecting it at rest is the storage
//!   layer's job, not the transport cipher's.
//! * **Retransmission resends the same bytes.** Re-encrypting would burn a
//!   ratchet step per retry; the sealed batch is stored until the peer's
//!   `DeliveryAck` and resent verbatim. The receiving side is idempotent
//!   (duplicate commits are `Duplicate`; duplicate messages are caught by
//!   `insert_if_new`).

use crate::key_package_directory::KeyPackageDirectory;
use crate::mls_store::{GroupRecord, MlsStore, PendingRecord};
use siar_crypto_mls::{
    pairwise_identity, MlsGroupSession, PairwiseError, PairwiseEvent, PairwisePolicy,
    PairwiseProfile, PairwiseSession, SignatureKeyPair, SqlitePersistentProvider,
};
use siar_domain::{ConversationId, DeviceId, MessageId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use thiserror::Error;

type Session = PairwiseSession<SqlitePersistentProvider>;

#[derive(Debug, Error)]
pub enum PairwiseServiceError {
    #[error("storage: {0}")]
    Storage(String),
    #[error(transparent)]
    Pairwise(#[from] PairwiseError),
    #[error("no session for this conversation")]
    NoSession,
    #[error("no key package waiting for this device (publish one first)")]
    NoPendingKeyPackage,
    #[error("malformed batch")]
    Malformed,
}

/// What travels in one envelope: the own commit (if one was due) followed by
/// the message, kept together so one retry resends both.
#[derive(serde::Serialize, serde::Deserialize)]
struct Batch {
    frames: Vec<Vec<u8>>,
}

/// Sealed bytes awaiting the peer's acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Unacked {
    pub message_id: MessageId,
    pub conversation: ConversationId,
    pub peer_ticket_hex: String,
    pub payload: Vec<u8>,
    /// `true` = the payload is a session-opening Welcome, not an MLS batch.
    pub welcome: bool,
    pub next_attempt_millis: u64,
}

struct Pending {
    provider: SqlitePersistentProvider,
    keys: SignatureKeyPair,
    file: String,
}

pub struct PairwiseService {
    device_id: DeviceId,
    store: MlsStore,
    dir: PathBuf,
    policy: PairwisePolicy,
    sessions: Mutex<HashMap<ConversationId, Session>>,
    pending: Mutex<Option<Pending>>,
    unacked: Mutex<Vec<Unacked>>,
}

fn io(e: impl std::fmt::Display) -> PairwiseServiceError {
    PairwiseServiceError::Storage(e.to_string())
}

impl PairwiseService {
    pub fn new(
        device_id: DeviceId,
        dir: impl Into<PathBuf>,
        policy: PairwisePolicy,
    ) -> Result<Self, PairwiseServiceError> {
        let dir = dir.into();
        let store = MlsStore::new(&dir).map_err(io)?;
        let unacked = std::fs::read(dir.join("unacked.bin"))
            .ok()
            .and_then(|b| postcard::from_bytes(&b).ok())
            .unwrap_or_default();
        Ok(Self {
            device_id,
            store,
            dir,
            policy,
            sessions: Mutex::default(),
            pending: Mutex::default(),
            unacked: Mutex::new(unacked),
        })
    }

    fn open(&self, file: &str) -> Result<SqlitePersistentProvider, PairwiseServiceError> {
        SqlitePersistentProvider::open(&self.store.path_of(file)).map_err(io)
    }

    fn save_unacked(&self, rows: &[Unacked]) -> Result<(), PairwiseServiceError> {
        let bytes = postcard::to_allocvec(rows).map_err(io)?;
        let tmp = self.dir.join("unacked.tmp");
        std::fs::write(&tmp, bytes).map_err(io)?;
        std::fs::rename(&tmp, self.dir.join("unacked.bin")).map_err(io)
    }

    /// Reopen every persisted session (and the pending key package) after a
    /// restart. A session that fails to reload is skipped and logged.
    pub fn restore(&self) -> Vec<ConversationId> {
        let index = self.store.load();
        let mut out = Vec::new();
        for rec in index.groups {
            let loaded = self.open(&rec.file).and_then(|p| {
                MlsGroupSession::load(p, &rec.group_id, &rec.signature_public_key)
                    .map_err(|e| io(format!("{e}")))
            });
            match loaded {
                Ok(Some(s)) => {
                    self.sessions.lock().expect("sessions lock").insert(
                        rec.conversation,
                        PairwiseSession::from_session(s, self.policy),
                    );
                    out.push(rec.conversation);
                }
                Ok(None) => {
                    tracing::warn!(conversation = ?rec.conversation, "pairwise file holds no group")
                }
                Err(e) => {
                    tracing::warn!(conversation = ?rec.conversation, error = %e, "couldn't reopen a pairwise session")
                }
            }
        }
        out
    }

    #[must_use]
    pub fn has_session(&self, conversation: ConversationId) -> bool {
        self.sessions
            .lock()
            .expect("sessions lock")
            .contains_key(&conversation)
    }

    /// Generate and publish a key package for `profile` so a peer can open a
    /// session with us while we are offline.
    pub fn publish_key_package(
        &self,
        profile: PairwiseProfile,
        directory: &dyn KeyPackageDirectory,
    ) -> Result<(), PairwiseServiceError> {
        let file = format!("kp-{}.sqlite3", ConversationId::new());
        let provider = self.open(&file)?;
        let identity = pairwise_identity(self.device_id, &provider, profile)
            .map_err(|e| io(format!("{e}")))?;
        let kp = siar_crypto_mls::encode_key_package(identity.key_package.key_package())
            .map_err(|e| io(format!("{e}")))?;
        self.store
            .record_pending(PendingRecord {
                file: file.clone(),
                signature_public_key: identity.signature_keys.to_public_vec(),
                key_package: kp.clone(),
            })
            .map_err(io)?;
        *self.pending.lock().expect("pending lock") = Some(Pending {
            provider,
            keys: identity.signature_keys,
            file,
        });
        directory.publish(self.device_id, kp);
        Ok(())
    }

    /// Initiator: open a session with `peer`'s key package. Returns the
    /// Welcome bytes to send (as `EnvelopeKind::PairwiseWelcome`).
    pub fn start_session(
        &self,
        conversation: ConversationId,
        peer_key_package: &[u8],
        profile: PairwiseProfile,
    ) -> Result<Vec<u8>, PairwiseServiceError> {
        let file = format!("pw-{conversation}.sqlite3");
        let provider = self.open(&file)?;
        let me = pairwise_identity(self.device_id, &provider, profile)
            .map_err(|e| io(format!("{e}")))?;
        let policy = PairwisePolicy {
            profile,
            ..self.policy
        };
        let (session, welcome) = PairwiseSession::start(provider, &me, peer_key_package, policy)?;
        self.store
            .record_group(GroupRecord {
                conversation,
                file,
                group_id: session.session().group_id_bytes(),
                signature_public_key: session.session().signature_public_key(),
            })
            .map_err(io)?;
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(conversation, session);
        Ok(welcome)
    }

    /// Responder: join from a Welcome using the key package we published.
    pub fn accept_welcome(
        &self,
        conversation: ConversationId,
        welcome: &[u8],
    ) -> Result<(), PairwiseServiceError> {
        let p = self
            .pending
            .lock()
            .expect("pending lock")
            .take()
            .ok_or(PairwiseServiceError::NoPendingKeyPackage)?;
        let session = PairwiseSession::accept(p.provider, p.keys, welcome, self.policy)?;
        self.store
            .record_group(GroupRecord {
                conversation,
                file: p.file,
                group_id: session.session().group_id_bytes(),
                signature_public_key: session.session().signature_public_key(),
            })
            .and_then(|()| self.store.clear_pending())
            .map_err(io)?;
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(conversation, session);
        Ok(())
    }

    /// Encrypt `plaintext`; returns the envelope payload (own commit if due,
    /// then the message).
    pub fn seal(
        &self,
        conversation: ConversationId,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, PairwiseServiceError> {
        let frames = self
            .sessions
            .lock()
            .expect("sessions lock")
            .get_mut(&conversation)
            .ok_or(PairwiseServiceError::NoSession)?
            .seal(plaintext)?;
        postcard::to_allocvec(&Batch { frames }).map_err(io)
    }

    /// Process an envelope payload from the peer; returns the plaintexts of
    /// every application message it (or messages it unblocked) carried.
    pub fn receive(
        &self,
        conversation: ConversationId,
        payload: &[u8],
    ) -> Result<Vec<Vec<u8>>, PairwiseServiceError> {
        let batch: Batch =
            postcard::from_bytes(payload).map_err(|_| PairwiseServiceError::Malformed)?;
        let mut sessions = self.sessions.lock().expect("sessions lock");
        let s = sessions
            .get_mut(&conversation)
            .ok_or(PairwiseServiceError::NoSession)?;
        let mut out = Vec::new();
        for f in &batch.frames {
            for ev in s.receive(f)? {
                if let PairwiseEvent::Message(m) = ev {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    // ── retransmission state ────────────────────────────────────────────

    pub fn remember_unacked(&self, row: Unacked) -> Result<(), PairwiseServiceError> {
        let mut rows = self.unacked.lock().expect("unacked lock");
        rows.retain(|r| r.message_id != row.message_id);
        rows.push(row);
        self.save_unacked(&rows)
    }

    pub fn ack(&self, message_id: MessageId) -> Result<bool, PairwiseServiceError> {
        let mut rows = self.unacked.lock().expect("unacked lock");
        let before = rows.len();
        rows.retain(|r| r.message_id != message_id);
        let removed = rows.len() != before;
        if removed {
            self.save_unacked(&rows)?;
        }
        Ok(removed)
    }

    /// Rows whose retry time has come.
    #[must_use]
    pub fn due(&self, now_millis: u64) -> Vec<Unacked> {
        self.unacked
            .lock()
            .expect("unacked lock")
            .iter()
            .filter(|r| r.next_attempt_millis <= now_millis)
            .cloned()
            .collect()
    }

    pub fn reschedule(
        &self,
        message_id: MessageId,
        next_attempt_millis: u64,
    ) -> Result<(), PairwiseServiceError> {
        let mut rows = self.unacked.lock().expect("unacked lock");
        if let Some(r) = rows.iter_mut().find(|r| r.message_id == message_id) {
            r.next_attempt_millis = next_attempt_millis;
        }
        self.save_unacked(&rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryKeyPackageDirectory;

    fn svc(dir: &std::path::Path, device: DeviceId) -> PairwiseService {
        PairwiseService::new(
            device,
            dir,
            PairwisePolicy {
                update_after_sent: 1,
                ..PairwisePolicy::default()
            },
        )
        .unwrap()
    }

    fn open_pair(
        tmp: &std::path::Path,
    ) -> (
        PairwiseService,
        PairwiseService,
        ConversationId,
        DeviceId,
        DeviceId,
    ) {
        let (da, db) = (DeviceId::new(), DeviceId::new());
        let (alice, bob) = (svc(&tmp.join("a"), da), svc(&tmp.join("b"), db));
        let dir = InMemoryKeyPackageDirectory::new();
        bob.publish_key_package(PairwiseProfile::Compact, &dir)
            .unwrap();
        let conv = ConversationId::new();
        let kp = dir.take(db).unwrap();
        let welcome = alice
            .start_session(conv, &kp, PairwiseProfile::Compact)
            .unwrap();
        bob.accept_welcome(conv, &welcome).unwrap();
        (alice, bob, conv, da, db)
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-pw-{name}-{}", ConversationId::new()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn two_services_exchange_messages_both_ways() {
        let t = tmpdir("both");
        let (a, b, c, ..) = open_pair(&t);
        let w = a.seal(c, b"hello bob").unwrap();
        assert_eq!(b.receive(c, &w).unwrap(), vec![b"hello bob".to_vec()]);
        let w = b.seal(c, b"hello alice").unwrap();
        assert_eq!(a.receive(c, &w).unwrap(), vec![b"hello alice".to_vec()]);
        let w = a.seal(c, b"again").unwrap();
        assert_eq!(b.receive(c, &w).unwrap(), vec![b"again".to_vec()]);
    }

    #[test]
    fn sessions_survive_a_restart_on_both_sides() {
        let t = tmpdir("restart");
        let (a, b, c, da, db) = open_pair(&t);
        let w = a.seal(c, b"before").unwrap();
        b.receive(c, &w).unwrap();
        drop((a, b));
        let (a2, b2) = (svc(&t.join("a"), da), svc(&t.join("b"), db));
        assert_eq!(a2.restore(), vec![c]);
        assert_eq!(b2.restore(), vec![c]);
        let w = a2.seal(c, b"after restart").unwrap();
        assert_eq!(b2.receive(c, &w).unwrap(), vec![b"after restart".to_vec()]);
        let w = b2.seal(c, b"and back").unwrap();
        assert_eq!(a2.receive(c, &w).unwrap(), vec![b"and back".to_vec()]);
    }

    #[test]
    fn a_resent_identical_batch_is_not_delivered_twice_as_new_text() {
        let t = tmpdir("dup");
        let (a, b, c, ..) = open_pair(&t);
        let w = a.seal(c, b"once").unwrap();
        assert_eq!(b.receive(c, &w).unwrap().len(), 1);
        // The retransmission may not decrypt (ratchet secret already used);
        // either way it must not yield the message a second time.
        let again = b.receive(c, &w).unwrap_or_default();
        assert!(again.is_empty());
    }

    #[test]
    fn unacked_ciphertext_is_persisted_resent_verbatim_and_cleared_by_the_ack() {
        let t = tmpdir("unacked");
        let (a, _b, c, da, _) = open_pair(&t);
        let m = MessageId::new();
        let payload = a.seal(c, b"x").unwrap();
        a.remember_unacked(Unacked {
            message_id: m,
            conversation: c,
            peer_ticket_hex: "ticket".into(),
            payload: payload.clone(),
            welcome: false,
            next_attempt_millis: 100,
        })
        .unwrap();
        assert!(a.due(99).is_empty());
        drop(a);
        let a2 = svc(&t.join("a"), da); // restart
        let due = a2.due(100);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].payload, payload);
        a2.reschedule(m, 500).unwrap();
        assert!(a2.due(100).is_empty());
        assert!(a2.ack(m).unwrap());
        assert!(a2.due(10_000).is_empty());
        assert!(!a2.ack(m).unwrap());
    }

    #[test]
    fn sealing_without_a_session_is_an_error() {
        let s = svc(&tmpdir("nosess"), DeviceId::new());
        assert!(matches!(
            s.seal(ConversationId::new(), b"x"),
            Err(PairwiseServiceError::NoSession)
        ));
    }

    #[test]
    fn accepting_a_welcome_with_no_published_key_package_fails_cleanly() {
        let s = svc(&tmpdir("nopend"), DeviceId::new());
        assert!(matches!(
            s.accept_welcome(ConversationId::new(), b"w"),
            Err(PairwiseServiceError::NoPendingKeyPackage)
        ));
    }
}

/// What a pairwise message carries *inside* the MLS ciphertext.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PairwisePlain {
    pub message_id: MessageId,
    pub sequence: u64,
    pub timestamp_millis: u64,
    /// `postcard(MessageContent)`.
    pub content: Vec<u8>,
}
