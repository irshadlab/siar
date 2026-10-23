//! Security Events Screen (ui-ux-15 §40-47), reconciling and widening
//! the simpler model this module previously held (which implemented
//! Part 28 §42, "Identity Change UX", specifically).
//!
//! **What changed and why**: the previous version of this module had
//! a 3-variant `SecurityEventKind` (`NewDeviceLinked`, `DeviceRevoked`,
//! `RootIdentityChanged`) and a 2-tier `SecurityEventSeverity`
//! (`Notice`/`StrongWarning`) — accurate to Part 28 §42's own text, but
//! narrower than this spec's own §41/§42, which define an 11-kind,
//! fieldless `SecurityEventKind` and a 3-tier `Info`/`Warning`/
//! `Critical` severity. Those are two different specs' models that
//! happened to share a name; a previous round's roadmap entry noted
//! this section as "already covered," which undersold the gap — this
//! is the correction, widening the existing type in place (same
//! module, same exported names where they still apply) rather than
//! adding a second, parallel type.
//!
//! §41's `SecurityEventKind` is fieldless in the spec's own sketch —
//! `related_device`/`related_contact` live on `SecurityEvent` itself
//! (§40), not embedded per-variant. Only 3 of the 11 kinds have a real
//! backend signal anywhere in this workspace today (device linked,
//! device revoked, identity changed); the other 8
//! (`DeviceLinkDenied`, `VerificationFailed`, `RecoveryConfigured`,
//! `RecoveryChanged`, `BackupFailed`, `KeyRotation`,
//! `SuspiciousAuthorization`, `SecurityPolicyChanged`) are included
//! because §41 defines them as part of the type's own literal shape —
//! same precedent as `trust.rs`'s `TrustSource` (Part 28 §41, a
//! different §41) — not because anything can construct them yet. A
//! caller trying to raise one of the 8 unbacked kinds today has no
//! wired path to do so; that's an honest gap, not a claim of hidden
//! functionality.
//!
//! §47: "Rust owns resolved state. UI can perform actions then event
//! becomes resolved." `resolved` is therefore the one state field this
//! module tracks — no separate "acknowledged" concept layered on top
//! (the previous version had one; dropped here as redundant now that
//! `resolved` exists and is the spec's own literal model).

use siar_domain::{AccountId, DeviceId};

/// §42, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SecurityEventSeverity {
    Info,
    Warning,
    Critical,
}

/// §41, verbatim — fieldless, per the spec's own sketch (see this
/// module's top doc comment for where the per-event device/contact
/// linkage actually lives).
///
/// §129-131 (Key Health Events): the spec's own worked examples —
/// "device key rotated," "recovery key changed," "identity key
/// changed," "backup key invalid" — map onto `KeyRotation`,
/// `RecoveryChanged`, `IdentityChanged`, and `BackupFailed` below,
/// already present before this reconciliation pass; §130's "routine
/// key rotation: informational, not alarming" and §131's "unexpected
/// root identity change: critical" already match this type's existing
/// `severity()` mapping (`KeyRotation` → `Info`, `IdentityChanged` →
/// `Critical`) — confirmed, not re-derived, when this doc comment was
/// added.
///
/// `KeyExpiryActionRequired` (§132) is new: "if product uses expiring
/// certs/keys: renew automatically, and only surface if action
/// required" — the *only* variant added this round, since automatic
/// renewal that succeeds is explicitly not supposed to raise any
/// event at all (matching the "routine rotation is invisible"
/// principle Part 28 §42 established, applied here to expiry too).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecurityEventKind {
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

/// §43's filter list mixes two different groupings — severity-based
/// (`Warnings`, `Critical`) and topic-based (`Devices`, `Identity`,
/// `Recovery`) — this is the topic half, used by `SecurityEventKind::category`.
/// `Other` isn't one of §43's named filter tabs (only `SecurityPolicyChanged`
/// falls here) — it exists so `category()` stays a total function
/// rather than needing an unwrap somewhere, not because the UI should
/// render an "Other" tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventCategory {
    Devices,
    Identity,
    Recovery,
    Other,
}

