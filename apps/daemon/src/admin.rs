//! Remote administration glue: maps the closed `comm-admin/1` operation list
//! onto the runtime, audits every outcome into the runtime's audit log, and
//! builds the operator table from `[admin]`.
//!
//! Nothing here widens what an operator can do: each [`AdminOp`] maps to one
//! existing runtime query or command, there is no pass-through to
//! application commands (`Command::App` / `Query::App`), and enrolment
//! (`Provision`) is not reachable remotely at all.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use siar_admin::server::BoxFuture;
use siar_admin::{
    AdminBackend, AdminError, AdminOp, AdminRole, AuditKind, Operator, OperatorContext,
    OperatorTable,
};
use siar_runtime::audit::{AuditEntry, Outcome};
use siar_runtime::config::AdminSection;
use siar_runtime::dedup::{CommandId, Principal};
use siar_runtime::error::RuntimeError;
use siar_runtime::protocol::{Command, Query, Role};
use siar_runtime::runtime::Runtime;

/// Remote principals live in their own id space: the top bit is set, local
/// UIDs never are, so a remote operator can never collide with — or replay
/// the idempotency ledger of — a local user.
#[must_use]
pub fn remote_principal(key: &[u8; 32]) -> Principal {
    let mut b = [0u8; 8];
    b.copy_from_slice(&key[..8]);
    u64::from_be_bytes(b) | (1 << 63)
}

fn audit_role(r: AdminRole) -> Role {
    match r {
        AdminRole::Viewer => Role::Observer,
        AdminRole::Operator => Role::Operator,
        AdminRole::SecurityAdmin | AdminRole::Owner => Role::Admin,
    }
}

fn map_error(e: &RuntimeError) -> AdminError {
    match e {
        RuntimeError::NotReady | RuntimeError::ReadOnly => AdminError::NotReady,
        RuntimeError::Busy | RuntimeError::Cancelled => AdminError::Busy,
        RuntimeError::InvalidCommand(_) => AdminError::Invalid,
        RuntimeError::Unsupported => AdminError::Unsupported,
        RuntimeError::Unauthorized => AdminError::Forbidden,
        _ => AdminError::Internal,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

pub struct DaemonAdmin {
    rt: Runtime,
}

impl DaemonAdmin {
    #[must_use]
    pub fn new(rt: Runtime) -> Arc<Self> {
        Arc::new(Self { rt })
    }

    async fn query(&self, q: Query) -> Result<Vec<u8>, AdminError> {
        let reply = self.rt.query(q).await.map_err(|e| map_error(&e))?;
        Ok(crate::render_reply(&reply, false).into_bytes())
    }

    async fn command(&self, ctx: &OperatorContext, cmd: Command) -> Result<Vec<u8>, AdminError> {
        // The request id is the idempotency key: a retried request replays
        // the stored outcome instead of running twice.
        let out = self
            .rt
            .command(remote_principal(&ctx.key), CommandId(ctx.request_id), cmd)
            .await
            .map_err(|e| map_error(&e))?;
        Ok(if out.is_empty() { b"ok".to_vec() } else { out })
    }
}

impl AdminBackend for DaemonAdmin {
    fn execute<'a>(
        &'a self,
        ctx: &'a OperatorContext,
        op: AdminOp,
    ) -> BoxFuture<'a, Result<Vec<u8>, AdminError>> {
        Box::pin(async move {
            match op {
                AdminOp::Health => self.query(Query::Health).await,
                AdminOp::Status => self.query(Query::Status).await,
                AdminOp::Metrics => self.query(Query::Metrics).await,
                AdminOp::Diagnostics => self.query(Query::Diagnostics).await,
                AdminOp::Power => self.query(Query::Power).await,
                AdminOp::ProvisionStatus => self.query(Query::ProvisionStatus).await,
                AdminOp::Audit => self.query(Query::Audit).await,
                AdminOp::SetMaintenance(on) => self.command(ctx, Command::SetMaintenance(on)).await,
                AdminOp::SetReadOnly(on) => self.command(ctx, Command::SetReadOnly(on)).await,
                AdminOp::CreateBackup { target } => {
                    self.command(ctx, Command::CreateBackup { target }).await
                }
                AdminOp::RestoreBackup { source } => {
                    self.command(ctx, Command::RestoreBackup { source }).await
                }
                AdminOp::Shutdown => self.command(ctx, Command::Shutdown).await,
            }
        })
    }

    fn audit(&self, who: Option<&OperatorContext>, label: &str, kind: AuditKind) {
        let (principal, role, who_label) = match who {
            Some(w) => (
                remote_principal(&w.key),
                audit_role(w.role),
                w.label.as_str(),
            ),
            None => (0, Role::Observer, "unknown"),
        };
        let outcome = match kind {
            AuditKind::Ok => Outcome::Ok,
            AuditKind::Failed(_) | AuditKind::Malformed => Outcome::Failed,
            AuditKind::Forbidden
            | AuditKind::RateLimited
            | AuditKind::UnknownPeer
            | AuditKind::Overloaded => Outcome::Denied,
        };
        // Labels are validated identifiers (acl.rs), so this stays inside the
        // audit log's fixed vocabulary and can carry no free text.
        self.rt.audit.record(AuditEntry {
            at_ms: now_ms(),
            principal,
            role,
            action: format!("remote:{who_label}:{label}"),
            outcome,
        });
    }
}

