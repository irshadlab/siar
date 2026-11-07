//! §93-§96: structured logs, trace correlation, the diagnostic bundle and
//! the crash report.
//!
//! The common rule is **no content by construction**, not by review:
//!
//! * a [`LogRecord`] carries `subsystem`, `event`, `severity` and an operation
//!   id — and its two text fields are validated to a tiny identifier alphabet
//!   ([`Label`]), so a message body, a ticket or a file name cannot be
//!   passed through them by accident (§93);
//! * a [`DiagnosticBundle`] is assembled from typed parts (versions, health,
//!   redacted config, metrics, recent errors) — there is no field a message
//!   body could occupy (§95);
//! * a [`CrashReport`] keeps the panic *location* and a redacted backtrace,
//!   never the panic *message* (which may quote user data) (§96).

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::config::{RuntimeConfig, SecretsConfig};
use crate::health::{HealthReport, HealthState};
use crate::protocol::PROTOCOL_VERSION;

// ---------------------------------------------------------------- §94 trace

/// §94: the ids that tie a log line to the thing it was about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TraceId {
    Correlation(u128),
    Transfer(u128),
    Bundle(u128),
    Command(u128),
}

impl TraceId {
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Correlation(_) => "corr",
            Self::Transfer(_) => "xfer",
            Self::Bundle(_) => "bundle",
            Self::Command(_) => "cmd",
        }
    }

    const fn value(self) -> u128 {
        match self {
            Self::Correlation(v) | Self::Transfer(v) | Self::Bundle(v) | Self::Command(v) => v,
        }
    }

    /// Parse the `kind:hex` form [`fmt::Display`] produces.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let (kind, hex) = s.split_once(':')?;
        if hex.is_empty() || hex.len() > 32 {
            return None;
        }
        let v = u128::from_str_radix(hex, 16).ok()?;
        Some(match kind {
            "corr" => Self::Correlation(v),
            "xfer" => Self::Transfer(v),
            "bundle" => Self::Bundle(v),
            "cmd" => Self::Command(v),
            _ => return None,
        })
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{:x}", self.kind(), self.value())
    }
}

// ----------------------------------------------------------------- §93 logs

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Debug,
    Info,
    Warn,
    Error,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadLabel;

impl fmt::Display for BadLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("label must be 1-48 chars of a-z, 0-9, '.', '_', '-', ':'")
    }
}

impl std::error::Error for BadLabel {}

/// A short machine identifier (`ipc`, `worker.restart`). Not free text: it
/// is exactly the shape that cannot hold a sentence, a path or a name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Label(String);

impl Label {
    pub const MAX: usize = 48;

    pub fn new(s: &str) -> Result<Self, BadLabel> {
        let ok = !s.is_empty()
            && s.len() <= Self::MAX
            && s.bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b':'));
        ok.then(|| Self(s.to_owned())).ok_or(BadLabel)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Deserialisation re-validates: a hostile or corrupt stored record must not
// smuggle free text in through the data format.
impl<'de> Deserialize<'de> for Label {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

/// §93: `timestamp, subsystem, event, severity, operation id` — nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    pub at_ms: u64,
    pub subsystem: Label,
    pub event: Label,
    pub severity: Severity,
    pub op: Option<TraceId>,
}

impl LogRecord {
    /// Build a record. Returns `None` when a label is not a valid identifier
    /// — callers pass string constants, so this is a programming error in
    /// tests rather than a runtime path.
    #[must_use]
    pub fn new(
        at_ms: u64,
        subsystem: &str,
        event: &str,
        severity: Severity,
        op: Option<TraceId>,
    ) -> Option<Self> {
        Some(Self {
            at_ms,
            subsystem: Label::new(subsystem).ok()?,
            event: Label::new(event).ok()?,
            severity,
            op,
        })
    }

    /// logfmt-style single line; every value is already identifier-safe so
    /// no quoting or escaping is needed (and none can be abused).
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = format!(
            "ts={} sev={} sub={} event={}",
            self.at_ms,
            self.severity.as_str(),
            self.subsystem.as_str(),
            self.event.as_str()
        );
        if let Some(op) = self.op {
            let _ = write!(s, " op={op}");
        }
        s
    }
}

