//! Security events on the wire.
//!
//! The daemon's core already raises (and persists) these — e.g. a contact's
//! key changed → `IdentityChanged`, Critical. Windows need to *see* them, so
//! they are a synced section like any other daemon-owned state.
//!
//! What crosses the wire is the **fact** (kind, time, linkage, offered
//! actions, resolved flag). Severity is deliberately *not* sent: the receiver
//! derives it from the kind (`SecurityEventKind::severity`), so a bug or a
//! hostile peer on the sync channel cannot downgrade a Critical event.
//!
//! Read-only for now: a window can see events but resolving one is a command
//! the sync protocol does not carry yet.

use serde::{Deserialize, Serialize};
use siar_domain::{AccountId, DeviceId};
use siar_ui_state::{SecurityEvent, SecurityEventAction, SecurityEventId, SecurityEventKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecKindDto {
    DeviceLinked,
    DeviceRevoked,
    DeviceLinkDenied,
    IdentityChanged,
    VerificationFailed,
    RecoveryConfigured,
    RecoveryChanged,
    BackupFailed,
    KeyRotation,
    SuspiciousAuthorization,
    SecurityPolicyChanged,
    KeyExpiryActionRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecActionDto {
    Revoke,
    Approve,
    Deny,
    Review,
    RotateCredentials,
    VerifyBackup,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityEventDto {
    pub id: u64,
    pub kind: SecKindDto,
    pub occurred_at_millis: u64,
    pub resolved: bool,
    pub related_device: Option<DeviceId>,
    pub related_contact: Option<AccountId>,
    pub actions: Vec<SecActionDto>,
}

macro_rules! bijection {
    ($from:ty, $dto:ty, $($v:ident),+ $(,)?) => {
        impl From<$from> for $dto {
            fn from(v: $from) -> Self { match v { $(<$from>::$v => Self::$v),+ } }
        }
        impl From<$dto> for $from {
            fn from(v: $dto) -> Self { match v { $(<$dto>::$v => Self::$v),+ } }
        }
    };
}

bijection!(
    SecurityEventKind,
    SecKindDto,
    DeviceLinked,
    DeviceRevoked,
    DeviceLinkDenied,
    IdentityChanged,
    VerificationFailed,
    RecoveryConfigured,
    RecoveryChanged,
    BackupFailed,
    KeyRotation,
    SuspiciousAuthorization,
    SecurityPolicyChanged,
    KeyExpiryActionRequired,
);

bijection!(
    SecurityEventAction,
    SecActionDto,
    Revoke,
    Approve,
    Deny,
    Review,
    RotateCredentials,
    VerifyBackup,
    Dismiss,
);

impl From<&SecurityEvent> for SecurityEventDto {
    fn from(e: &SecurityEvent) -> Self {
        Self {
            id: e.id.get(),
            kind: e.kind.into(),
            occurred_at_millis: e.occurred_at_millis,
            resolved: e.resolved,
            related_device: e.related_device,
            related_contact: e.related_contact,
            actions: e.actions.iter().map(|a| (*a).into()).collect(),
        }
    }
}

impl From<&SecurityEventDto> for SecurityEvent {
    fn from(d: &SecurityEventDto) -> Self {
        let kind = SecurityEventKind::from(d.kind);
        Self {
            id: SecurityEventId::from_raw(d.id),
            kind,
            severity: kind.severity(), // derived, never trusted from the wire
            occurred_at_millis: d.occurred_at_millis,
            resolved: d.resolved,
            related_device: d.related_device,
            related_contact: d.related_contact,
            actions: d.actions.iter().map(|a| (*a).into()).collect(),
        }
    }
}