impl SecurityEventKind {
    /// Severity mapping: not given verbatim by the spec (§41 and §42
    /// are two separate lists with no stated correspondence between
    /// them) — inferred from §6-8's own worked Healthy/Attention/
    /// Critical examples where they clearly apply (identity compromise
    /// → Critical, unknown/suspicious authorization → Critical), and a
    /// reasoned default elsewhere. Documented as inferred, not
    /// spec-literal, so a future correction against clearer guidance
    /// is an easy find-and-fix rather than an assumption buried in
    /// behavior.
    pub const fn severity(self) -> SecurityEventSeverity {
        match self {
            Self::DeviceLinked
            | Self::DeviceRevoked
            | Self::DeviceLinkDenied
            | Self::RecoveryConfigured
            | Self::KeyRotation => SecurityEventSeverity::Info,
            Self::VerificationFailed
            | Self::RecoveryChanged
            | Self::BackupFailed
            | Self::SecurityPolicyChanged
            | Self::KeyExpiryActionRequired => SecurityEventSeverity::Warning,
            Self::IdentityChanged | Self::SuspiciousAuthorization => {
                SecurityEventSeverity::Critical
            }
        }
    }

    pub const fn category(self) -> SecurityEventCategory {
        match self {
            Self::DeviceLinked
            | Self::DeviceRevoked
            | Self::DeviceLinkDenied
            | Self::SuspiciousAuthorization => SecurityEventCategory::Devices,
            Self::IdentityChanged
            | Self::VerificationFailed
            | Self::KeyRotation
            | Self::KeyExpiryActionRequired => SecurityEventCategory::Identity,
            Self::RecoveryConfigured | Self::RecoveryChanged | Self::BackupFailed => {
                SecurityEventCategory::Recovery
            }
            Self::SecurityPolicyChanged => SecurityEventCategory::Other,
        }
    }
}

/// §43's six filter tabs, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventFilter {
    All,
    Warnings,
    Critical,
    Devices,
    Identity,
    Recovery,
}

impl SecurityEventFilter {
    fn matches(self, event: &SecurityEvent) -> bool {
        match self {
            Self::All => true,
            Self::Warnings => event.severity == SecurityEventSeverity::Warning,
            Self::Critical => event.severity == SecurityEventSeverity::Critical,
            Self::Devices => event.kind.category() == SecurityEventCategory::Devices,
            Self::Identity => event.kind.category() == SecurityEventCategory::Identity,
            Self::Recovery => event.kind.category() == SecurityEventCategory::Recovery,
        }
    }
}

/// Not one of §40's fields — a locally-assigned identifier this module
/// needs to let `resolve()` target one specific event without relying
/// on list position (which shifts under insertion). A `u64` sequence
/// counter rather than a `Uuid`: this ID only needs to be unique within
/// one `SecurityEventState` instance, not globally, and adding a `uuid`
/// dependency to this crate for that would be more than the need calls
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SecurityEventId(u64);

impl SecurityEventId {
    /// The raw sequence number, for UIs that must round-trip an id
    /// through a string-keyed row (the Slint desktop UI does).
    pub fn get(self) -> u64 {
        self.0
    }

    /// Rebuilds an id from `get()`'s output. An id that matches no event
    /// is harmless: `SecurityEventState::resolve` ignores it.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// §41's actions have no spec-given enumeration anywhere in this
/// document — same under-specification as `security_center.rs`'s
/// `DeviceSecurityFlag`. This is a minimal, honestly-scoped set drawn
/// from concrete actions already named elsewhere in this spec (§22
/// Approve/Deny, §26/§32 revoke wording, §38's checklist actions), not
/// a guess at an exhaustive list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventAction {
    Revoke,
    Approve,
    Deny,
    Review,
    RotateCredentials,
    VerifyBackup,
    Dismiss,
}

