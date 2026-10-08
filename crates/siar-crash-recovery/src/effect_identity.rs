//! §59 "Duplicate Effect Prevention" and §60 "Idempotent Receiver".
//!
//! §59: every external effect has a stable identity and recovery
//! retries *with the same id*. [`Effect`] makes that structural —
//! its only retry path, [`Effect::retry`], keeps the id and bumps an
//! attempt counter; no function here mints a fresh id for an existing
//! effect. §60: the receiver tolerates duplicates of the five listed
//! kinds, built on this crate's existing
//! [`crate::messaging_recovery::Deduplicator`] rather than a second
//! dedup mechanism.

use crate::chunk_recovery::TransferId;
use crate::messaging_recovery::Deduplicator;
use serde::{Deserialize, Serialize};
use siar_domain::MessageId;
use siar_dtn_bundle::BundleId;

/// §59's fourth example. No such id exists upstream yet (checked), so
/// this crate defines the minimal opaque one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceEventId(uuid::Uuid);

impl DeviceEventId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for DeviceEventId {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceEventId {
    #[cfg(test)]
    pub(crate) fn new_for_tests(n: u16) -> Self {
        Self(uuid::Uuid::from_u128(n as u128 + 1))
    }

    pub const fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

/// Stable identity of one external effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectId {
    Message(MessageId),
    Transfer(TransferId),
    Bundle(BundleId),
    DeviceEvent(DeviceEventId),
}

impl EffectId {
    /// Total, stable ordering key (kind tag, then id bytes) so recovery
    /// output never depends on hash-map iteration order (§131).
    pub fn sort_key(&self) -> (u8, [u8; 16]) {
        match self {
            EffectId::Message(m) => (0, *m.as_uuid().as_bytes()),
            EffectId::Transfer(t) => (1, *t.as_uuid().as_bytes()),
            EffectId::Bundle(b) => (2, *b.as_bytes()),
            EffectId::DeviceEvent(d) => (3, *d.as_uuid().as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    id: EffectId,
    attempt: u32,
}

impl Effect {
    pub const fn first_attempt(id: EffectId) -> Self {
        Self { id, attempt: 1 }
    }

    pub const fn id(&self) -> EffectId {
        self.id
    }

    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Recovery's retry: same id, next attempt. Saturates rather than
    /// wrapping so a pathological retry loop can never revisit
    /// "attempt 1".
    pub const fn retry(&self) -> Self {
        Self {
            id: self.id,
            attempt: self.attempt.saturating_add(1),
        }
    }
}

/// §60's five duplicate-tolerant kinds, each with the identity that
/// makes a duplicate recognisable. A chunk is identified by its
/// transfer *and* index; a receipt by the message it is for plus a
/// small kind tag (delivered/read/…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiveKey {
    Message(MessageId),
    Chunk { transfer: TransferId, index: u32 },
    Bundle(BundleId),
    Receipt { message: MessageId, kind: u8 },
    DeviceEvent(DeviceEventId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// First time: process it.
    Process,
    /// Duplicate: acknowledge as success, do not apply again.
    Duplicate,
}

#[derive(Debug, Clone, Default)]
pub struct IdempotentReceiver {
    seen: Deduplicator<ReceiveKey>,
}

impl IdempotentReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn receive(&mut self, key: ReceiveKey) -> ReceiveOutcome {
        if self.seen.try_admit(key) {
            ReceiveOutcome::Process
        } else {
            ReceiveOutcome::Duplicate
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_keeps_the_identity_and_counts_attempts() {
        let id = EffectId::Message(MessageId::new());
        let e = Effect::first_attempt(id);
        let r = e.retry().retry();
        assert_eq!(r.id(), id);
        assert_eq!(r.attempt(), 3);
    }

    #[test]
    fn attempt_counter_saturates() {
        let mut e = Effect::first_attempt(EffectId::Bundle(BundleId::new()));
        e.attempt = u32::MAX;
        assert_eq!(e.retry().attempt(), u32::MAX);
    }

    #[test]
    fn all_four_effect_kinds_have_distinct_identities() {
        let ids = [
            EffectId::Message(MessageId::new()),
            EffectId::Transfer(TransferId::new()),
            EffectId::Bundle(BundleId::new()),
            EffectId::DeviceEvent(DeviceEventId::new()),
        ];
        let set: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn every_listed_kind_is_deduplicated() {
        let mut r = IdempotentReceiver::new();
        let t = TransferId::new();
        let m = MessageId::new();
        let keys = [
            ReceiveKey::Message(m),
            ReceiveKey::Chunk {
                transfer: t,
                index: 7,
            },
            ReceiveKey::Bundle(BundleId::new()),
            ReceiveKey::Receipt {
                message: m,
                kind: 1,
            },
            ReceiveKey::DeviceEvent(DeviceEventId::new()),
        ];
        for k in keys {
            assert_eq!(r.receive(k), ReceiveOutcome::Process);
            assert_eq!(r.receive(k), ReceiveOutcome::Duplicate);
            assert_eq!(r.receive(k), ReceiveOutcome::Duplicate);
        }
    }

    #[test]
    fn near_misses_are_not_duplicates() {
        let mut r = IdempotentReceiver::new();
        let t = TransferId::new();
        let m = MessageId::new();
        assert_eq!(
            r.receive(ReceiveKey::Chunk {
                transfer: t,
                index: 1
            }),
            ReceiveOutcome::Process
        );
        assert_eq!(
            r.receive(ReceiveKey::Chunk {
                transfer: t,
                index: 2
            }),
            ReceiveOutcome::Process
        );
        assert_eq!(
            r.receive(ReceiveKey::Receipt {
                message: m,
                kind: 1
            }),
            ReceiveOutcome::Process
        );
        assert_eq!(
            r.receive(ReceiveKey::Receipt {
                message: m,
                kind: 2
            }),
            ReceiveOutcome::Process
        );
        // a message and a receipt for it are different things
        assert_eq!(r.receive(ReceiveKey::Message(m)), ReceiveOutcome::Process);
    }

    #[test]
    fn crash_resend_scenario_applies_once() {
        // Sender crashed before recording success and retries with the
        // same id; receiver must apply exactly once.
        let m = MessageId::new();
        let first = Effect::first_attempt(EffectId::Message(m));
        let retried = first.retry();
        let mut r = IdempotentReceiver::new();
        let mut applied = 0;
        for e in [first, retried] {
            let EffectId::Message(id) = e.id() else {
                unreachable!()
            };
            if r.receive(ReceiveKey::Message(id)) == ReceiveOutcome::Process {
                applied += 1;
            }
        }
        assert_eq!(applied, 1);
    }
}
