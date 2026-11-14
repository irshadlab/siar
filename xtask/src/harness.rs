//! Test harness for black-box scenarios: isolated per-node XDG directories,
//! process spawning, polling instead of `sleep`, and assertions that collect
//! every failure before reporting. Replaces the ad-hoc bash in the old
//! `scripts/*.sh`.

use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// Where the built binaries are.
pub struct Bins {
    dir: PathBuf,
}

impl Bins {
    /// Build `packages` (unless `build` is false) and locate `target/debug`.
    pub fn locate(build: bool, packages: &[&str]) -> Result<Self> {
        let meta = cargo_metadata::MetadataCommand::new().no_deps().exec()?;
        if build {
            let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
            cmd.arg("build");
            for p in packages {
                cmd.args(["-p", p]);
            }
            let st = cmd.status().context("running cargo build")?;
            if !st.success() {
                bail!("cargo build failed");
            }
        }
        Ok(Self {
            dir: PathBuf::from(meta.target_directory).join("debug"),
        })
    }

    pub fn path(&self, name: &str) -> Result<PathBuf> {
        let p = self.dir.join(name);
        if p.is_file() {
            Ok(p)
        } else {
            Err(anyhow!(
                "{} not found — build it or drop --no-build",
                p.display()
            ))
        }
    }
}

pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// One daemon with its own data/runtime/config directories.
pub struct Node {
    pub name: String,
    pub root: PathBuf,
    daemon: PathBuf,
}

impl Node {
    pub fn new(name: &str, bins: &Bins) -> Result<Self> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("siar-xtask-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["data", "run", "cfg/siar", "home"] {
            fs::create_dir_all(root.join(d))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            name: name.to_owned(),
            root,
            daemon: bins.path("siar-daemon")?,
        })
    }

    pub fn socket(&self) -> PathBuf {
        self.root.join("run/siar/daemon.sock")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("cfg/siar/daemon.toml")
    }

    pub fn write_config(&self, text: &str) -> Result<()> {
        fs::write(self.config_path(), text)?;
        Ok(())
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(&self.daemon);
        c.args(args)
            .env("RUST_BACKTRACE", "0")
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("XDG_CONFIG_HOME", self.root.join("cfg"));
        c
    }

    /// Run a one-shot daemon subcommand to completion.
    pub fn run(&self, args: &[&str]) -> Result<Out> {
        let o = self
            .command(args)
            .output()
            .context("running the daemon binary")?;
        Ok(Out {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        })
    }

    /// `siar-daemon ctl …`
    pub fn ctl(&self, args: &[&str]) -> Result<Out> {
        let mut a = vec!["ctl"];
        a.extend_from_slice(args);
        self.run(&a)
    }

    /// Start `siar-daemon run …` in the background; its log goes to `<tag>.log`.
    pub fn spawn(&self, args: &[&str], tag: &str) -> Result<Child> {
        let log = fs::File::create(self.root.join(format!("{tag}.log")))?;
        let mut a = vec!["run"];
        a.extend_from_slice(args);
        self.command(&a)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("spawning the daemon")
    }

    pub fn log(&self, tag: &str) -> String {
        strip_ansi(&fs::read_to_string(self.root.join(format!("{tag}.log"))).unwrap_or_default())
    }

    /// Wait until the daemon answers `ctl status` with `state: Ready`.
    pub fn wait_ready(&self, timeout: Duration) -> Result<()> {
        poll(timeout, || {
            self.socket().exists()
                && self
                    .ctl(&["status"])
                    .map(|o| o.ok && o.stdout.contains("Ready"))
                    .unwrap_or(false)
        })
        .with_context(|| {
            format!(
                "{} did not become Ready (see {}/*.log)",
                self.name,
                self.root.display()
            )
        })
    }

    /// Ask for a graceful stop and return the exit code.
    pub fn stop(&self, child: &mut Child) -> Result<Option<i32>> {
        let _ = self.ctl(&["shutdown"]);
        wait_exit(child, Duration::from_secs(20))
    }

    /// Send a signal (Unix `kill`, no `unsafe`).
    pub fn signal(&self, child: &Child, sig: &str) -> Result<()> {
        let st = Command::new("kill")
            .args([&format!("-{sig}"), &child.id().to_string()])
            .status()?;
        if st.success() {
            Ok(())
        } else {
            bail!("kill -{sig} failed")
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if std::env::var_os("SIAR_XTASK_KEEP").is_none() {
            let _ = fs::remove_dir_all(&self.root);
        } else {
            eprintln!("kept {}", self.root.display());
        }
    }
}

