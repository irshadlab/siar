//! Encrypted session state with stale-restore protection (§24, §50, §103).
//!
//! Two stores on purpose:
//!
//! * `states` — the encrypted ratchet state. **Included in backups.**
//! * `marks`  — the per-session high-water mark of the highest epoch ever
//!   saved. **Must live outside the backup set** (a different directory
//!   excluded from backup, the key store, a monotonic counter). Restoring an
//!   old `states` leaves `marks` ahead of it, and `load` refuses the stale
//!   state instead of reusing message keys.
//!
//! Write order matters: state first, then the mark. A crash between them
//! leaves the mark *behind* the state (harmless). The reverse order could
//! leave the mark ahead of the only state and kill the session.

use std::io;
use std::sync::Arc;

use siar_ratchet::{RatchetError, RatchetState};

use crate::cipher::{AtRestKey, CipherError};
use crate::store::BlobStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub [u8; 16]);

impl SessionId {
    /// Order-independent id of a pair of identity DH public keys: both sides
    /// compute the same value, and it reveals nothing beyond the keys.
    #[must_use]
    pub fn for_pair(a: &[u8; 32], b: &[u8; 32]) -> Self {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut h = blake3::Hasher::new();
        h.update(b"siar/session-id/v1");
        h.update(lo);
        h.update(hi);
        let mut id = [0u8; 16];
        id.copy_from_slice(&h.finalize().as_bytes()[..16]);
        Self(id)
    }

    #[must_use]
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("storage error: {0:?}")]
    Store(io::ErrorKind),
    #[error(transparent)]
    Cipher(#[from] CipherError),
    #[error(transparent)]
    State(#[from] RatchetError),
    #[error(
        "stored session state is older than state already used; the session must be re-established"
    )]
    Stale,
}

impl From<io::Error> for VaultError {
    fn from(e: io::Error) -> Self {
        Self::Store(e.kind())
    }
}

pub struct SessionVault {
    states: Arc<dyn BlobStore>,
    marks: Arc<dyn BlobStore>,
    key: AtRestKey,
}

fn aad(kind: &str, id: &SessionId) -> Vec<u8> {
    let mut a = format!("siar-session/{kind}/").into_bytes();
    a.extend_from_slice(&id.0);
    a
}

impl SessionVault {
    #[must_use]
    pub fn new(states: Arc<dyn BlobStore>, marks: Arc<dyn BlobStore>, key: AtRestKey) -> Self {
        Self { states, marks, key }
    }

    fn state_key(id: &SessionId) -> String {
        format!("sessions/{}.state", id.hex())
    }
    fn init_key(id: &SessionId) -> String {
        format!("sessions/{}.init", id.hex())
    }
    fn mark_key(id: &SessionId) -> String {
        format!("hw/{}", id.hex())
    }

    fn mark(&self, id: &SessionId) -> Result<u64, VaultError> {
        Ok(self
            .marks
            .get(&Self::mark_key(id))?
            .and_then(|b| b.try_into().ok())
            .map_or(0, u64::from_le_bytes))
    }

    /// Load and validate. Refuses a state older than anything saved before.
    pub fn load(&self, id: &SessionId) -> Result<Option<RatchetState>, VaultError> {
        let Some(blob) = self.states.get(&Self::state_key(id))? else {
            return Ok(None);
        };
        let plain = self.key.open(&aad("state", id), &blob)?;
        let state = RatchetState::from_bytes(&plain)?;
        if state.epoch() < self.mark(id)? {
            return Err(VaultError::Stale);
        }
        Ok(Some(state))
    }

    /// Persist `state`. **Call before releasing the message that produced it.**
    pub fn save(&self, id: &SessionId, state: &RatchetState) -> Result<(), VaultError> {
        let mark = self.mark(id)?;
        if state.epoch() < mark {
            return Err(VaultError::Stale);
        }
        let blob = self.key.seal(&aad("state", id), &state.to_bytes());
        self.states.put(&Self::state_key(id), &blob)?;
        // After the state is durable, never before (see module docs).
        self.marks
            .put(&Self::mark_key(id), &state.epoch().max(mark).to_le_bytes())?;
        Ok(())
    }

    /// The only way to move a session *backwards*: an explicit re-establishment
    /// (a fresh handshake), after which epochs start again from zero.
    pub fn reestablish(&self, id: &SessionId, state: &RatchetState) -> Result<(), VaultError> {
        self.marks.delete(&Self::mark_key(id))?;
        self.states.delete(&Self::state_key(id))?;
        self.states.delete(&Self::init_key(id))?;
        self.save(id, state)
    }

    pub fn delete(&self, id: &SessionId) -> Result<(), VaultError> {
        self.states.delete(&Self::state_key(id))?;
        self.states.delete(&Self::init_key(id))?;
        self.marks.delete(&Self::mark_key(id))?;
        Ok(())
    }

    /// The initiator keeps sending its handshake header until the peer
    /// answers (so a lost first message cannot strand the session).
    pub fn set_pending_init(&self, id: &SessionId, header: &[u8]) -> Result<(), VaultError> {
        self.states.put(
            &Self::init_key(id),
            &self.key.seal(&aad("init", id), header),
        )?;
        Ok(())
    }

    pub fn pending_init(&self, id: &SessionId) -> Result<Option<Vec<u8>>, VaultError> {
        let Some(blob) = self.states.get(&Self::init_key(id))? else {
            return Ok(None);
        };
        Ok(Some(self.key.open(&aad("init", id), &blob)?.to_vec()))
    }

