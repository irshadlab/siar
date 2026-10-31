//! `SecureSessions`: the stateful 1:1 E2EE facade (§111's `SecureMessenger`
//! role) over `siar-ratchet`, with the ordering rules **enforced by
//! construction** rather than left to each caller:
//!
//! * **Send:** the next state is made durable *before* the wire bytes are
//!   returned. If saving fails, no message is released and nothing advanced
//!   (§75-§76).
//! * **Receive:** decrypt is pure; the caller's `commit` (store the plaintext,
//!   idempotently) runs *before* the new state is saved. A crash between the
//!   two means the same message decrypts again on retransmission and `commit`
//!   sees it twice — which `insert_if_new` already tolerates (§77).
//! * **One operation per session at a time** (a per-session async lock), so
//!   two tasks can never both load the same state and then both advance it.
//! * **No silent session replacement.** A first-flight that arrives while a
//!   healthy session exists is *not* acted on automatically: replaying a
//!   captured first-flight must not be able to roll a conversation back.
//!   `receive` reports [`SessionError::ResetNeeded`]; replacing the session is
//!   the explicit [`SecureSessions::accept_reset`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use ed25519_dalek::VerifyingKey;
use siar_crypto::DeviceIdentity;
use siar_ratchet::handshake::identity_dh_public;
use siar_ratchet::{
    initiate, respond, HandshakeError, InitialHeader, PrekeyBundle, RatchetError, RatchetState,
};
use zeroize::Zeroizing;

use crate::prekeys::{PrekeyError, PrekeyManager};
use crate::vault::{SessionId, SessionVault, VaultError};