/// `[admin]` → an [`OperatorTable`]. Config validation already checked the
/// shapes; this additionally rejects keys that are not valid Ed25519 points.
pub fn operator_table(section: &AdminSection) -> Result<OperatorTable> {
    let ops = section
        .operators
        .iter()
        .map(|o| {
            Ok(Operator {
                key: siar_admin::keys::parse_hex32(&o.key)
                    .ok_or_else(|| anyhow!("admin operator key is not 64 hex characters"))?,
                role: AdminRole::parse(&o.role)
                    .ok_or_else(|| anyhow!("unknown admin role {:?}", o.role))?,
                label: o.label.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    OperatorTable::new(ops).map_err(|e| anyhow!("admin.operators: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_runtime::config::AdminOperatorEntry;

    #[test]
    fn remote_principals_never_collide_with_local_uids() {
        for k in [[0u8; 32], [0xff; 32], [7; 32]] {
            let p = remote_principal(&k);
            assert!(p >= 1 << 63, "top bit must be set");
            assert!(u64::from(u32::MAX) < p, "no 32-bit uid can equal it");
        }
        assert_ne!(remote_principal(&[1; 32]), remote_principal(&[2; 32]));
    }

    #[test]
    fn errors_map_to_the_fixed_remote_vocabulary_without_detail() {
        assert_eq!(map_error(&RuntimeError::Busy), AdminError::Busy);
        assert_eq!(map_error(&RuntimeError::NotReady), AdminError::NotReady);
        assert_eq!(map_error(&RuntimeError::ReadOnly), AdminError::NotReady);
        assert_eq!(
            map_error(&RuntimeError::InvalidCommand("/home/alice/x".into())),
            AdminError::Invalid,
            "the detail (a path) must not cross the wire"
        );
        assert_eq!(map_error(&RuntimeError::Storage), AdminError::Internal);
        assert_eq!(
            map_error(&RuntimeError::Unsupported),
            AdminError::Unsupported
        );
    }

    #[test]
    fn the_table_is_built_from_config_and_bad_keys_are_refused() {
        let good = iroh::SecretKey::generate().public();
        let mut s = AdminSection {
            enabled: true,
            discovery: false,
            bind: None,
            operators: vec![AdminOperatorEntry {
                key: siar_admin::keys::to_hex(good.as_bytes()),
                role: "security-admin".into(),
                label: "laptop".into(),
            }],
        };
        let t = operator_table(&s).unwrap();
        let op = t.lookup(good.as_bytes()).unwrap();
        assert_eq!(op.role, AdminRole::SecurityAdmin);
        // Hex-valid but not a curve point.
        s.operators[0].key = format!("02{}", "00".repeat(31));
        assert!(operator_table(&s).is_err());
    }
}

// ---------------------------------------------------------------- operator side

/// `siar-daemon admin-keygen`: create the operator's key (never overwritten)
/// and return its public id, which goes into the node's `[admin]` config.
pub fn keygen(out: &std::path::Path) -> Result<String> {
    if out.exists() {
        anyhow::bail!(
            "{} already exists; refusing to overwrite a key",
            out.display()
        );
    }
    let key = siar_admin::keys::load_or_create(out)?;
    Ok(siar_admin::keys::public_hex(&key))
}

/// `ctl`-style words → the closed operation list.
pub fn parse_op(args: &[String]) -> Result<AdminOp> {
    let word = |i: usize| args.get(i).map(String::as_str);
    let onoff = |name: &str| -> Result<bool> {
        match (word(1), args.len()) {
            (Some("on"), 2) => Ok(true),
            (Some("off"), 2) => Ok(false),
            _ => Err(anyhow!("usage: admin <node> {name} <on|off>")),
        }
    };
    let one_arg = |name: &str| -> Result<String> {
        match (word(1), args.len()) {
            (Some(a), 2) => Ok(a.to_owned()),
            _ => Err(anyhow!("usage: admin <node> {name} <absolute-directory>")),
        }
    };
    let no_args = |op: AdminOp| -> Result<AdminOp> {
        if args.len() == 1 {
            Ok(op)
        } else {
            Err(anyhow!("`{}` takes no arguments", args[0]))
        }
    };
    match word(0) {
        Some("health") => no_args(AdminOp::Health),
        Some("status") => no_args(AdminOp::Status),
        Some("metrics") => no_args(AdminOp::Metrics),
        Some("diagnostics") => no_args(AdminOp::Diagnostics),
        Some("power") => no_args(AdminOp::Power),
        Some("provision-status") => no_args(AdminOp::ProvisionStatus),
        Some("audit") => no_args(AdminOp::Audit),
        Some("shutdown") => no_args(AdminOp::Shutdown),
        Some("maintenance") => Ok(AdminOp::SetMaintenance(onoff("maintenance")?)),
        Some("read-only") => Ok(AdminOp::SetReadOnly(onoff("read-only")?)),
        Some("backup") => Ok(AdminOp::CreateBackup { target: one_arg("backup")? }),
        Some("restore") => Ok(AdminOp::RestoreBackup { source: one_arg("restore")? }),
        Some(other) => Err(anyhow!(
            "unknown operation {other:?}. There is no remote shell: one of health, status, metrics, \
             diagnostics, power, provision-status, audit, maintenance on|off, read-only on|off, \
             backup <dir>, restore <dir>, shutdown"
        )),
        None => Err(anyhow!("no operation given")),
    }
}

/// `siar-daemon admin …`: run one operation against a node.
pub async fn remote(
    key_file: &std::path::Path,
    node_id_hex: &str,
    addrs: &[std::net::SocketAddr],
    discovery: bool,
    op: AdminOp,
) -> Result<String> {
    let id_bytes = siar_admin::keys::parse_hex32(node_id_hex)
        .ok_or_else(|| anyhow!("the node id must be 64 lowercase hex characters"))?;
    let id = iroh::PublicKey::from_bytes(&id_bytes).map_err(|_| anyhow!("not a valid node id"))?;
    if addrs.is_empty() && !discovery {
        anyhow::bail!(
            "give the node's address with --addr <ip:port> (repeatable), or pass --discovery"
        );
    }
    let node =
        iroh::EndpointAddr::from_parts(id, addrs.iter().map(|a| iroh::TransportAddr::Ip(*a)));
    let secret = siar_admin::keys::load_or_create(key_file)?;
    let client = siar_admin::AdminClient::new(secret, discovery).await?;
    let result = client.request(node, op).await;
    client.close().await;
    Ok(String::from_utf8_lossy(&result?).into_owned())
}

#[cfg(test)]
mod operator_tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_command_words_map_to_the_closed_list() {
        assert_eq!(parse_op(&w("health")).unwrap(), AdminOp::Health);
        assert_eq!(
            parse_op(&w("maintenance on")).unwrap(),
            AdminOp::SetMaintenance(true)
        );
        assert_eq!(
            parse_op(&w("read-only off")).unwrap(),
            AdminOp::SetReadOnly(false)
        );
        assert_eq!(
            parse_op(&w("backup /var/backups/siar")).unwrap(),
            AdminOp::CreateBackup {
                target: "/var/backups/siar".into()
            }
        );
        assert_eq!(parse_op(&w("shutdown")).unwrap(), AdminOp::Shutdown);
    }

    #[test]
    fn there_is_no_shell_and_no_malformed_arguments() {
        for bad in [
            "sh -c id",
            "exec ls",
            "app send hi",
            "provision x y",
            "maintenance",
            "maintenance maybe",
            "maintenance on now",
            "backup",
            "backup a b",
            "health now",
            "shutdown -f",
            "",
        ] {
            assert!(parse_op(&w(bad)).is_err(), "{bad:?} must not parse");
        }
        let e = parse_op(&w("sh -c id")).unwrap_err().to_string();
        assert!(e.contains("no remote shell"), "{e}");
    }

    #[test]
    fn keygen_never_overwrites_and_prints_a_public_id() {
        let d = std::env::temp_dir().join(format!("siar-adm-kg-{}", uuid::Uuid::new_v4()));
        let f = d.join("op.key");
        let id = keygen(&f).unwrap();
        assert_eq!(id.len(), 64);
        let again = keygen(&f).unwrap_err().to_string();
        assert!(again.contains("refusing to overwrite"), "{again}");
        // The id printed is the id the key file yields.
        assert_eq!(
            siar_admin::keys::public_hex(&siar_admin::keys::load_or_create(&f).unwrap()),
            id
        );
    }
}