/// Bounded ring of the most recent warnings/errors (for the bundle, §95).
/// Lower severities are not kept — debug chatter must not evict an error.
#[derive(Debug, Clone)]
pub struct RecentErrors {
    inner: Arc<Mutex<VecDeque<LogRecord>>>,
    cap: usize,
}

impl RecentErrors {
    pub const DEFAULT_CAP: usize = 128;

    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Arc::default(),
            cap: cap.max(1),
        }
    }

    pub fn record(&self, r: LogRecord) {
        if r.severity < Severity::Warn {
            return;
        }
        let mut q = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() == self.cap {
            q.pop_front();
        }
        q.push_back(r);
    }

    /// Oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LogRecord> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

impl Default for RecentErrors {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAP)
    }
}

// ------------------------------------------------------------- redaction

/// Replace the user-name component of home-directory paths. A diagnostic
/// bundle gets mailed to people; a backtrace saying `/home/irshad/...` names
/// its owner.
#[must_use]
pub fn redact_paths(text: &str) -> String {
    fn scrub(out: &mut String, prefix: &str, seps: &[char]) {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(prefix) {
            result.push_str(&rest[..i + prefix.len()]);
            rest = &rest[i + prefix.len()..];
            let end = rest.find(seps).unwrap_or(rest.len());
            if end > 0 {
                result.push_str("<redacted>");
            }
            rest = &rest[end..];
        }
        result.push_str(rest);
        *out = result;
    }
    let mut s = text.to_owned();
    scrub(&mut s, "/home/", &['/', '\\', ' ', '"', '\'', ':', '\n']);
    scrub(&mut s, "/Users/", &['/', '\\', ' ', '"', '\'', ':', '\n']);
    scrub(
        &mut s,
        "C:\\Users\\",
        &['\\', '/', ' ', '"', '\'', ':', '\n'],
    );
    scrub(
        &mut s,
        "c:\\users\\",
        &['\\', '/', ' ', '"', '\'', ':', '\n'],
    );
    s
}

/// §95: the config as a bundle may show it. Secret *references* (paths,
/// environment names) are dropped — even a path leaks a user name and a
/// directory layout — and only a count of what was configured remains.
#[must_use]
pub fn redact_config(c: &RuntimeConfig) -> (RuntimeConfig, usize) {
    let configured = usize::from(c.secrets.identity_key.is_some())
        + usize::from(c.secrets.backup_passphrase.is_some());
    let mut r = c.clone();
    r.secrets = SecretsConfig::default();
    (r, configured)
}

// ------------------------------------------------------------ §95 bundle

/// Hard cap on a rendered bundle; it is assembled in memory and may be sent
/// over IPC (`MAX_FRAME` is 1 MiB).
pub const MAX_BUNDLE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    pub generated_at_ms: u64,
    pub build: String,
    pub protocol_version: u16,
    pub state: String,
    pub health: HealthReport,
    /// Redacted config, TOML.
    pub config_toml: String,
    pub secrets_configured: usize,
    /// Prometheus-style text, already free of identifiers (§91).
    pub metrics_text: String,
    pub recent_errors: Vec<LogRecord>,
}

/// Inputs for [`DiagnosticBundle::build`]; borrowed so the caller keeps
/// ownership of its live objects.
pub struct BundleInputs<'a> {
    pub now_ms: u64,
    pub build: &'a str,
    pub state: &'a str,
    pub health: HealthReport,
    pub config: &'a RuntimeConfig,
    pub metrics_text: String,
    pub recent_errors: Vec<LogRecord>,
}

impl DiagnosticBundle {
    #[must_use]
    pub fn build(i: BundleInputs<'_>) -> Self {
        let (cfg, secrets_configured) = redact_config(i.config);
        Self {
            generated_at_ms: i.now_ms,
            // The build id is operator-set; scrub it the same way as
            // everything else rather than trust it.
            build: redact_paths(i.build),
            protocol_version: PROTOCOL_VERSION,
            state: i.state.to_owned(),
            health: i.health,
            config_toml: cfg.to_toml(),
            secrets_configured,
            metrics_text: i.metrics_text,
            recent_errors: i.recent_errors,
        }
    }

