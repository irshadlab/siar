//! Durable, hash-chained audit file (§161-§162, §70).
//!
//! The in-memory [`AuditLog`](crate::audit::AuditLog) is a 512-entry ring that
//! vanishes at restart. This sink writes every entry to an append-only file:
//!
//! ```text
//! <seq>\t<at_ms>\t<principal>\t<Role>\t<action>\t<Outcome>\t<chain>
//! chain = hex( blake3( previous_chain ‖ "<seq>\t…\t<Outcome>" ) )
//! ```
//!
//! * **Tamper-evident, not tamper-proof.** Editing, deleting or reordering a
//!   line breaks the chain from that point on, and [`verify`] reports where.
//!   Anyone who can rewrite the *whole* file can recompute the chain, and
//!   cutting off the *tail* leaves a valid shorter chain. The defence is to
//!   record [`FileSink::head`] somewhere the node's own user cannot write
//!   (the remote `audit` operation shows it for exactly that purpose).
//! * **Rotation keeps the chain:** the first line of a new segment hashes the
//!   last line of the previous one, so [`verify`] checks across segments.
//! * **Corruption is never silently continued.** On open the existing
//!   segments are verified; if they do not verify they are moved aside to
//!   `audit.corrupt-<ms>/` (kept as evidence) and a **new chain starts with a
//!   visible `audit:chain-restarted` entry**.
//! * Bounded: `max_bytes` per segment, `keep` rotated segments. Files are
//!   mode 0600, the directory 0700.
//! * Writes are best-effort appends (no per-entry fsync): a crash can lose
//!   the last few entries, and a failing disk never blocks or panics the
//!   caller — failures are counted ([`FileSink::write_failures`]).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::audit::AuditEntry;

pub const ACTIVE: &str = "audit.log";

#[derive(Debug, Clone, Copy)]
pub struct FileLimits {
    /// Rotate when the active segment would exceed this.
    pub max_bytes: u64,
    /// Rotated segments kept besides the active one.
    pub keep: usize,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self {
            max_bytes: 1 << 20,
            keep: 4,
        }
    }
}

/// Result of verifying a set of segments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub entries: u64,
    pub first_seq: Option<u64>,
    pub last_seq: Option<u64>,
    /// The chain value after the last entry (the head to anchor off-box).
    pub head: Option<String>,
    /// `None` = every line verified. Otherwise `(segment, line, why)`.
    pub problem: Option<(String, usize, String)>,
}

impl Report {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.problem.is_none()
    }
}

fn to_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn chain(prev: &[u8; 32], body: &str) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(prev);
    h.update(body.as_bytes());
    *h.finalize().as_bytes()
}

/// Defence in depth: the vocabulary is already fixed, but a tab or newline in
/// an action would forge a field or a line, so nothing unprintable survives.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_graphic() { c } else { '_' })
        .take(96)
        .collect()
}

fn body_of(seq: u64, e: &AuditEntry) -> String {
    format!(
        "{seq}\t{}\t{}\t{:?}\t{}\t{:?}",
        e.at_ms,
        e.principal,
        e.role,
        clean(&e.action),
        e.outcome
    )
}

/// Segment paths oldest → newest: `audit.log.N … audit.log.1, audit.log`.
fn segments(dir: &Path, keep: usize) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = (1..=keep + 1)
        .rev()
        .map(|i| dir.join(format!("{ACTIVE}.{i}")))
        .filter(|p| p.is_file())
        .collect();
    let active = dir.join(ACTIVE);
    if active.is_file() {
        v.push(active);
    }
    v
}

