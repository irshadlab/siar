//! The real `siar-desktop` binary, driven like a user would from a shell.
//! Only the backup subcommands run, so no window or network is touched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PASS: &str = "a long enough test passphrase";

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("siar-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `XDG_DATA_HOME/siar` is where the app keeps its data on Linux.
fn make_account(xdg: &Path) -> PathBuf {
    let data = xdg.join("siar");
    std::fs::create_dir_all(data.join("mls")).unwrap();
    std::fs::write(data.join("identity.bin"), [9u8; 64]).unwrap();
    std::fs::write(data.join("account_id.txt"), "account-xyz").unwrap();
    std::fs::write(data.join("device_id.txt"), "device-xyz").unwrap();
    std::fs::write(data.join("mls/index.txt"), "group …\n").unwrap();
    data
}

fn run(xdg: &Path, pass: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_siar-desktop"))
        .args(args)
        .env("XDG_DATA_HOME", xdg)
        .env("SIAR_BACKUP_PASSPHRASE", pass)
        .output()
        .expect("run siar-desktop")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn backup_then_restore_on_a_fresh_machine_brings_the_account_back() {
    let old = scratch("old");
    let data = make_account(&old);
    let archive = scratch("out").join("account.siarbak");

    let o = run(&old, PASS, &["backup", archive.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("Done: 4 files"), "{}", text(&o));

    // the file is not readable plaintext
    let raw = std::fs::read(&archive).unwrap();
    assert!(!raw.windows(11).any(|w| w == b"account-xyz"));

    let info = run(&old, "", &["backup-info", archive.to_str().unwrap()]);
    assert!(info.status.success());
    assert!(text(&info).contains("Argon2id"), "{}", text(&info));

    // "new machine": a different, empty XDG data home
    let fresh = scratch("fresh");
    let o = run(&fresh, PASS, &["restore", archive.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    let restored = fresh.join("siar");
    assert_eq!(
        std::fs::read(restored.join("identity.bin")).unwrap(),
        vec![9u8; 64]
    );
    assert_eq!(
        std::fs::read_to_string(restored.join("account_id.txt")).unwrap(),
        "account-xyz"
    );
    assert_eq!(
        std::fs::read(data.join("identity.bin")).unwrap(),
        vec![9u8; 64]
    );

    // restoring AGAIN must refuse: that machine now has an account
    let o = run(&fresh, PASS, &["restore", archive.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("already a Siar account"), "{}", text(&o));
}

#[test]
fn a_wrong_passphrase_fails_clearly_and_restores_nothing() {
    let old = scratch("wp-old");
    make_account(&old);
    let archive = scratch("wp-out").join("a.siarbak");
    assert!(run(&old, PASS, &["backup", archive.to_str().unwrap()])
        .status
        .success());

    let fresh = scratch("wp-fresh");
    let o = run(
        &fresh,
        "totally different passphrase",
        &["restore", archive.to_str().unwrap()],
    );
    assert!(!o.status.success());
    assert!(text(&o).contains("wrong passphrase"), "{}", text(&o));
    assert!(!fresh.join("siar").exists());
}

#[test]
fn a_weak_passphrase_is_refused_when_creating_a_backup() {
    let old = scratch("weak-old");
    make_account(&old);
    let archive = scratch("weak-out").join("a.siarbak");
    let o = run(&old, "short", &["backup", archive.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("too weak"), "{}", text(&o));
    assert!(!archive.exists());
}

#[test]
fn backing_up_while_the_app_holds_the_data_folder_is_refused() {
    let old = scratch("busy-old");
    let data = make_account(&old);
    let _app_is_running = siar_backup::InstanceLock::acquire(&data).unwrap();
    let archive = scratch("busy-out").join("a.siarbak");
    let o = run(&old, PASS, &["backup", archive.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("running"), "{}", text(&o));
    assert!(!archive.exists());
}

#[test]
fn bad_usage_exits_with_the_usage_text_and_never_starts_the_app() {
    let any = scratch("usage");
    for args in [
        &["frobnicate"][..],
        &["backup"],
        &["restore", "a", "b", "c"],
    ] {
        let o = run(&any, "", args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(text(&o).contains("Usage:"), "{}", text(&o));
    }
    let help = run(&any, "", &["--help"]);
    assert!(help.status.success());
    assert!(text(&help).contains("siar-desktop backup"));
}
