//! Wire types for `comm-admin/1`.
//!
//! One request per bidirectional stream: the client writes a postcard
//! [`Request`], finishes the stream, and reads one postcard [`Reply`].

use serde::{Deserialize, Serialize};

/// The protocol id. Versioned and distinct from `comm-ipc/1` (§182).
pub const ALPN: &[u8] = b"comm-admin/1";
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_REPLY_BYTES: usize = 512 * 1024;

/// §69. Ordered: a higher role includes every lower one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AdminRole {
    /// Read health, status, metrics and the redacted diagnostic bundle.
    Viewer,
    /// Lifecycle that does not endanger data: maintenance, read-only, backup.
    Operator,
    /// Read the audit log.
    SecurityAdmin,
    /// Destructive or irreversible: restore a backup, shut the node down.
    Owner,
}

impl AdminRole {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::SecurityAdmin => "security-admin",
            Self::Owner => "owner",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Viewer,
            Self::Operator,
            Self::SecurityAdmin,
            Self::Owner,
        ]
        .into_iter()
        .find(|r| r.name() == s)
    }
}

/// §67/§71: the **closed** list of things a remote admin can ask for. There
/// is deliberately no variant that runs a command line, forwards an
/// application command, or reads message content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminOp {
    Health,
    Status,
    Metrics,
    /// Redacted diagnostic bundle (no secrets, no message content).
    Diagnostics,
    Power,
    ProvisionStatus,
    Audit,
    SetMaintenance(bool),
    SetReadOnly(bool),
    CreateBackup {
        target: String,
    },
    RestoreBackup {
        source: String,
    },
    Shutdown,
}

impl AdminOp {
    /// The lowest role allowed to perform this operation (§69). Exhaustive:
    /// adding an operation without deciding its role does not compile.
    #[must_use]
    pub const fn required_role(&self) -> AdminRole {
        match self {
            Self::Health
            | Self::Status
            | Self::Metrics
            | Self::Diagnostics
            | Self::Power
            | Self::ProvisionStatus => AdminRole::Viewer,
            Self::SetMaintenance(_) | Self::SetReadOnly(_) | Self::CreateBackup { .. } => {
                AdminRole::Operator
            }
            Self::Audit => AdminRole::SecurityAdmin,
            Self::RestoreBackup { .. } | Self::Shutdown => AdminRole::Owner,
        }
    }

    /// Fixed vocabulary for the audit log: never carries arguments (a backup
    /// path can name a user).
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Health => "health",
            Self::Status => "status",
            Self::Metrics => "metrics",
            Self::Diagnostics => "diagnostics",
            Self::Power => "power",
            Self::ProvisionStatus => "provision-status",
            Self::Audit => "audit",
            Self::SetMaintenance(true) => "maintenance:on",
            Self::SetMaintenance(false) => "maintenance:off",
            Self::SetReadOnly(true) => "read-only:on",
            Self::SetReadOnly(false) => "read-only:off",
            Self::CreateBackup { .. } => "create-backup",
            Self::RestoreBackup { .. } => "restore-backup",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// Also the idempotency key: a retry with the same id replays the stored
    /// outcome instead of running the operation twice.
    pub id: u128,
    pub op: AdminOp,
}

/// A fixed vocabulary — no free text, so a failing operation cannot leak
/// paths or internals to a remote caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum AdminError {
    #[error("your role does not allow this operation")]
    Forbidden,
    #[error("rate limit exceeded; slow down")]
    RateLimited,
    #[error("the node is busy; retry")]
    Busy,
    #[error("the node is not ready for this operation")]
    NotReady,
    #[error("invalid request")]
    Invalid,
    #[error("not supported on this node")]
    Unsupported,
    #[error("the operation timed out")]
    Timeout,
    #[error("internal error")]
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u128,
    pub result: Result<Vec<u8>, AdminError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_op() -> Vec<AdminOp> {
        vec![
            AdminOp::Health,
            AdminOp::Status,
            AdminOp::Metrics,
            AdminOp::Diagnostics,
            AdminOp::Power,
            AdminOp::ProvisionStatus,
            AdminOp::Audit,
            AdminOp::SetMaintenance(true),
            AdminOp::SetMaintenance(false),
            AdminOp::SetReadOnly(true),
            AdminOp::SetReadOnly(false),
            AdminOp::CreateBackup {
                target: "/x".into(),
            },
            AdminOp::RestoreBackup {
                source: "/x".into(),
            },
            AdminOp::Shutdown,
        ]
    }

    #[test]
    fn roles_are_ordered_and_named() {
        assert!(AdminRole::Viewer < AdminRole::Operator);
        assert!(AdminRole::Operator < AdminRole::SecurityAdmin);
        assert!(AdminRole::SecurityAdmin < AdminRole::Owner);
        for r in [
            AdminRole::Viewer,
            AdminRole::Operator,
            AdminRole::SecurityAdmin,
            AdminRole::Owner,
        ] {
            assert_eq!(AdminRole::parse(r.name()), Some(r));
        }
        assert_eq!(AdminRole::parse("root"), None);
    }

    #[test]
    fn the_role_table_is_what_the_spec_intends() {
        use AdminRole::*;
        let want = [
            (AdminOp::Health, Viewer),
            (AdminOp::Status, Viewer),
            (AdminOp::Metrics, Viewer),
            (AdminOp::Diagnostics, Viewer),
            (AdminOp::Power, Viewer),
            (AdminOp::ProvisionStatus, Viewer),
            (AdminOp::SetMaintenance(true), Operator),
            (AdminOp::SetReadOnly(false), Operator),
            (
                AdminOp::CreateBackup {
                    target: String::new(),
                },
                Operator,
            ),
            (AdminOp::Audit, SecurityAdmin),
            (
                AdminOp::RestoreBackup {
                    source: String::new(),
                },
                Owner,
            ),
            (AdminOp::Shutdown, Owner),
        ];
        for (op, role) in want {
            assert_eq!(op.required_role(), role, "{op:?}");
        }
        // Nothing destructive is reachable below Owner; nothing mutating at Viewer.
        for op in every_op() {
            let r = op.required_role();
            if matches!(op, AdminOp::Shutdown | AdminOp::RestoreBackup { .. }) {
                assert_eq!(r, Owner);
            }
            if r == Viewer {
                assert!(!matches!(
                    op,
                    AdminOp::SetMaintenance(_) | AdminOp::SetReadOnly(_) | AdminOp::Shutdown
                ));
            }
        }
    }

    #[test]
    fn audit_labels_never_carry_arguments() {
        for op in every_op() {
            let l = op.label();
            assert!(!l.contains('/') && l.len() < 24, "{l}");
        }
        let a = AdminOp::CreateBackup {
            target: "/home/alice/secret".into(),
        };
        assert!(!a.label().contains("alice"));
    }

    #[test]
    fn requests_round_trip_and_unknown_variants_do_not_decode() {
        for op in every_op() {
            let r = Request { id: 7, op };
            let bytes = postcard::to_allocvec(&r).unwrap();
            assert_eq!(postcard::from_bytes::<Request>(&bytes).unwrap(), r);
        }
        // A variant index beyond the closed list is an error, not a panic.
        let mut bytes = postcard::to_allocvec(&Request {
            id: 1,
            op: AdminOp::Health,
        })
        .unwrap();
        let last = bytes.len() - 1;
        bytes[last] = 0x7f;
        assert!(postcard::from_bytes::<Request>(&bytes).is_err());
    }
}