pub fn wait_exit(child: &mut Child, timeout: Duration) -> Result<Option<i32>> {
    let end = Instant::now() + timeout;
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(st.code());
        }
        if Instant::now() >= end {
            let _ = child.kill();
            bail!("process did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Poll `f` until it returns true.
pub fn poll(timeout: Duration, mut f: impl FnMut() -> bool) -> Result<()> {
    let end = Instant::now() + timeout;
    loop {
        if f() {
            return Ok(());
        }
        if Instant::now() >= end {
            bail!("condition not met within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' && it.peek() == Some(&'[') {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `(pid, command line)` of every direct child of `parent`, read from `/proc`
/// (Linux). Pure Rust — no `pgrep`/`ps`.
pub fn children_of(parent: u32) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else {
        return out;
    };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        if parse_ppid(&stat) == Some(parent) {
            let cmd = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let cmd = cmd
                .split(|b| *b == 0)
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            out.push((pid, cmd.trim().to_owned()));
        }
    }
    out
}

/// `pid (comm) state ppid …` — `comm` may contain spaces and parentheses, so
/// split after the LAST `)`.
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

pub fn sockets_in(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".sock"))
                .count()
        })
        .unwrap_or(0)
}

/// Collects every failed assertion, prints a ✓/✗ line for each.
#[derive(Default)]
pub struct Check {
    failed: Vec<String>,
    total: usize,
}

impl Check {
    pub fn new(title: &str) -> Self {
        println!("== {title}");
        Self::default()
    }

    pub fn step(&self, what: impl Display) {
        println!("-- {what}");
    }

    pub fn ok(&mut self, cond: bool, what: impl Display) -> bool {
        self.total += 1;
        if cond {
            println!("  ✓ {what}");
        } else {
            println!("  ✗ {what}");
            self.failed.push(what.to_string());
        }
        cond
    }

    pub fn finish(self) -> Result<()> {
        if self.failed.is_empty() {
            println!("\nall {} checks passed", self.total);
            Ok(())
        } else {
            bail!(
                "{} of {} checks failed:\n  - {}",
                self.failed.len(),
                self.total,
                self.failed.join("\n  - ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_is_stripped_and_plain_text_is_untouched() {
        assert_eq!(strip_ansi("\u{1b}[32m INFO\u{1b}[0m ready"), " INFO ready");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi("\u{1b}[1;31merr\u{1b}[0m: x"), "err: x");
    }

    #[test]
    fn ppid_is_parsed_even_when_the_command_name_is_hostile() {
        assert_eq!(parse_ppid("123 (siar-daemon) S 77 123 123 0"), Some(77));
        assert_eq!(
            parse_ppid("123 (a b) c) S 88 1 1 0"),
            Some(88),
            "spaces and ')' inside comm"
        );
        assert_eq!(parse_ppid("garbage"), None);
        assert_eq!(parse_ppid("1 (x) S"), None);
    }

    #[test]
    fn our_own_children_are_found() {
        let mut c = Command::new("sleep").arg("5").spawn().unwrap();
        let found = children_of(std::process::id())
            .iter()
            .any(|(p, _)| *p == c.id());
        let _ = c.kill();
        let _ = c.wait();
        assert!(found, "a spawned child must appear in /proc scanning");
    }

    #[test]
    fn poll_returns_when_true_and_times_out_otherwise() {
        let mut n = 0;
        assert!(poll(Duration::from_secs(2), || {
            n += 1;
            n > 2
        })
        .is_ok());
        assert!(poll(Duration::from_millis(250), || false).is_err());
    }

    #[test]
    fn check_collects_every_failure_before_reporting() {
        let mut c = Check::default();
        assert!(c.ok(true, "fine"));
        assert!(!c.ok(false, "first problem"));
        assert!(!c.ok(false, "second problem"));
        let e = c.finish().unwrap_err().to_string();
        assert!(
            e.contains("2 of 3") && e.contains("first problem") && e.contains("second problem"),
            "{e}"
        );
    }
}