/// Verify segments given **oldest first**. If the log starts at sequence 0
/// every line is verified from the genesis value. If older segments have been
/// rotated away, the first remaining line has no known predecessor, so it is
/// the anchor: its own chain value is taken as given and everything after it
/// is checked (the anchor line's *body* cannot be checked — a documented limit).
pub fn verify(files: &[PathBuf]) -> io::Result<Report> {
    let mut rep = Report::default();
    let mut prev: Option<[u8; 32]> = None;
    for f in files {
        let name = f
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let text = fs::read_to_string(f)?;
        for (i, line) in text.lines().enumerate() {
            let fail = |why: &str, mut rep: Report| {
                rep.problem = Some((name.clone(), i + 1, why.to_owned()));
                rep
            };
            let Some((body, mac)) = line.rsplit_once('\t') else {
                return Ok(fail("not an audit line", rep));
            };
            let Some(seq) = body.split('\t').next().and_then(|s| s.parse::<u64>().ok()) else {
                return Ok(fail("bad sequence number", rep));
            };
            if body.split('\t').count() != 6 {
                return Ok(fail("wrong field count", rep));
            }
            let Some(got) = decode_mac(mac) else {
                return Ok(fail("bad chain value", rep));
            };
            if let Some(last) = rep.last_seq {
                if seq != last + 1 {
                    return Ok(fail("sequence gap or reorder", rep));
                }
            }
            // A log that begins at sequence 0 has a KNOWN predecessor (the
            // all-zero genesis value), so even its first line is verified.
            // Only a log whose oldest segments were rotated away has an
            // unverifiable first line, which is then taken as the anchor.
            if prev.is_none() && seq == 0 {
                prev = Some([0; 32]);
            }
            if let Some(p) = prev {
                if chain(&p, body) != got {
                    return Ok(fail("chain mismatch (edited, removed or reordered)", rep));
                }
            }
            prev = Some(got);
            rep.entries += 1;
            rep.first_seq.get_or_insert(seq);
            rep.last_seq = Some(seq);
            rep.head = Some(mac.to_owned());
        }
    }
    Ok(rep)
}

fn decode_mac(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl std::fmt::Debug for FileSink {
    // Deliberately terse: no paths, no chain value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSink")
            .field("next_seq", &self.next_seq)
            .finish_non_exhaustive()
    }
}