    /// Human-readable, bounded. When over the cap the *oldest* errors are
    /// dropped first (the newest are the useful ones) and the truncation is
    /// stated, never silent.
    #[must_use]
    pub fn render_text(&self) -> String {
        let mut keep = self.recent_errors.len();
        loop {
            let s = self.render_with(keep);
            if s.len() <= MAX_BUNDLE_BYTES || keep == 0 {
                return if s.len() <= MAX_BUNDLE_BYTES {
                    s
                } else {
                    // Even with no errors it is too big: cut hard, on a
                    // char boundary, and say so.
                    let mut cut = MAX_BUNDLE_BYTES - 64;
                    while !s.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    format!("{}\n# truncated\n", &s[..cut])
                };
            }
            keep /= 2;
        }
    }

    fn render_with(&self, keep_errors: usize) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# siar diagnostic bundle");
        let _ = writeln!(s, "generated_at_ms = {}", self.generated_at_ms);
        let _ = writeln!(s, "build = {}", self.build);
        let _ = writeln!(s, "protocol_version = {}", self.protocol_version);
        let _ = writeln!(s, "state = {}", self.state);
        let _ = writeln!(
            s,
            "\n# health (overall: {})",
            health_word(self.health.overall)
        );
        for h in &self.health.subsystems {
            let _ = writeln!(
                s,
                "{} {}{} {}",
                h.name,
                health_word(h.state),
                if h.critical { " (critical)" } else { "" },
                redact_paths(&h.detail)
            );
        }
        let _ = writeln!(
            s,
            "\n# config (redacted; {} secret reference(s) not shown)",
            self.secrets_configured
        );
        s.push_str(&self.config_toml);
        let _ = writeln!(s, "\n# metrics");
        s.push_str(&self.metrics_text);
        let total = self.recent_errors.len();
        let skip = total - keep_errors.min(total);
        let _ = writeln!(s, "\n# recent errors (showing {} of {total})", total - skip);
        for r in self.recent_errors.iter().skip(skip) {
            let _ = writeln!(s, "{}", r.render());
        }
        s
    }
}

const fn health_word(h: HealthState) -> &'static str {
    match h {
        HealthState::Healthy => "healthy",
        HealthState::Degraded => "degraded",
        HealthState::Unavailable => "unavailable",
    }
}

// -------------------------------------------------------- §96 crash report

/// §96: stack, build id, subsystem-state summary — redacted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashReport {
    pub at_ms: u64,
    pub build: String,
    /// `file:line` of the panic. **Not** its message: panic messages quote
    /// values (`called unwrap() on Err("alice@…")`).
    pub location: String,
    /// Length of the dropped message, so "empty vs huge" is still visible.
    pub message_len: usize,
    pub backtrace: String,
    /// `name=state` per subsystem, e.g. `dtn=degraded`.
    pub subsystems: Vec<String>,
}

/// Longest backtrace we keep.
pub const MAX_BACKTRACE: usize = 32 * 1024;

impl CrashReport {
    #[must_use]
    pub fn new(
        at_ms: u64,
        build: &str,
        location: Option<(&str, u32)>,
        message_len: usize,
        backtrace: &str,
        health: &HealthReport,
    ) -> Self {
        let mut bt = redact_paths(backtrace);
        if bt.len() > MAX_BACKTRACE {
            let mut cut = MAX_BACKTRACE;
            while !bt.is_char_boundary(cut) {
                cut -= 1;
            }
            bt.truncate(cut);
            bt.push_str("\n… truncated");
        }
        Self {
            at_ms,
            build: redact_paths(build),
            location: location
                .map(|(f, l)| format!("{}:{l}", redact_paths(f)))
                .unwrap_or_else(|| "unknown".into()),
            message_len,
            backtrace: bt,
            subsystems: health
                .subsystems
                .iter()
                .map(|h| format!("{}={}", h.name, health_word(h.state)))
                .collect(),
        }
    }