const MAGIC: u8 = 0x53; // 'S'
const WIRE_VERSION: u8 = 1;
const FLAG_INITIAL: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error(transparent)]
    Ratchet(#[from] RatchetError),
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
    #[error(transparent)]
    Prekey(#[from] PrekeyError),
    #[error("no session with this peer yet and no prekey bundle to start one")]
    NeedsIntroduction,
    #[error("message needs a session we do not have")]
    NoSession,
    /// The peer used a prekey we no longer hold; they must fetch a fresh bundle.
    #[error("the peer used a prekey that has expired; ask them to refresh and resend")]
    PrekeyExpired,
    #[error("the handshake does not match the peer we expected")]
    PeerIdentityMismatch,
    #[error("message is malformed")]
    Malformed,
    #[error("duplicate message")]
    Duplicate,
    /// A first-flight arrived but a session already exists and could not read
    /// it. Not acted on automatically (see module docs).
    #[error("the peer started a new session; confirm before replacing the current one")]
    ResetNeeded,
    #[error("storing the message failed: {0}")]
    Commit(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitError(pub String);

impl From<&str> for CommitError {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// Who we are talking to, as far as the session layer needs to know.
#[derive(Debug, Clone, Copy)]
pub struct PeerRef {
    pub identity_dh_public: [u8; 32],
}

/// What is needed to *start* a session with someone new.
#[derive(Clone, Copy)]
pub struct Introduction<'a> {
    pub bundle: &'a PrekeyBundle,
    /// From the trust store (pairing/QR/SAS) — **not** from the bundle.
    pub trusted_signer: &'a VerifyingKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveKind {
    /// An established session carried on.
    Continued,
    /// First message of a new session from this peer.
    Established,
    /// The session was replaced by an explicit `accept_reset`.
    Reestablished,
}

pub struct Received {
    pub plaintext: Zeroizing<Vec<u8>>,
    pub kind: ReceiveKind,
}

pub struct SecureSessions {
    me: Arc<DeviceIdentity>,
    my_ik: [u8; 32],
    vault: SessionVault,
    prekeys: PrekeyManager,
    locks: Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>,
}

struct ParsedWire<'a> {
    initial: Option<InitialHeader>,
    ratchet: &'a [u8],
}

fn parse(wire: &[u8]) -> Result<ParsedWire<'_>, SessionError> {
    let [magic, version, flags, rest @ ..] = wire else {
        return Err(SessionError::Malformed);
    };
    if *magic != MAGIC || *version != WIRE_VERSION || flags & !FLAG_INITIAL != 0 {
        return Err(SessionError::Malformed);
    }
    if flags & FLAG_INITIAL != 0 {
        let (init, ratchet) = InitialHeader::decode(rest).ok_or(SessionError::Malformed)?;
        Ok(ParsedWire {
            initial: Some(init),
            ratchet,
        })
    } else {
        Ok(ParsedWire {
            initial: None,
            ratchet: rest,
        })
    }
}

fn build(initial: Option<&[u8]>, ratchet: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(3 + initial.map_or(0, <[u8]>::len) + ratchet.len());
    w.extend_from_slice(&[
        MAGIC,
        WIRE_VERSION,
        if initial.is_some() { FLAG_INITIAL } else { 0 },
    ]);
    if let Some(i) = initial {
        w.extend_from_slice(i);
    }
    w.extend_from_slice(ratchet);
    w
}

impl SecureSessions {
    #[must_use]
    pub fn new(me: Arc<DeviceIdentity>, vault: SessionVault, prekeys: PrekeyManager) -> Self {
        let my_ik = identity_dh_public(&me);
        Self {
            me,
            my_ik,
            vault,
            prekeys,
            locks: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn prekeys(&self) -> &PrekeyManager {
        &self.prekeys
    }

    #[must_use]
    pub const fn my_identity_dh(&self) -> [u8; 32] {
        self.my_ik
    }

    fn lock_for(&self, id: SessionId) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.locks.lock().unwrap_or_else(PoisonError::into_inner);
        // Opportunistic cleanup so the map tracks only sessions in use.
        if m.len() > 256 {
            m.retain(|_, l| Arc::strong_count(l) > 1);
        }
        m.entry(id).or_default().clone()
    }

    fn session_id(&self, peer: &PeerRef) -> SessionId {
        SessionId::for_pair(&self.my_ik, &peer.identity_dh_public)
    }

    /// Encrypt `plaintext` for `peer`, starting a session from `intro` if
    /// there is none. The returned bytes are safe to transmit: the state that
    /// produced them is already durable.
    pub async fn encrypt(
        &self,
        peer: &PeerRef,
        intro: Option<Introduction<'_>>,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, SessionError> {
        let sid = self.session_id(peer);
        let lock = self.lock_for(sid);
        let _held = lock.lock().await;

        let (state, header, fresh) = match self.vault.load(&sid)? {
            Some(s) => (s, self.vault.pending_init(&sid)?, false),
            None => {
                let intro = intro.ok_or(SessionError::NeedsIntroduction)?;
                if intro.bundle.identity_dh_public != peer.identity_dh_public {
                    return Err(SessionError::PeerIdentityMismatch);
                }
                let (st, h) = initiate(
                    &self.me,
                    intro.bundle,
                    intro.trusted_signer,
                    &mut rand_core::OsRng,
                )?;
                (st, Some(h.encode().to_vec()), true)
            }
        };
        let (ratchet, next) = state.encrypt(plaintext)?;
        // Header first: a crash after it leaves a harmless unused header; the
        // other order could leave a session whose first flight lacks one.
        if fresh {
            if let Some(h) = &header {
                self.vault.set_pending_init(&sid, h)?;
            }
        }
        self.vault.save(&sid, &next)?; // durable BEFORE the bytes leave
        Ok(build(header.as_deref(), &ratchet))
    }

    /// Decrypt a message from `peer`. `commit` must store the plaintext
    /// **idempotently**; it runs before the new state is saved.
    pub async fn receive<F>(
        &self,
        peer: &PeerRef,
        wire: &[u8],
        commit: F,
    ) -> Result<Received, SessionError>
    where
        F: FnOnce(&[u8]) -> Result<(), CommitError>,
    {
        let w = parse(wire)?;
        let sid = self.session_id(peer);
        let lock = self.lock_for(sid);
        let _held = lock.lock().await;

        match self.vault.load(&sid)? {
            Some(state) => match state.decrypt(w.ratchet) {
                Ok((pt, next)) => {
                    commit(&pt).map_err(|e| SessionError::Commit(e.0))?;
                    self.vault.save(&sid, &next)?;
                    self.vault.clear_pending_init(&sid)?;
                    Ok(Received {
                        plaintext: Zeroizing::new(pt),
                        kind: ReceiveKind::Continued,
                    })
                }
                Err(RatchetError::DuplicateOrExpired) => Err(SessionError::Duplicate),
                Err(RatchetError::AuthenticationFailed) if w.initial.is_some() => {
                    Err(SessionError::ResetNeeded)
                }
                Err(e) => Err(e.into()),
            },
            None => {
                let init = w.initial.ok_or(SessionError::NoSession)?;
                let (pt, next) = self.open_initial(peer, &init, w.ratchet)?;
                commit(&pt).map_err(|e| SessionError::Commit(e.0))?;
                self.vault.save(&sid, &next)?;
                Ok(Received {
                    plaintext: Zeroizing::new(pt),
                    kind: ReceiveKind::Established,
                })
            }
        }
    }

    /// Replace the current session with the one the peer just started. Call
    /// only after the caller has decided that is what should happen (user
    /// confirmation, or a policy that knows the peer reinstalled).
    pub async fn accept_reset<F>(
        &self,
        peer: &PeerRef,
        wire: &[u8],
        commit: F,
    ) -> Result<Received, SessionError>
    where
        F: FnOnce(&[u8]) -> Result<(), CommitError>,
    {
        let w = parse(wire)?;
        let init = w.initial.ok_or(SessionError::Malformed)?;
        let sid = self.session_id(peer);
        let lock = self.lock_for(sid);
        let _held = lock.lock().await;
        let (pt, next) = self.open_initial(peer, &init, w.ratchet)?;
        commit(&pt).map_err(|e| SessionError::Commit(e.0))?;
        self.vault.reestablish(&sid, &next)?;
        Ok(Received {
            plaintext: Zeroizing::new(pt),
            kind: ReceiveKind::Reestablished,
        })
    }

    fn open_initial(
        &self,
        peer: &PeerRef,
        init: &InitialHeader,
        ratchet: &[u8],
    ) -> Result<(Vec<u8>, RatchetState), SessionError> {
        if init.initiator_identity_dh != peer.identity_dh_public {
            return Err(SessionError::PeerIdentityMismatch);
        }
        let spk = self
            .prekeys
            .secret_for(&init.responder_prekey)
            .ok_or(SessionError::PrekeyExpired)?;
        let state = respond(&self.me, &spk, init)?;
        Ok(state.decrypt(ratchet)?)
    }

    /// Forget a session (e.g. the user deleted the contact).
    pub fn forget(&self, peer: &PeerRef) -> Result<(), SessionError> {
        Ok(self.vault.delete(&self.session_id(peer))?)
    }

    #[must_use]
    pub fn has_session(&self, peer: &PeerRef) -> bool {
        matches!(self.vault.load(&self.session_id(peer)), Ok(Some(_)))
    }
}