    pub fn clear_pending_init(&self, id: &SessionId) -> Result<(), VaultError> {
        self.states.delete(&Self::init_key(id))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{FaultyStore, MemBlobStore};
    use siar_crypto::DeviceIdentity;
    use siar_ratchet::{initiate, new_signed_prekey, respond};

    fn live_pair() -> (RatchetState, RatchetState) {
        let (a, b) = (DeviceIdentity::generate(), DeviceIdentity::generate());
        let (spk, bundle) = new_signed_prekey(&b, &mut rand_core::OsRng);
        let (sa, init) = initiate(&a, &bundle, &b.verifying_key(), &mut rand_core::OsRng).unwrap();
        (sa, respond(&b, &spk, &init).unwrap())
    }

    fn vault() -> (SessionVault, Arc<MemBlobStore>, Arc<MemBlobStore>) {
        let (s, m) = (Arc::new(MemBlobStore::new()), Arc::new(MemBlobStore::new()));
        (
            SessionVault::new(s.clone(), m.clone(), AtRestKey::from_bytes([7; 32])),
            s,
            m,
        )
    }

    #[test]
    fn session_ids_agree_from_both_sides() {
        let (a, b) = ([1u8; 32], [2u8; 32]);
        assert_eq!(SessionId::for_pair(&a, &b), SessionId::for_pair(&b, &a));
        assert_ne!(
            SessionId::for_pair(&a, &b),
            SessionId::for_pair(&a, &[3u8; 32])
        );
    }

    #[test]
    fn state_round_trips_encrypted_and_is_not_stored_in_clear() {
        let (v, states, _m) = vault();
        let (a, _b) = live_pair();
        let id = SessionId([9; 16]);
        assert!(v.load(&id).unwrap().is_none());
        v.save(&id, &a).unwrap();
        let back = v.load(&id).unwrap().unwrap();
        assert_eq!(back.epoch(), a.epoch());
        let raw = states
            .get(&format!("sessions/{}.state", id.hex()))
            .unwrap()
            .unwrap();
        let plain = a.to_bytes();
        assert!(
            !raw.windows(32).any(|w| plain.windows(32).any(|p| p == w)),
            "no 32-byte run of the plaintext state appears in the stored blob"
        );
    }

    #[test]
    fn restoring_an_old_backup_is_detected_and_refused() {
        let (v, states, _marks) = vault();
        let (mut a, _b) = live_pair();
        let id = SessionId([1; 16]);
        v.save(&id, &a).unwrap();
        let backup = states.snapshot(); // the state at backup time
        for _ in 0..3 {
            a = a.encrypt(b"x").unwrap().1;
        }
        v.save(&id, &a).unwrap();
        // The device is restored from the old backup: states roll back, the
        // mark (kept elsewhere) does not.
        states.restore(backup);
        assert!(matches!(v.load(&id), Err(VaultError::Stale)));
        assert!(
            matches!(v.save(&id, &live_pair().0), Err(VaultError::Stale) | Ok(())),
            "a fresh session has epoch 0 and is also below the mark"
        );
    }

    #[test]
    fn a_state_cannot_be_moved_to_another_session() {
        let (v, states, _m) = vault();
        let (a, _b) = live_pair();
        let (id1, id2) = (SessionId([1; 16]), SessionId([2; 16]));
        v.save(&id1, &a).unwrap();
        let blob = states
            .get(&format!("sessions/{}.state", id1.hex()))
            .unwrap()
            .unwrap();
        states
            .put(&format!("sessions/{}.state", id2.hex()), &blob)
            .unwrap();
        assert!(matches!(v.load(&id2), Err(VaultError::Cipher(_))));
    }

    #[test]
    fn a_failed_state_write_leaves_the_mark_alone() {
        let f = Arc::new(FaultyStore::new(MemBlobStore::new(), "state"));
        let marks = Arc::new(MemBlobStore::new());
        let v = SessionVault::new(f.clone(), marks.clone(), AtRestKey::from_bytes([7; 32]));
        let (a, _b) = live_pair();
        let id = SessionId([3; 16]);
        let a2 = a.encrypt(b"x").unwrap().1;
        v.save(&id, &a).unwrap();
        f.fail_next(1);
        assert!(matches!(v.save(&id, &a2), Err(VaultError::Store(_))));
        assert_eq!(
            v.load(&id).unwrap().unwrap().epoch(),
            a.epoch(),
            "the previous state is intact"
        );
        assert!(v.save(&id, &a2).is_ok(), "and a retry works");
    }

    #[test]
    fn reestablish_is_the_only_way_back_to_epoch_zero() {
        let (v, _s, _m) = vault();
        let (mut a, _b) = live_pair();
        let id = SessionId([4; 16]);
        for _ in 0..4 {
            a = a.encrypt(b"x").unwrap().1;
        }
        v.save(&id, &a).unwrap();
        let (fresh, _) = live_pair();
        assert!(matches!(v.save(&id, &fresh), Err(VaultError::Stale)));
        v.reestablish(&id, &fresh).unwrap();
        assert_eq!(v.load(&id).unwrap().unwrap().epoch(), 0);
    }

    #[test]
    fn pending_init_is_stored_encrypted_and_clearable() {
        let (v, _s, _m) = vault();
        let id = SessionId([5; 16]);
        assert!(v.pending_init(&id).unwrap().is_none());
        v.set_pending_init(&id, &[7u8; 96]).unwrap();
        assert_eq!(v.pending_init(&id).unwrap().unwrap(), vec![7u8; 96]);
        v.clear_pending_init(&id).unwrap();
        assert!(v.pending_init(&id).unwrap().is_none());
    }
}
