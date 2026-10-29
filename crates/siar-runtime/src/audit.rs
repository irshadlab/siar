//! §161-§162: audit log of administrative actions.
//!
//! Records *who* (authenticated principal + role), *what kind of action* and
//! *the outcome* — never command payloads, message text, tickets or file
//! names (§162: no message-content admin). Bounded: the oldest entries fall
//! off, so a hostile client cannot grow it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

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
}

impl AuditLog {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            inner: Arc::default(),
        }
    }

    pub fn record(&self, e: AuditEntry) {
        let mut q = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() >= self.cap {
            q.pop_front();
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
        self.recent()
            .iter()
            .map(|e| {
                format!(
                    "{}\tuid={}\t{:?}\t{}\t{:?}",
                    e.at_ms, e.principal, e.role, e.action, e.outcome
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
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
}
