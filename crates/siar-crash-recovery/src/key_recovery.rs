//! §151 "Key Backup Policy", §152 "Session Key Recovery", §153 "Group
//! Key Recovery".
//!
//! Three rules about what recovery may *not* do with key material:
//! it may not pretend a non-exportable device key came back, it may
//! not restore ephemeral session keys, and it may not derive group
//! keys from stale membership.

use crate::recovery_security::{accept_repair, RepairOffer, RepairRejection};
use crate::session_recovery::{
    validate_ticket, ResumptionContext, ResumptionTicket, TicketRejection,
};

// ---------------------------------------------------------------------
// §151
// ---------------------------------------------------------------------

/// Product semantics, chosen deliberately and documented (§151).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKeyPolicy {
    /// The private key may be backed up and restored.
    Exportable,
    /// The key is bound to the device (hardware/secure element) and
    /// can never be exported.
    NonExportable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRestorePlan {
    /// The restored device *is* the same device identity as before.
    RestoreSameKey,
    /// The restored device is a **new** device of the same account and
    /// must be linked/authorized like any new device.
    NewDeviceIdentity,
}

/// `key_available` is whether the exportable key material actually
/// came back with the backup.
pub const fn device_restore_plan(
    policy: DeviceKeyPolicy,
    key_available: bool,
) -> DeviceRestorePlan {
    match (policy, key_available) {
        (DeviceKeyPolicy::Exportable, true) => DeviceRestorePlan::RestoreSameKey,
        _ => DeviceRestorePlan::NewDeviceIdentity,
    }
}

impl DeviceRestorePlan {
    /// What the user is told — the "document product semantics" line.
    pub const fn user_facing(self) -> &'static str {
        match self {
            DeviceRestorePlan::RestoreSameKey => {
                "This device was restored with its original identity."
            }
            DeviceRestorePlan::NewDeviceIdentity => {
                "This device is set up as a new device on your account and must be approved from another of your devices."
            }
        }
    }

    pub const fn needs_device_authorization(self) -> bool {
        matches!(self, DeviceRestorePlan::NewDeviceIdentity)
    }
}

// ---------------------------------------------------------------------
// §152
// ---------------------------------------------------------------------

/// §152: ephemeral session keys are never restored from storage.
pub const PERSISTED_SESSION_KEYS_ALLOWED: bool = false;

// Compile-time pin: flipping the constant breaks the build, not just a test.
const _: () = assert!(!PERSISTED_SESSION_KEYS_ALLOWED);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKeyPlan {
    /// The protocol explicitly supports secure resumption and the
    /// ticket passed every §35 check.
    ResumeWithTicket,
    /// Default: new session handshake. `why` is `None` when there was
    /// simply no ticket to consider.
    FreshHandshake { why: Option<TicketRejection> },
}

pub fn session_key_plan(
    ticket: Option<&ResumptionTicket>,
    ctx: &ResumptionContext<'_>,
) -> SessionKeyPlan {
    match ticket {
        None => SessionKeyPlan::FreshHandshake { why: None },
        Some(t) => match validate_ticket(t, ctx) {
            Ok(()) => SessionKeyPlan::ResumeWithTicket,
            Err(e) => SessionKeyPlan::FreshHandshake { why: Some(e) },
        },
    }
}