pub struct FileSink {
    dir: PathBuf,
    limits: FileLimits,
    file: File,
    size: u64,
    next_seq: u64,
    prev: [u8; 32],
    failures: AtomicU64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn private_dir(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn open_append(p: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

impl FileSink {
    /// Open (or start) the audit file in `dir`. Returns the sink and, if the
    /// existing log failed verification, a description of what was set aside.
    pub fn open(dir: &Path, limits: FileLimits) -> io::Result<(Self, Option<String>)> {
        private_dir(dir)?;
        let segs = segments(dir, limits.keep);
        let rep = verify(&segs)?;
        let mut warning = None;
        let (next_seq, prev, restarted) = if rep.is_ok() {
            match (&rep.last_seq, &rep.head) {
                (Some(seq), Some(h)) => (*seq + 1, decode_mac(h).unwrap_or([0; 32]), false),
                _ => (0, [0; 32], false),
            }
        } else {
            let (seg, line, why) = rep.problem.clone().unwrap_or_default();
            let aside = dir.join(format!("audit.corrupt-{}", now_ms()));
            fs::create_dir_all(&aside)?;
            for s in &segs {
                if let Some(n) = s.file_name() {
                    fs::rename(s, aside.join(n))?;
                }
            }
            warning = Some(format!(
                "audit log failed verification ({seg} line {line}: {why}); set aside in {} and started a new chain",
                aside.display()
            ));
            (0, [0; 32], true)
        };
        let active = dir.join(ACTIVE);
        let file = open_append(&active)?;
        let size = fs::metadata(&active).map(|m| m.len()).unwrap_or(0);
        let mut sink = Self {
            dir: dir.to_owned(),
            limits,
            file,
            size,
            next_seq,
            prev,
            failures: AtomicU64::new(0),
        };
        if restarted {
            // Visible in the log itself, not just in a startup warning.
            sink.append(&AuditEntry {
                at_ms: now_ms(),
                principal: 0,
                role: crate::protocol::Role::Admin,
                action: "audit:chain-restarted".into(),
                outcome: crate::audit::Outcome::Failed,
            });
        }
        Ok((sink, warning))
    }

    /// Append one entry. Never panics and never returns an error: a failing
    /// disk must not take the node down; it is counted instead.
    pub fn append(&mut self, e: &AuditEntry) {
        if self.try_append(e).is_err() {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn try_append(&mut self, e: &AuditEntry) -> io::Result<()> {
        let body = body_of(self.next_seq, e);
        let mac = chain(&self.prev, &body);
        let line = format!("{body}\t{}\n", to_hex(&mac));
        if self.size > 0 && self.size + line.len() as u64 > self.limits.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.size += line.len() as u64;
        self.next_seq += 1;
        self.prev = mac;
        Ok(())
    }

    /// `audit.log` → `.1` → `.2` … ; the oldest beyond `keep` is dropped. The
    /// chain continues in the new segment because `prev` carries over.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.sync_data()?;
        let oldest = self.dir.join(format!("{ACTIVE}.{}", self.limits.keep));
        if oldest.exists() {
            fs::remove_file(&oldest)?;
        }
        for i in (1..self.limits.keep).rev() {
            let from = self.dir.join(format!("{ACTIVE}.{i}"));
            if from.exists() {
                fs::rename(&from, self.dir.join(format!("{ACTIVE}.{}", i + 1)))?;
            }
        }
        if self.limits.keep > 0 {
            fs::rename(self.dir.join(ACTIVE), self.dir.join(format!("{ACTIVE}.1")))?;
        } else {
            fs::remove_file(self.dir.join(ACTIVE))?;
        }
        self.file = open_append(&self.dir.join(ACTIVE))?;
        self.size = 0;
        Ok(())
    }

    /// `(sequence of the last entry, its chain value)` — the head to record
    /// off-box. `None` before the first entry.
    #[must_use]
    pub fn head(&self) -> Option<(u64, String)> {
        (self.next_seq > 0).then(|| (self.next_seq - 1, to_hex(&self.prev)))
    }

    #[must_use]
    pub fn write_failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Verify everything currently on disk in `dir`.
    pub fn verify_dir(dir: &Path, keep: usize) -> io::Result<Report> {
        verify(&segments(dir, keep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Outcome;
    use crate::protocol::Role;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("siar-auditf-{tag}-{}", uuid::Uuid::new_v4()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn entry(i: u64, action: &str) -> AuditEntry {
        AuditEntry {
            at_ms: 1_000 + i,
            principal: 42,
            role: Role::Admin,
            action: action.into(),
            outcome: Outcome::Ok,
        }
    }

    fn fill(sink: &mut FileSink, n: u64) {
        for i in 0..n {
            sink.append(&entry(i, "set-maintenance:on"));
        }
    }

    #[test]
    fn entries_persist_verify_and_resume_the_chain_after_a_restart() {
        let d = tmp("resume");
        let (mut s, w) = FileSink::open(&d, FileLimits::default()).unwrap();
        assert!(w.is_none());
        assert_eq!(s.head(), None);
        fill(&mut s, 5);
        let head = s.head().unwrap();
        assert_eq!(head.0, 4);
        drop(s);

        // Restart: same chain, sequence continues.
        let (mut s, w) = FileSink::open(&d, FileLimits::default()).unwrap();
        assert!(w.is_none());
        assert_eq!(s.head(), Some(head.clone()));
        fill(&mut s, 3);
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert!(rep.is_ok(), "{rep:?}");
        assert_eq!(
            (rep.entries, rep.first_seq, rep.last_seq),
            (8, Some(0), Some(7))
        );
        assert_eq!(rep.head.as_deref(), Some(s.head().unwrap().1.as_str()));
    }

    #[test]
    fn editing_removing_or_reordering_a_line_is_detected_and_located() {
        let d = tmp("tamper");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        fill(&mut s, 6);
        drop(s);
        let path = d.join(ACTIVE);
        let original = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = original.lines().collect();

        // Edit a field (someone rewrites an outcome).
        let mut edited = lines.clone();
        let l3 = edited[3].replace("Ok", "Denied");
        edited[3] = &l3;
        fs::write(&path, edited.join("\n") + "\n").unwrap();
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert_eq!(rep.problem.as_ref().map(|p| p.1), Some(4), "{rep:?}");

        // Remove a middle line.
        let mut removed = lines.clone();
        removed.remove(2);
        fs::write(&path, removed.join("\n") + "\n").unwrap();
        assert!(!FileSink::verify_dir(&d, 4).unwrap().is_ok());

        // Swap two lines.
        let mut swapped = lines.clone();
        swapped.swap(1, 2);
        fs::write(&path, swapped.join("\n") + "\n").unwrap();
        assert!(!FileSink::verify_dir(&d, 4).unwrap().is_ok());

        // Garbage line.
        fs::write(&path, format!("{original}not an audit line\n")).unwrap();
        assert!(!FileSink::verify_dir(&d, 4).unwrap().is_ok());

        // Restoring the original verifies again (the check is deterministic).
        fs::write(&path, &original).unwrap();
        assert!(FileSink::verify_dir(&d, 4).unwrap().is_ok());
    }

    #[test]
    fn truncating_the_tail_is_not_detectable_from_the_file_alone_but_the_anchored_head_shows_it() {
        let d = tmp("tail");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        fill(&mut s, 6);
        let anchored = s.head().unwrap(); // what an operator recorded off-box
        drop(s);
        let path = d.join(ACTIVE);
        let lines: Vec<String> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        fs::write(&path, lines[..4].join("\n") + "\n").unwrap();
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert!(
            rep.is_ok(),
            "a shorter valid chain: this is the documented limit"
        );
        assert_ne!(
            rep.head.as_deref(),
            Some(anchored.1.as_str()),
            "…but it no longer matches the anchor"
        );
        assert!(rep.last_seq.unwrap() < anchored.0);
    }

    #[test]
    fn rotation_keeps_one_continuous_verifiable_chain_and_bounds_the_disk() {
        let d = tmp("rotate");
        let limits = FileLimits {
            max_bytes: 400,
            keep: 2,
        };
        let (mut s, _) = FileSink::open(&d, limits).unwrap();
        fill(&mut s, 60);
        drop(s);
        let names: Vec<String> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.contains(&"audit.log".to_owned()) && names.contains(&"audit.log.1".to_owned())
        );
        assert!(
            !names.iter().any(|n| n == "audit.log.3"),
            "only `keep` rotated segments: {names:?}"
        );
        let total: u64 = names
            .iter()
            .map(|n| fs::metadata(d.join(n)).unwrap().len())
            .sum();
        assert!(total <= 3 * (400 + 120), "disk use is bounded: {total}");
        // The oldest entries rotated away; what remains is one contiguous, valid chain.
        let rep = FileSink::verify_dir(&d, 2).unwrap();
        assert!(rep.is_ok(), "{rep:?}");
        assert_eq!(rep.last_seq, Some(59));
        assert!(rep.first_seq.unwrap() > 0, "old segments were dropped");
        assert_eq!(
            rep.entries,
            rep.last_seq.unwrap() - rep.first_seq.unwrap() + 1
        );
        // And a restart resumes it.
        let (s, w) = FileSink::open(&d, limits).unwrap();
        assert!(w.is_none());
        assert_eq!(s.head().unwrap().0, 59);
    }

    #[test]
    fn a_corrupt_log_is_set_aside_as_evidence_and_a_new_chain_starts_visibly() {
        let d = tmp("corrupt");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        fill(&mut s, 4);
        drop(s);
        let path = d.join(ACTIVE);
        let bad = fs::read_to_string(&path).unwrap().replace("42", "43");
        fs::write(&path, &bad).unwrap();

        let (s, warning) = FileSink::open(&d, FileLimits::default()).unwrap();
        let w = warning.expect("must warn");
        assert!(
            w.contains("failed verification") && w.contains("audit.corrupt-"),
            "{w}"
        );
        drop(s);
        // The evidence is kept, byte for byte.
        let aside: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("audit.corrupt-")
            })
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            fs::read_to_string(aside[0].path().join(ACTIVE)).unwrap(),
            bad
        );
        // The new chain verifies and starts with the visible marker.
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert!(
            rep.is_ok() && rep.entries == 1 && rep.first_seq == Some(0),
            "{rep:?}"
        );
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("audit:chain-restarted"));
    }

    #[test]
    fn hostile_action_text_cannot_forge_fields_or_lines() {
        let d = tmp("forge");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        s.append(&entry(
            0,
            "x\tAdmin\tOk\n999\t0\t0\tAdmin\tforged\tOk\tdeadbeef",
        ));
        drop(s);
        let text = fs::read_to_string(d.join(ACTIVE)).unwrap();
        assert_eq!(text.lines().count(), 1, "no extra line: {text:?}");
        assert!(FileSink::verify_dir(&d, 4).unwrap().is_ok());
        let long = "a".repeat(5000);
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        s.append(&entry(1, &long));
        drop(s);
        assert!(fs::read_to_string(d.join(ACTIVE)).unwrap().len() < 1000);
    }

    #[test]
    fn files_are_private_and_concurrent_writers_keep_the_sequence_contiguous() {
        let d = tmp("conc");
        let (s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&d).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(d.join(ACTIVE)).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let s = std::sync::Arc::new(std::sync::Mutex::new(s));
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        s.lock().unwrap().append(&entry(t * 100 + i, "a"));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert!(rep.is_ok(), "{rep:?}");
        assert_eq!(rep.entries, 400);
        assert_eq!(s.lock().unwrap().write_failures(), 0);
    }

    /// What the chain adds beyond per-line hashes: an attacker who rewrites a
    /// line AND recomputes that line's own hash correctly (the algorithm is
    /// public) still breaks the *next* line, because it hashes this one.
    #[test]
    fn a_rewritten_line_with_a_correct_own_hash_still_breaks_the_next_link() {
        let d = tmp("relink");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        fill(&mut s, 6);
        drop(s);
        let path = d.join(ACTIVE);
        let original = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = original.lines().collect();

        // Line index 3: change the outcome, then recompute its hash properly
        // from the (untouched) previous line's hash.
        let prev_mac = decode_mac(lines[2].rsplit_once('\t').unwrap().1).unwrap();
        let body = lines[3]
            .rsplit_once('\t')
            .unwrap()
            .0
            .replace("Ok", "Denied");
        let forged = format!("{body}\t{}", to_hex(&chain(&prev_mac, &body)));
        let mut out: Vec<String> = lines.iter().map(|l| (*l).to_owned()).collect();
        out[3] = forged;
        fs::write(&path, out.join("\n") + "\n").unwrap();

        let rep = FileSink::verify_dir(&d, 4).unwrap();
        // The forged line itself looks fine; the line AFTER it exposes it.
        assert_eq!(rep.problem.as_ref().map(|p| p.1), Some(5), "{rep:?}");
        assert!(rep.problem.unwrap().2.contains("chain mismatch"));
    }

    #[test]
    fn the_very_first_entry_is_verified_when_the_log_starts_at_genesis() {
        let d = tmp("genesis");
        let (mut s, _) = FileSink::open(&d, FileLimits::default()).unwrap();
        fill(&mut s, 3);
        drop(s);
        let path = d.join(ACTIVE);
        let original = fs::read_to_string(&path).unwrap();
        // Edit ONLY the first entry (no later line is touched).
        let forged = original.replacen("Ok", "Denied", 1);
        assert_ne!(forged, original);
        fs::write(&path, &forged).unwrap();
        let rep = FileSink::verify_dir(&d, 4).unwrap();
        assert_eq!(rep.problem.as_ref().map(|p| p.1), Some(1), "{rep:?}");
        // A single-entry log is protected too.
        let d2 = tmp("genesis1");
        let (mut s, _) = FileSink::open(&d2, FileLimits::default()).unwrap();
        fill(&mut s, 1);
        drop(s);
        let one = fs::read_to_string(d2.join(ACTIVE)).unwrap();
        fs::write(d2.join(ACTIVE), one.replacen("Ok", "Denied", 1)).unwrap();
        assert!(!FileSink::verify_dir(&d2, 4).unwrap().is_ok());
    }
}
