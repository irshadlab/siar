//! §91-§92: local metrics. Plain atomic counters, rendered in the text
//! exposition format so any scraper (or `grep`) can read them. **Local only**:
//! exposed through the authenticated IPC `Query::Metrics`, never on a network
//! port. Counters carry no identifiers, content or per-peer labels.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Debug, Default)]
pub struct Metrics {
    pub ipc_connections: AtomicU64,
    pub ipc_requests: AtomicU64,
    pub ipc_rate_limited: AtomicU64,
    pub ipc_unauthorized: AtomicU64,
    pub commands: AtomicU64,
    pub commands_deduplicated: AtomicU64,
    pub worker_restarts: AtomicU64,
    pub worker_gave_up: AtomicU64,
}

impl Metrics {
    pub fn inc(c: &AtomicU64) {
        c.fetch_add(1, Relaxed);
    }

    /// `# TYPE` + value per counter, stable order.
    #[must_use]
    pub fn render_text(&self) -> String {
        let rows: [(&str, &str, u64); 8] = [
            (
                "siar_ipc_connections_total",
                "IPC connections accepted",
                self.ipc_connections.load(Relaxed),
            ),
            (
                "siar_ipc_requests_total",
                "IPC requests handled",
                self.ipc_requests.load(Relaxed),
            ),
            (
                "siar_ipc_rate_limited_total",
                "IPC requests refused by the rate limit",
                self.ipc_rate_limited.load(Relaxed),
            ),
            (
                "siar_ipc_unauthorized_total",
                "IPC requests refused for lack of role",
                self.ipc_unauthorized.load(Relaxed),
            ),
            (
                "siar_commands_total",
                "Mutating commands executed",
                self.commands.load(Relaxed),
            ),
            (
                "siar_commands_deduplicated_total",
                "Retried commands answered from the ledger",
                self.commands_deduplicated.load(Relaxed),
            ),
            (
                "siar_worker_restarts_total",
                "Supervised worker restarts",
                self.worker_restarts.load(Relaxed),
            ),
            (
                "siar_worker_gave_up_total",
                "Workers parked by the circuit breaker",
                self.worker_gave_up.load(Relaxed),
            ),
        ];
        let mut out = String::new();
        for (name, help, v) in rows {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {v}"
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_counter_with_help_and_type() {
        let m = Metrics::default();
        Metrics::inc(&m.ipc_requests);
        Metrics::inc(&m.ipc_requests);
        let t = m.render_text();
        assert!(t.contains("# TYPE siar_ipc_requests_total counter\nsiar_ipc_requests_total 2"));
        assert_eq!(t.matches("# TYPE").count(), 8);
    }

    #[test]
    fn no_identifiers_or_labels() {
        let t = Metrics::default().render_text();
        assert!(!t.contains('{') && !t.contains("peer") && !t.contains("uid"));
    }
}