// ---------------------------------------------------------------------
// §153
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupKeyFacts {
    /// Epoch keys held locally for the group's current epoch.
    pub local_epoch: Option<u64>,
    /// Current epoch per the authoritative group state, if known.
    pub current_epoch: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    KeysMissing,
    EpochBehind { local: u64, current: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKeyAction {
    UseLocalKeys,
    /// Ask an authorized member for the current epoch (see
    /// [`accept_resync_offer`]). Never derive keys locally.
    RequestAuthorizedResync(ResyncReason),
}

/// §153: "do not derive from stale membership" — there is no
/// derive-locally outcome. Missing or behind keys always mean resync.
pub const fn group_key_action(f: GroupKeyFacts) -> GroupKeyAction {
    match (f.local_epoch, f.current_epoch) {
        (None, _) => GroupKeyAction::RequestAuthorizedResync(ResyncReason::KeysMissing),
        (Some(l), Some(c)) if l < c => {
            GroupKeyAction::RequestAuthorizedResync(ResyncReason::EpochBehind {
                local: l,
                current: c,
            })
        }
        (Some(_), _) => GroupKeyAction::UseLocalKeys,
    }
}

/// Resync material is repair state from a peer, so §147 applies in
/// full.
pub const fn accept_resync_offer(o: &RepairOffer) -> Result<(), RepairRejection> {
    accept_repair(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_recovery::ResumptionPolicy;
    use siar_domain::DeviceId;
    use std::collections::HashSet;

    #[test]
    fn non_exportable_keys_always_mean_a_new_device_identity() {
        for avail in [false, true] {
            assert_eq!(
                device_restore_plan(DeviceKeyPolicy::NonExportable, avail),
                DeviceRestorePlan::NewDeviceIdentity
            );
        }
    }

    #[test]
    fn exportable_keys_restore_only_if_they_actually_came_back() {
        assert_eq!(
            device_restore_plan(DeviceKeyPolicy::Exportable, true),
            DeviceRestorePlan::RestoreSameKey
        );
        assert_eq!(
            device_restore_plan(DeviceKeyPolicy::Exportable, false),
            DeviceRestorePlan::NewDeviceIdentity
        );
    }

    #[test]
    fn a_new_device_identity_needs_authorization_and_says_so() {
        let p = DeviceRestorePlan::NewDeviceIdentity;
        assert!(p.needs_device_authorization());
        assert!(p.user_facing().contains("new device"));
        assert!(!DeviceRestorePlan::RestoreSameKey.needs_device_authorization());
    }

    fn ticket() -> ResumptionTicket {
        ResumptionTicket::new(DeviceId::new(), 1_000, 4, true, vec![1])
    }

    fn ctx<'a>(policy: ResumptionPolicy, rev: &'a HashSet<DeviceId>) -> ResumptionContext<'a> {
        ResumptionContext {
            policy,
            now_millis: 10,
            current_account_generation: 4,
            revoked: rev,
        }
    }

    #[test]
    fn session_keys_are_never_persisted_and_the_default_is_a_new_handshake() {
        let rev = HashSet::new();
        assert_eq!(
            session_key_plan(None, &ctx(ResumptionPolicy::Enabled, &rev)),
            SessionKeyPlan::FreshHandshake { why: None }
        );
        assert_eq!(
            session_key_plan(Some(&ticket()), &ctx(ResumptionPolicy::default(), &rev)),
            SessionKeyPlan::FreshHandshake {
                why: Some(TicketRejection::Disabled)
            }
        );
    }

    #[test]
    fn resumption_needs_explicit_protocol_support_and_a_valid_ticket() {
        let rev = HashSet::new();
        assert_eq!(
            session_key_plan(Some(&ticket()), &ctx(ResumptionPolicy::Enabled, &rev)),
            SessionKeyPlan::ResumeWithTicket
        );
        let t = ticket();
        let revoked: HashSet<_> = [t.peer].into();
        assert_eq!(
            session_key_plan(Some(&t), &ctx(ResumptionPolicy::Enabled, &revoked)),
            SessionKeyPlan::FreshHandshake {
                why: Some(TicketRejection::PeerRevoked)
            }
        );
    }

    #[test]
    fn missing_group_keys_request_resync_never_local_derivation() {
        assert_eq!(
            group_key_action(GroupKeyFacts {
                local_epoch: None,
                current_epoch: Some(9)
            }),
            GroupKeyAction::RequestAuthorizedResync(ResyncReason::KeysMissing)
        );
        assert_eq!(
            group_key_action(GroupKeyFacts {
                local_epoch: None,
                current_epoch: None
            }),
            GroupKeyAction::RequestAuthorizedResync(ResyncReason::KeysMissing)
        );
    }

    #[test]
    fn stale_group_keys_request_resync() {
        assert_eq!(
            group_key_action(GroupKeyFacts {
                local_epoch: Some(3),
                current_epoch: Some(7)
            }),
            GroupKeyAction::RequestAuthorizedResync(ResyncReason::EpochBehind {
                local: 3,
                current: 7
            })
        );
    }

    #[test]
    fn current_group_keys_are_used() {
        assert_eq!(
            group_key_action(GroupKeyFacts {
                local_epoch: Some(7),
                current_epoch: Some(7)
            }),
            GroupKeyAction::UseLocalKeys
        );
        assert_eq!(
            group_key_action(GroupKeyFacts {
                local_epoch: Some(7),
                current_epoch: None
            }),
            GroupKeyAction::UseLocalKeys
        );
    }

    #[test]
    fn resync_offers_get_the_full_repair_security_check() {
        let bad = RepairOffer {
            identity_verified: true,
            authorized_for_this_repair: false,
            integrity_verified: true,
            version_ok: true,
        };
        assert_eq!(
            accept_resync_offer(&bad),
            Err(RepairRejection::Authorization)
        );
        let good = RepairOffer {
            authorized_for_this_repair: true,
            ..bad
        };
        assert_eq!(accept_resync_offer(&good), Ok(()));
    }
}
