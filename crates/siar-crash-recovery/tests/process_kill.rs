//! §122 "Process-Kill Tests": a real child process, a real SIGKILL, a
//! real restart — not a simulated exception.
//!
//! The test binary re-executes itself. In *child mode*
//! (`SIAR_KILL_CHILD_DIR` set) it publishes numbered files in a loop
//! using this crate's `publish_atomic` (temp file → fsync → atomic
//! rename → fsync dir) and reports each committed file on stdout. The
//! parent waits for several commits, kills the child with
//! `Child::kill` (SIGKILL on Unix), then inspects what is on disk and
//! runs the crate's own temp-file recovery over the leftovers.
//!
//! Property checked on the real OS and filesystem: every file that
//! carries a *final* name is complete and uncorrupted, no matter where
//! the kill landed; anything partial is a recognisable temp file that
//! §23/§24 classify and clean up.

use siar_crash_recovery::{
    classify_temp_file, cleanup_decision, publish_atomic, CleanupDecision, CleanupPolicy,
    OpaqueTempId, RuntimeGeneration, ScanContext, TempArea, TempClass, TempFileEntry,
};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CHILD_ENV: &str = "SIAR_KILL_CHILD_DIR";
const PAYLOAD_LEN: usize = 256 * 1024;

fn payload(i: u32) -> Vec<u8> {
    vec![(i % 251) as u8 + 1; PAYLOAD_LEN]
}

/// Child entry point. A no-op when run as an ordinary test.
#[test]
fn child_writer() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let out = std::io::stdout();
    for i in 0..u32::MAX {
        let tmp = dir.join(OpaqueTempId::new().file_name());
        {
            let mut f = std::fs::File::create(&tmp).unwrap();
            f.write_all(&payload(i)).unwrap();
            f.sync_all().unwrap();
        }
        publish_atomic(&tmp, &dir.join(format!("final-{i:08}"))).unwrap();
        let mut o = out.lock();
        writeln!(o, "committed {i}").unwrap();
        o.flush().unwrap();
    }
}

fn scratch() -> PathBuf {
    let d = std::env::temp_dir().join(format!("siar-kill-{}-{}", std::process::id(), uuid_like()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!(
        "{:x}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn spawn_child(dir: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["child_writer", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child test process")
}

/// Waits until the child reports `n` committed files; returns how many
/// it had reported when we stopped reading.
fn wait_for_commits(child: &mut std::process::Child, n: u32) -> u32 {
    let stdout = child.stdout.take().expect("piped stdout");
    let mut seen = 0;
    for line in BufReader::new(stdout).lines() {
        let line = line.unwrap();
        if line.starts_with("committed ") {
            seen += 1;
            if seen >= n {
                break;
            }
        }
    }
    seen
}

#[test]
fn sigkill_mid_publish_never_leaves_a_partial_final_file() {
    let dir = scratch();
    let mut child = spawn_child(&dir);
    let seen = wait_for_commits(&mut child, 15);
    assert!(
        seen >= 15,
        "child produced only {seen} commits before exiting"
    );

    // SIGKILL on Unix: no cleanup handlers, no flush, no atomicity
    // help from the process itself.
    child.kill().expect("kill child");
    let status = child.wait().expect("reap child");
    assert!(
        !status.success(),
        "child must have been killed, not exited cleanly"
    );

    // ---- "restart": inspect real on-disk state ----
    let mut finals = Vec::new();
    let mut temps = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let e = e.unwrap();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("final-") {
            finals.push(name);
        } else {
            temps.push((name, e.metadata().unwrap()));
        }
    }
    finals.sort();
    assert!(
        finals.len() >= 15,
        "at least the reported commits are present"
    );

    // 1. every final-named file is complete and uncorrupted
    for name in &finals {
        let i: u32 = name.trim_start_matches("final-").parse().unwrap();
        let data = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(data.len(), PAYLOAD_LEN, "{name}: partial final file");
        assert_eq!(data, payload(i), "{name}: corrupted final file");
    }

    // 2. finals are a gap-free prefix 0..n (commits are sequential)
    for (idx, name) in finals.iter().enumerate() {
        assert_eq!(
            name,
            &format!("final-{idx:08}"),
            "gap in committed sequence"
        );
    }

    // 3. anything else is at most one recognisable temp file, and the
    //    crate's own recovery scan classifies it correctly
    assert!(temps.len() <= 1, "more than one leftover: {temps:?}");
    let policy = CleanupPolicy::new(1, 60_000).unwrap();
    let active = HashSet::new();
    for (name, _meta) in &temps {
        let id = OpaqueTempId::parse_file_name(name)
            .unwrap_or_else(|| panic!("leftover {name} is not an opaque temp name"));
        let entry = TempFileEntry {
            area: TempArea::Staging,
            id: Some(id),
            ownership: None,
            created_at_millis: 0,
            corrupt: false,
        };
        let ctx = ScanContext {
            now_millis: 10_000,
            current_generation: RuntimeGeneration(2),
            active_transfers: &active,
            policy,
        };
        let class = classify_temp_file(&entry, &ctx);
        // unowned and not yet at hard expiry → kept until expiry, never
        // deleted on sight (§24)
        assert_eq!(class, TempClass::Orphan);
        assert!(matches!(
            cleanup_decision(&entry, class),
            CleanupDecision::Keep(_)
        ));
        // once past hard expiry it is deletable
        let late = ScanContext {
            now_millis: 120_000,
            ..ctx
        };
        let late_class = classify_temp_file(&entry, &late);
        assert_eq!(
            cleanup_decision(&entry, late_class),
            CleanupDecision::Delete
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_restarted_writer_continues_alongside_the_survivors() {
    // After the kill, a fresh process can publish into the same
    // directory: leftovers don't block recovery.
    let dir = scratch();
    let mut child = spawn_child(&dir);
    wait_for_commits(&mut child, 5);
    child.kill().unwrap();
    child.wait().unwrap();

    let tmp = dir.join(OpaqueTempId::new().file_name());
    std::fs::write(&tmp, payload(9_999)).unwrap();
    publish_atomic(&tmp, &dir.join("final-restarted")).unwrap();
    assert_eq!(
        std::fs::read(dir.join("final-restarted")).unwrap(),
        payload(9_999)
    );
    std::fs::remove_dir_all(&dir).ok();
}