/// §40, field-for-field (`Timestamp` as `u64` millis, this workspace's
/// existing convention — see `security_center.rs`'s own note on the
/// same choice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityEvent {
    pub id: SecurityEventId,
    pub kind: SecurityEventKind,
    pub severity: SecurityEventSeverity,
    pub occurred_at_millis: u64,
    pub resolved: bool,
    pub related_device: Option<DeviceId>,
    pub related_contact: Option<AccountId>,
    pub actions: Vec<SecurityEventAction>,
}

/// Its own state slice for the same granular-re-render reason every
/// other `*State` type in this crate already is.
#[derive(Debug, Default)]
pub struct SecurityEventState {
    events: Vec<SecurityEvent>,
    next_id: u64,
}

impl SecurityEventState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs and appends a new, unresolved event. Severity is
    /// derived from `kind` (`SecurityEventKind::severity`) rather than
    /// caller-supplied — a caller doesn't get to independently assert
    /// severity for a kind the type itself already has an answer for.
    pub fn push(
        &mut self,
        kind: SecurityEventKind,
        occurred_at_millis: u64,
        related_device: Option<DeviceId>,
        related_contact: Option<AccountId>,
        actions: Vec<SecurityEventAction>,
    ) -> SecurityEventId {
        let id = SecurityEventId(self.next_id);
        self.next_id += 1;
        self.events.push(SecurityEvent {
            id,
            kind,
            severity: kind.severity(),
            occurred_at_millis,
            resolved: false,
            related_device,
            related_contact,
            actions,
        });
        id
    }

    /// §47: "Rust owns resolved state." The one way an event's
    /// `resolved` flag ever flips — no direct field mutation exposed.
    pub fn resolve(&mut self, id: SecurityEventId) {
        if let Some(event) = self.events.iter_mut().find(|e| e.id == id) {
            event.resolved = true;
        }
    }

    /// §43: "sort newest first."
    pub fn events(&self) -> Vec<&SecurityEvent> {
        let mut sorted: Vec<&SecurityEvent> = self.events.iter().collect();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.occurred_at_millis));
        sorted
    }

    /// §43's filter tabs, applied on top of the same newest-first sort.
    pub fn filtered(&self, filter: SecurityEventFilter) -> Vec<&SecurityEvent> {
        self.events()
            .into_iter()
            .filter(|e| filter.matches(e))
            .collect()
    }

    /// What a component should treat as needing attention — every
    /// unresolved `Critical`-severity event. Narrower than "every
    /// unresolved event" the same way the previous version's
    /// `unacknowledged_strong_warnings` was — a `Warning` or `Info`
    /// event sitting unresolved in a list is fine; a `Critical` one
    /// (identity compromise, suspicious authorization) is what should
    /// interrupt.
    pub fn unresolved_critical_events(&self) -> Vec<&SecurityEvent> {
        self.filtered(SecurityEventFilter::Critical)
            .into_iter()
            .filter(|e| !e.resolved)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_changed_and_suspicious_authorization_are_critical() {
        assert_eq!(
            SecurityEventKind::IdentityChanged.severity(),
            SecurityEventSeverity::Critical
        );
        assert_eq!(
            SecurityEventKind::SuspiciousAuthorization.severity(),
            SecurityEventSeverity::Critical
        );
    }

    #[test]
    fn device_linked_and_revoked_are_info() {
        assert_eq!(
            SecurityEventKind::DeviceLinked.severity(),
            SecurityEventSeverity::Info
        );
        assert_eq!(
            SecurityEventKind::DeviceRevoked.severity(),
            SecurityEventSeverity::Info
        );
    }

    /// ui-ux-15 §130: "Routine Key Rotation: informational, not
    /// alarming."
    #[test]
    fn key_rotation_is_informational() {
        assert_eq!(
            SecurityEventKind::KeyRotation.severity(),
            SecurityEventSeverity::Info
        );
    }

    /// §132: "if product uses expiring certs/keys: renew automatically,
    /// and only surface if action required." This variant should only
    /// ever be constructed when renewal has already failed/needs a
    /// person — so it's a `Warning`, not `Info`: something did fail to
    /// resolve itself automatically.
    #[test]
    fn key_expiry_action_required_is_a_warning_not_info_or_critical() {
        assert_eq!(
            SecurityEventKind::KeyExpiryActionRequired.severity(),
            SecurityEventSeverity::Warning
        );
    }

    #[test]
    fn events_sort_newest_first() {
        let mut state = SecurityEventState::new();
        state.push(SecurityEventKind::DeviceLinked, 1_000, None, None, vec![]);
        state.push(SecurityEventKind::DeviceRevoked, 3_000, None, None, vec![]);
        state.push(
            SecurityEventKind::IdentityChanged,
            2_000,
            None,
            None,
            vec![],
        );

        let ordered = state.events();
        assert_eq!(ordered[0].occurred_at_millis, 3_000);
        assert_eq!(ordered[1].occurred_at_millis, 2_000);
        assert_eq!(ordered[2].occurred_at_millis, 1_000);
    }

    #[test]
    fn filtering_by_category() {
        let mut state = SecurityEventState::new();
        state.push(SecurityEventKind::DeviceLinked, 1_000, None, None, vec![]);
        state.push(
            SecurityEventKind::RecoveryConfigured,
            2_000,
            None,
            None,
            vec![],
        );
        state.push(
            SecurityEventKind::IdentityChanged,
            3_000,
            None,
            None,
            vec![],
        );

        assert_eq!(state.filtered(SecurityEventFilter::Devices).len(), 1);
        assert_eq!(state.filtered(SecurityEventFilter::Recovery).len(), 1);
        assert_eq!(state.filtered(SecurityEventFilter::Identity).len(), 1);
        assert_eq!(state.filtered(SecurityEventFilter::All).len(), 3);
    }

    #[test]
    fn filtering_by_severity() {
        let mut state = SecurityEventState::new();
        state.push(SecurityEventKind::DeviceLinked, 1_000, None, None, vec![]); // Info
        state.push(SecurityEventKind::BackupFailed, 2_000, None, None, vec![]); // Warning
        state.push(
            SecurityEventKind::IdentityChanged,
            3_000,
            None,
            None,
            vec![],
        ); // Critical

        assert_eq!(state.filtered(SecurityEventFilter::Warnings).len(), 1);
        assert_eq!(state.filtered(SecurityEventFilter::Critical).len(), 1);
    }

    #[test]
    fn resolving_an_event_removes_it_from_unresolved_critical() {
        let mut state = SecurityEventState::new();
        let id = state.push(
            SecurityEventKind::SuspiciousAuthorization,
            1_000,
            None,
            None,
            vec![],
        );
        assert_eq!(state.unresolved_critical_events().len(), 1);

        state.resolve(id);
        assert_eq!(state.unresolved_critical_events().len(), 0);
        assert!(state.events()[0].resolved);
    }

    #[test]
    fn resolving_an_unknown_id_is_a_harmless_no_op() {
        let mut state = SecurityEventState::new();
        state.push(SecurityEventKind::DeviceLinked, 1_000, None, None, vec![]);
        state.resolve(SecurityEventId(999));
        assert!(!state.events()[0].resolved);
    }

    #[test]
    fn warning_and_info_severity_events_never_appear_in_unresolved_critical() {
        let mut state = SecurityEventState::new();
        state.push(SecurityEventKind::DeviceLinked, 1_000, None, None, vec![]);
        state.push(SecurityEventKind::BackupFailed, 2_000, None, None, vec![]);
        assert_eq!(state.unresolved_critical_events().len(), 0);
    }
}