    #[must_use]
    pub fn render_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# siar crash report");
        let _ = writeln!(s, "at_ms = {}", self.at_ms);
        let _ = writeln!(s, "build = {}", self.build);
        let _ = writeln!(s, "location = {}", self.location);
        let _ = writeln!(
            s,
            "message_len = {} (message not recorded)",
            self.message_len
        );
        let _ = writeln!(s, "subsystems = {}", self.subsystems.join(" "));
        let _ = writeln!(s, "\n{}", self.backtrace);
        s
    }
}

/// How many crash reports are kept on disk.
pub const KEEP_CRASH_REPORTS: usize = 5;

/// Write `report` atomically into `dir` (owner-only) and prune to the newest
/// [`KEEP_CRASH_REPORTS`]. Returns the file written.
pub fn store_crash_report(dir: &Path, report: &CrashReport) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    // Time-ordered, unique per report even within one millisecond.
    let name = format!("crash-{:020}-{}.txt", report.at_ms, std::process::id());
    let path = dir.join(&name);
    let tmp = dir.join(format!("{name}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(report.render_text().as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    prune_crash_reports(dir)?;
    Ok(path)
}

fn prune_crash_reports(dir: &Path) -> io::Result<()> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("crash-") && n.ends_with(".txt"))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(KEEP_CRASH_REPORTS);
    for n in names.into_iter().take(excess) {
        let _ = fs::remove_file(dir.join(n));
    }
    Ok(())
}

