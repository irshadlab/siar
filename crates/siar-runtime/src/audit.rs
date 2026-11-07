//! §161-§162: audit log of administrative actions.
//!
//! Records *who* (authenticated principal + role), *what kind of action* and
//! *the outcome* — never command payloads, message text, tickets or file
//! names (§162: no message-content admin). Bounded: the oldest entries fall
//! off, so a hostile client cannot grow it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::audit_file::{FileLimits, FileSink};
use crate::protocol::{Command, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Denied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub at_ms: u64,
    pub principal: u64,
    pub role: Role,
    /// A fixed vocabulary, e.g. `shutdown`, `set-maintenance:on`, `app:send`.
    pub action: String,
    pub outcome: Outcome,
}

/// The action label for a command — kind only, **never its data**.
#[must_use]
pub fn action_label(c: &Command) -> String {
    match c {
        Command::Shutdown => "shutdown".into(),
        Command::SetEmergency(on) => format!("set-emergency:{}", if *on { "on" } else { "off" }),
        Command::SetMaintenance(on) => {
            format!("set-maintenance:{}", if *on { "on" } else { "off" })
        }
        Command::SetReadOnly(on) => format!("set-read-only:{}", if *on { "on" } else { "off" }),
        Command::CreateBackup { .. } => "create-backup".into(),
        Command::RestoreBackup { .. } => "restore-backup".into(),
        Command::Provision { .. } => "provision".into(),
        Command::App { name, .. } => format!(
            "app:{}",
            name.chars()
                .take(32)
                .filter(|c| c.is_ascii_graphic())
                .collect::<String>()
        ),
    }
}

#[derive(Debug, Clone)]
pub struct AuditLog {
    cap: usize,
    inner: Arc<Mutex<VecDeque<AuditEntry>>>,
    /// Optional durable, hash-chained file (see [`crate::audit_file`]).
    /// Lock order is always `inner` → `sink`.
    sink: Arc<Mutex<Option<FileSink>>>,
}

impl AuditLog {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            inner: Arc::default(),
            sink: Arc::default(),
        }
    }

    /// Also write every entry to a durable, hash-chained file in `dir`.
    /// Returns a warning if an existing log failed verification and was set
    /// aside. Entries recorded before this call stay memory-only.
    pub fn attach_file(
        &self,
        dir: &std::path::Path,
        limits: FileLimits,
    ) -> std::io::Result<Option<String>> {
        let (sink, warning) = FileSink::open(dir, limits)?;
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
        Ok(warning)
    }

    /// `(seq, chain value)` of the last persisted entry — record it somewhere
    /// the node's own user cannot write, to make tail-truncation visible.
    #[must_use]
    pub fn persisted_head(&self) -> Option<(u64, String)> {
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(FileSink::head)
    }

    /// How many entries could not be written to disk (0 when healthy).
    #[must_use]
    pub fn persist_failures(&self) -> u64 {
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, FileSink::write_failures)
    }

    pub fn record(&self, e: AuditEntry) {
        let mut q = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() >= self.cap {
            q.pop_front();
        }
        if let Some(sink) = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            sink.append(&e);
        }
        q.push_back(e);
    }

    #[must_use]
    pub fn recent(&self) -> Vec<AuditEntry> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// One line per entry, oldest first.
    #[must_use]
    pub fn render_text(&self) -> String {
        let mut text = self
            .recent()
            .iter()
            .map(|e| {
                format!(
                    "{}\tuid={}\t{:?}\t{}\t{:?}",
                    e.at_ms, e.principal, e.role, e.action, e.outcome
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The chain head, for off-box anchoring (only when persistence is on).
        if let Some((seq, chain)) = self.persisted_head() {
            let failures = self.persist_failures();
            text.push_str(&format!(
                "\n# persisted head seq={seq} chain={chain} write-failures={failures}"
            ));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_never_contain_payloads() {
        let c = Command::App {
            name: "send".into(),
            data: b"super secret message".to_vec(),
        };
        let l = action_label(&c);
        assert_eq!(l, "app:send");
        assert!(!l.contains("secret"));
        assert_eq!(
            action_label(&Command::SetMaintenance(true)),
            "set-maintenance:on"
        );
    }

    #[test]
    fn hostile_action_names_are_sanitised_and_truncated() {
        let l = action_label(&Command::App {
            name: format!("a\n\u{1b}[31m{}", "x".repeat(500)),
            data: vec![],
        });
        assert!(l.len() <= "app:".len() + 32 && !l.contains('\n') && !l.contains('\u{1b}'));
    }

    #[test]
    fn bounded_oldest_first() {
        let log = AuditLog::new(3);
        for i in 0..10 {
            log.record(AuditEntry {
                at_ms: i,
                principal: 1,
                role: Role::Admin,
                action: "x".into(),
                outcome: Outcome::Ok,
            });
        }
        let r = log.recent();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].at_ms, 7);
    }

    #[test]
    fn a_persistent_log_records_to_disk_and_reports_its_head() {
        let d = std::env::temp_dir().join(format!("siar-auditlog-{}", uuid::Uuid::new_v4()));
        let log = AuditLog::new(4);
        assert_eq!(log.persisted_head(), None);
        assert!(!log.render_text().contains("persisted head"));
        assert!(log
            .attach_file(&d, FileLimits::default())
            .unwrap()
            .is_none());
        for i in 0..10 {
            log.record(AuditEntry {
                at_ms: i,
                principal: 7,
                role: Role::Admin,
                action: format!("a{i}"),
                outcome: Outcome::Ok,
            });
        }
        // The ring forgot 6 of them; the file did not.
        assert_eq!(log.recent().len(), 4);
        let rep = crate::audit_file::FileSink::verify_dir(&d, 4).unwrap();
        assert!(rep.is_ok() && rep.entries == 10, "{rep:?}");
        let (seq, chain) = log.persisted_head().unwrap();
        assert_eq!(seq, 9);
        assert!(log.render_text().ends_with(&format!(
            "# persisted head seq=9 chain={chain} write-failures=0"
        )));
        // Clones share the sink.
        log.clone().record(AuditEntry {
            at_ms: 11,
            principal: 7,
            role: Role::Admin,
            action: "b".into(),
            outcome: Outcome::Denied,
        });
        assert_eq!(log.persisted_head().unwrap().0, 10);
    }
}