/// Install a panic hook that writes a [`CrashReport`] into `dir`, then
/// chains to the previous hook (so the normal stderr message still appears).
///
/// `health` is called inside the hook: keep it cheap and non-blocking
/// (`HealthRegistry::report` takes a short mutex).
pub fn install_panic_hook(
    dir: PathBuf,
    build: String,
    health: impl Fn() -> HealthReport + Send + Sync + 'static,
    now_ms: impl Fn() -> u64 + Send + Sync + 'static,
) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info.location().map(|l| (l.file(), l.line()));
        let msg_len = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.len())
            .or_else(|| info.payload().downcast_ref::<String>().map(String::len))
            .unwrap_or(0);
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        let report = CrashReport::new(now_ms(), &build, loc, msg_len, &bt, &health());
        // Best effort: a crash handler must not itself panic or block.
        let _ = store_crash_report(&dir, &report);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::SubsystemHealth;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-diag-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn health() -> HealthReport {
        HealthReport {
            overall: HealthState::Degraded,
            subsystems: vec![
                SubsystemHealth {
                    name: "storage".into(),
                    state: HealthState::Healthy,
                    detail: String::new(),
                    critical: true,
                },
                SubsystemHealth {
                    name: "dtn".into(),
                    state: HealthState::Degraded,
                    detail: "restarting 2/5 in /home/irshad/data".into(),
                    critical: false,
                },
            ],
        }
    }

    #[test]
    fn labels_cannot_carry_free_text() {
        for bad in [
            "",
            "hello world",
            "alice@example.com",
            "/home/x",
            "Upper",
            "új",
            "a\nb",
            &"x".repeat(49),
        ] {
            assert!(Label::new(bad).is_err(), "{bad:?}");
        }
        for ok in ["ipc", "worker.restart", "dtn-bundle", "a_b", "x:y"] {
            assert!(Label::new(ok).is_ok(), "{ok:?}");
        }
        assert!(LogRecord::new(1, "ipc", "hello there", Severity::Info, None).is_none());
    }

    #[test]
    fn deserialising_a_record_revalidates_its_labels() {
        let good = LogRecord::new(5, "ipc", "accepted", Severity::Warn, None).unwrap();
        let bytes = postcard::to_allocvec(&good).unwrap();
        assert_eq!(postcard::from_bytes::<LogRecord>(&bytes).unwrap(), good);

        // Hand-build a record whose event is a sentence: must not decode.
        #[derive(Serialize)]
        struct Raw<'a> {
            at_ms: u64,
            subsystem: &'a str,
            event: &'a str,
            severity: Severity,
            op: Option<TraceId>,
        }
        let evil = postcard::to_allocvec(&Raw {
            at_ms: 1,
            subsystem: "ipc",
            event: "message from alice: hi",
            severity: Severity::Info,
            op: None,
        })
        .unwrap();
        assert!(postcard::from_bytes::<LogRecord>(&evil).is_err());
    }

    #[test]
    fn record_renders_the_five_spec_fields_and_nothing_else() {
        let r = LogRecord::new(
            1234,
            "dtn",
            "bundle.stored",
            Severity::Info,
            Some(TraceId::Bundle(0xabc)),
        )
        .unwrap();
        assert_eq!(
            r.render(),
            "ts=1234 sev=info sub=dtn event=bundle.stored op=bundle:abc"
        );
        let r = LogRecord::new(1, "ipc", "up", Severity::Debug, None).unwrap();
        assert_eq!(r.render(), "ts=1 sev=debug sub=ipc event=up");
    }

    #[test]
    fn trace_ids_round_trip_and_reject_junk() {
        for t in [
            TraceId::Correlation(1),
            TraceId::Transfer(u128::MAX),
            TraceId::Bundle(0),
            TraceId::Command(0xdead_beef),
        ] {
            assert_eq!(TraceId::parse(&t.to_string()), Some(t));
        }
        for bad in [
            "",
            "cmd:",
            "cmd",
            "nope:1",
            "cmd:zz",
            "cmd:-1",
            &format!("cmd:{}", "f".repeat(33)),
        ] {
            assert_eq!(TraceId::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn recent_errors_keep_only_warnings_and_are_bounded_newest_win() {
        let q = RecentErrors::new(3);
        q.record(LogRecord::new(0, "a", "debug", Severity::Debug, None).unwrap());
        q.record(LogRecord::new(0, "a", "info", Severity::Info, None).unwrap());
        assert!(q.snapshot().is_empty(), "chatter must not be kept");
        for i in 0..5u64 {
            q.record(LogRecord::new(i, "a", "boom", Severity::Error, None).unwrap());
        }
        let snap = q.snapshot();
        assert_eq!(snap.iter().map(|r| r.at_ms).collect::<Vec<_>>(), [2, 3, 4]);
    }

    #[test]
    fn home_directory_user_names_are_scrubbed() {
        let t =
            "at /home/irshad/proj/src/a.rs:10 and /Users/Bob Smith/x, C:\\Users\\alice\\AppData \
                 plus /home/";
        let r = redact_paths(t);
        assert!(
            !r.contains("irshad") && !r.contains("alice") && !r.contains("Bob"),
            "{r}"
        );
        assert!(r.contains("/home/<redacted>/proj/src/a.rs:10"));
        assert!(r.contains("C:\\Users\\<redacted>\\AppData"));
        assert!(r.ends_with("/home/"), "a bare prefix is left alone: {r}");
        // Idempotent.
        assert_eq!(redact_paths(&r), r);
        // Unrelated text is untouched.
        assert_eq!(redact_paths("nothing to see"), "nothing to see");
    }

    #[test]
    fn config_redaction_drops_secret_references_but_keeps_counts() {
        let mut c = RuntimeConfig::default();
        c.secrets.identity_key = Some(crate::config::SecretRef::File {
            path: "/home/irshad/.siar/identity.key".into(),
        });
        let (r, n) = redact_config(&c);
        assert_eq!(n, 1);
        assert!(r.secrets.identity_key.is_none());
        assert!(!r.to_toml().contains("irshad"));
        assert_eq!(r.ipc, c.ipc, "non-secret settings are preserved");
    }

    fn bundle(errors: Vec<LogRecord>) -> DiagnosticBundle {
        let mut cfg = RuntimeConfig::default();
        cfg.secrets.identity_key = Some(crate::config::SecretRef::Env {
            name: "SIAR_SECRET_FOR_IRSHAD".into(),
        });
        DiagnosticBundle::build(BundleInputs {
            now_ms: 99,
            build: "siar 0.1.0 (/home/irshad/target)",
            state: "ready",
            health: health(),
            config: &cfg,
            metrics_text: "siar_ipc_requests_total 3\n".into(),
            recent_errors: errors,
        })
    }

    #[test]
    fn bundle_has_the_five_spec_parts_and_leaks_nothing_personal() {
        let b = bundle(vec![LogRecord::new(
            7,
            "dtn",
            "store.failed",
            Severity::Error,
            None,
        )
        .unwrap()]);
        let t = b.render_text();
        // §95: versions, health, config redacted, metrics snapshot, recent errors.
        assert!(t.contains("build = siar 0.1.0"));
        assert!(t.contains(&format!("protocol_version = {PROTOCOL_VERSION}")));
        assert!(t.contains("# health (overall: degraded)"));
        assert!(t.contains("dtn degraded restarting 2/5"));
        assert!(t.contains("1 secret reference(s) not shown"));
        assert!(t.contains("siar_ipc_requests_total 3"));
        assert!(t.contains("sub=dtn event=store.failed"));
        for leak in ["irshad", "IRSHAD", "SIAR_SECRET"] {
            assert!(!t.contains(leak), "bundle leaked {leak}:\n{t}");
        }
    }

    #[test]
    fn bundle_is_bounded_and_drops_the_oldest_errors_first() {
        let many: Vec<_> = (0..20_000u64)
            .map(|i| {
                LogRecord::new(
                    i,
                    "dtn",
                    "store.failed",
                    Severity::Error,
                    Some(TraceId::Bundle(u128::from(i))),
                )
                .unwrap()
            })
            .collect();
        let b = bundle(many);
        let t = b.render_text();
        assert!(t.len() <= MAX_BUNDLE_BYTES, "{}", t.len());
        assert!(t.contains("ts=19999 "), "the newest error survives");
        assert!(!t.contains("ts=0 "), "the oldest is dropped first");
        assert!(t.contains("showing "), "truncation is stated");
    }

    #[test]
    fn crash_report_keeps_location_but_never_the_message() {
        let r = CrashReport::new(
            10,
            "siar 0.1.0",
            Some(("/home/irshad/siar/src/x.rs", 42)),
            27,
            "0: foo at /home/irshad/siar/src/x.rs:42\n1: bar",
            &health(),
        );
        let t = r.render_text();
        assert!(t.contains("location = /home/<redacted>/siar/src/x.rs:42"));
        assert!(t.contains("message_len = 27 (message not recorded)"));
        assert!(t.contains("storage=healthy dtn=degraded"));
        assert!(!t.contains("irshad"));
        assert!(CrashReport::new(0, "b", None, 0, "", &health()).location == "unknown");
    }

    #[test]
    fn huge_backtraces_are_cut_on_a_char_boundary() {
        let bt = "é".repeat(MAX_BACKTRACE); // 2 bytes each → well over the cap
        let r = CrashReport::new(0, "b", None, 0, &bt, &health());
        assert!(r.backtrace.len() <= MAX_BACKTRACE + 32);
        assert!(r.backtrace.ends_with("truncated"));
    }

    #[test]
    fn crash_reports_are_private_and_pruned_to_the_newest_few() {
        let d = tmp("crash");
        let mut last = PathBuf::new();
        for i in 0..(KEEP_CRASH_REPORTS as u64 + 4) {
            let r = CrashReport::new(i, "b", Some(("f.rs", 1)), 0, "bt", &health());
            last = store_crash_report(&d, &r).unwrap();
        }
        let mut left: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEEP_CRASH_REPORTS, "{left:?}");
        assert!(left.iter().all(|n| !n.ends_with(".tmp")));
        assert!(
            left.last().unwrap().contains("00000000000000000008"),
            "{left:?}"
        );
        assert!(last.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&last).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn the_panic_hook_writes_a_report_without_the_panic_message() {
        let d = tmp("hook");
        let secret = "alice@example.com sent: hunter2";
        install_panic_hook(d.clone(), "test-build".into(), health, || 5);
        let r = std::thread::spawn(move || {
            panic!("{secret}");
        })
        .join();
        assert!(r.is_err());
        // Restore a plain hook so other tests' panics behave normally.
        let _ = std::panic::take_hook();
        let files: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let t = fs::read_to_string(&files[0]).unwrap();
        assert!(t.contains("build = test-build"));
        assert!(t.contains("diag.rs"), "location recorded: {t}");
        assert!(!t.contains("hunter2") && !t.contains("alice@"), "{t}");
        assert!(t.contains("message_len = 31"), "{t}");
    }
}
