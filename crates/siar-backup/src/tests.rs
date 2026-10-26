use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

const FAST: KdfParams = KdfParams {
    mem_kib: 8,
    iterations: 1,
    lanes: 1,
};
const PASS: &str = "correct horse battery staple";

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "siar-backup-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// A data directory shaped like the real one.
fn data_dir(name: &str) -> PathBuf {
    let d = scratch(name).join("data");
    fs::create_dir_all(d.join("siar.db/wal")).unwrap();
    fs::create_dir_all(d.join("mls")).unwrap();
    fs::write(d.join("identity.bin"), [7u8; 64]).unwrap();
    fs::write(d.join("account_id.txt"), "acct-1111").unwrap();
    fs::write(d.join("device_id.txt"), "dev-2222").unwrap();
    fs::write(
        d.join("siar.db/wal/000001.log"),
        (0..=255u8).collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write(d.join("mls/index.txt"), "group a b c d\n").unwrap();
    fs::write(d.join("mls/empty.sqlite3"), []).unwrap();
    d
}

fn read_tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                let rel = p
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel != "siar.lock" {
                    out.push((rel, fs::read(&p).unwrap()));
                }
            }
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

fn backup(data: &Path, out_dir_name: &str) -> PathBuf {
    let out = scratch(out_dir_name).join("b.siarbak");
    create_backup_with(data, PASS, &out, FAST).unwrap();
    out
}

#[test]
fn a_backup_restores_every_file_byte_for_byte() {
    let data = data_dir("roundtrip");
    let archive = backup(&data, "rt-out");
    let target = scratch("rt-target").join("restored");
    let summary = restore_backup(&archive, PASS, &target).unwrap();
    assert_eq!(summary.files, 6);
    assert_eq!(read_tree(&target), read_tree(&data));
}

#[test]
fn bookkeeping_files_and_links_are_not_backed_up() {
    let data = data_dir("skip");
    fs::write(data.join("siar.lock"), "").unwrap();
    fs::write(data.join("ui-settings.tmp"), "half written").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", data.join("link-out")).unwrap();
    let archive = backup(&data, "skip-out");
    let target = scratch("skip-target").join("r");
    restore_backup(&archive, PASS, &target).unwrap();
    let names: Vec<String> = read_tree(&target).into_iter().map(|(n, _)| n).collect();
    assert!(!names
        .iter()
        .any(|n| n.contains("siar.lock") || n.ends_with(".tmp") || n.contains("link-out")));
}

#[test]
fn the_wrong_passphrase_is_refused_and_writes_nothing() {
    let archive = backup(&data_dir("wrongpass"), "wp-out");
    let target = scratch("wp-target").join("restored");
    let err = restore_backup(&archive, "another long passphrase", &target).unwrap_err();
    assert!(matches!(err, BackupError::Unreadable));
    assert!(
        !target.exists(),
        "a failed restore must not create the target"
    );
}

#[test]
fn flipping_any_single_bit_anywhere_in_the_file_is_detected() {
    let archive = backup(&data_dir("flip"), "flip-out");
    let original = fs::read(&archive).unwrap();
    let target_root = scratch("flip-target");
    for i in 0..original.len() {
        let mut damaged = original.clone();
        damaged[i] ^= 0x01;
        let path = target_root.join("damaged.siarbak");
        fs::write(&path, &damaged).unwrap();
        let r = restore_backup(&path, PASS, &target_root.join(format!("t{i}")));
        assert!(r.is_err(), "a bit flip at byte {i} went unnoticed");
    }
}

#[test]
fn truncation_at_any_length_is_an_error_not_a_partial_restore() {
    let archive = backup(&data_dir("trunc"), "trunc-out");
    let original = fs::read(&archive).unwrap();
    let root = scratch("trunc-target");
    for len in [
        0,
        1,
        7,
        8,
        20,
        HEADER_LEN - 1,
        HEADER_LEN,
        HEADER_LEN + 1,
        original.len() / 2,
        original.len() - 1,
    ] {
        let path = root.join("t.siarbak");
        fs::write(&path, &original[..len]).unwrap();
        let target = root.join(format!("t{len}"));
        assert!(
            restore_backup(&path, PASS, &target).is_err(),
            "length {len}"
        );
        assert!(!target.exists());
    }
}

#[test]
fn weak_passphrases_are_refused_before_anything_is_written() {
    let data = data_dir("weak");
    let out = scratch("weak-out").join("b.siarbak");
    for bad in [
        "",
        "short",
        "12345678901",
        "aaaaaaaaaaaaaaaa",
        "            ",
    ] {
        let err = create_backup_with(&data, bad, &out, FAST).unwrap_err();
        assert!(matches!(err, BackupError::WeakPassphrase(_)), "{bad:?}");
    }
    assert!(!out.exists());
    // a long passphrase is fine, and so is one with non-ASCII characters
    create_backup_with(&data, "pässwörd with ünicode ✓ ok", &out, FAST).unwrap();
    let target = scratch("weak-target").join("r");
    restore_backup(&out, "pässwörd with ünicode ✓ ok", &target).unwrap();
}

#[test]
fn restore_refuses_to_overwrite_an_existing_account() {
    let data = data_dir("overwrite");
    let archive = backup(&data, "ow-out");
    // restoring on top of the live data folder is exactly the mistake to stop
    let err = restore_backup(&archive, PASS, &data).unwrap_err();
    assert!(matches!(err, BackupError::TargetNotEmpty(_)));
    assert_eq!(
        fs::read(data.join("identity.bin")).unwrap(),
        vec![7u8; 64],
        "untouched"
    );
    // an existing but EMPTY folder (or one with only a stale lock) is fine
    let empty = scratch("ow-empty");
    restore_backup(&archive, PASS, &empty).unwrap();
    let lock_only = scratch("ow-lock");
    fs::write(lock_only.join("siar.lock"), "").unwrap();
    restore_backup(&archive, PASS, &lock_only).unwrap();
    assert!(lock_only.join("identity.bin").exists());
}

fn craft(entries: Vec<(&str, &[u8])>) -> PathBuf {
    let entries = entries
        .into_iter()
        .map(|(p, d)| Entry {
            path: p.to_string(),
            data: d.to_vec(),
        })
        .collect();
    let bytes = seal(entries, PASS, FAST, 1).unwrap();
    let path = scratch("craft").join("evil.siarbak");
    fs::write(&path, bytes).unwrap();
    path
}

fn base_entries() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("identity.bin", b"k"),
        ("account_id.txt", b"a"),
        ("device_id.txt", b"d"),
    ]
}

#[test]
fn path_traversal_and_other_unsafe_paths_are_rejected_before_any_write() {
    for evil in [
        "../escape.txt",
        "/etc/cron.d/x",
        "a/../../b",
        "",
        "a\\b",
        "ok/../../x",
        "x\0y",
    ] {
        let mut entries = base_entries();
        entries.push((evil, b"pwned"));
        let archive = craft(entries);
        let parent = scratch("evil-target");
        let target = parent.join("restored");
        let err = restore_backup(&archive, PASS, &target).unwrap_err();
        assert!(
            matches!(err, BackupError::UnsafePath(_)),
            "{evil:?} → {err:?}"
        );
        assert!(!target.exists(), "{evil:?} created the target");
        assert!(!parent.join("escape.txt").exists());
        assert!(!parent.parent().unwrap().join("escape.txt").exists());
    }
}

#[test]
fn duplicate_paths_and_missing_identity_files_are_rejected() {
    let mut dup = base_entries();
    dup.push(("identity.bin", b"second"));
    let err = restore_backup(&craft(dup), PASS, &scratch("dup").join("r")).unwrap_err();
    assert!(matches!(err, BackupError::UnsafePath(_)));

    for missing in REQUIRED {
        let entries: Vec<_> = base_entries()
            .into_iter()
            .filter(|(p, _)| p != missing)
            .collect();
        let err = restore_backup(&craft(entries), PASS, &scratch("miss").join("r")).unwrap_err();
        assert!(matches!(err, BackupError::MissingRequired(m) if m == *missing));
    }
}

#[test]
fn a_backup_cannot_be_written_inside_the_folder_it_backs_up() {
    let data = data_dir("inside");
    let err = create_backup_with(&data, PASS, &data.join("mine.siarbak"), FAST).unwrap_err();
    assert!(matches!(err, BackupError::OutputInsideData));
    let err = create_backup_with(&data, PASS, &data.join("mls/b.siarbak"), FAST).unwrap_err();
    assert!(matches!(err, BackupError::OutputInsideData));
}

#[test]
fn a_folder_without_an_identity_is_not_backed_up_as_if_it_were_an_account() {
    let d = scratch("noident").join("data");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("account_id.txt"), "x").unwrap();
    let out = scratch("noident-out").join("b");
    let err = create_backup_with(&d, PASS, &out, FAST).unwrap_err();
    assert!(matches!(err, BackupError::MissingRequired(_)));
    assert!(!out.exists());
}

#[test]
fn a_backup_of_a_running_app_is_refused() {
    let data = data_dir("running");
    let _running = InstanceLock::acquire(&data).unwrap(); // what the app holds
    let out = scratch("running-out").join("b");
    let err = create_backup_with(&data, PASS, &out, FAST).unwrap_err();
    assert!(matches!(err, BackupError::InstanceRunning));
    assert!(!out.exists());
}

#[test]
fn the_instance_lock_is_exclusive_and_released_on_drop() {
    let d = scratch("lock");
    let first = InstanceLock::acquire(&d).unwrap();
    assert!(matches!(InstanceLock::acquire(&d), Err(LockError::Held)));
    drop(first);
    let again = InstanceLock::acquire(&d).unwrap();
    drop(again);
}

#[test]
fn hostile_kdf_parameters_are_refused_without_doing_the_work() {
    let archive = backup(&data_dir("kdf"), "kdf-out");
    let original = fs::read(&archive).unwrap();
    let root = scratch("kdf-target");
    // mem_kib = 0xFFFFFFFF (≈4 TiB), then iterations/lanes absurd
    for (offset, value) in [
        (10usize, u32::MAX),
        (14, u32::MAX),
        (18, u32::MAX),
        (10, 0),
        (14, 0),
        (18, 0),
    ] {
        let mut b = original.clone();
        b[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        let path = root.join("h.siarbak");
        fs::write(&path, &b).unwrap();
        let started = std::time::Instant::now();
        assert!(restore_backup(&path, PASS, &root.join("t")).is_err());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "did the hostile work"
        );
    }
}

#[test]
fn inspect_reads_the_header_without_a_passphrase_and_rejects_other_files() {
    let archive = backup(&data_dir("inspect"), "inspect-out");
    let info = inspect(&archive).unwrap();
    assert_eq!(info.version, 1);
    assert_eq!(info.kdf, FAST);
    assert!(info.created_at_millis > 1_600_000_000_000);
    assert!(info.encrypted_bytes > 0);

    let junk = scratch("inspect-junk").join("x");
    fs::write(&junk, b"this is not a backup at all").unwrap();
    assert!(matches!(inspect(&junk), Err(BackupError::NotABackup)));
    assert!(matches!(
        restore_backup(&junk, PASS, &scratch("inspect-t").join("r")),
        Err(BackupError::NotABackup)
    ));

    let mut newer = fs::read(&archive).unwrap();
    newer[8] = 9;
    fs::write(&junk, newer).unwrap();
    assert!(matches!(
        inspect(&junk),
        Err(BackupError::UnsupportedVersion(9))
    ));
}

#[test]
fn two_backups_of_the_same_data_differ_so_nothing_is_reused() {
    let data = data_dir("fresh");
    let (a, b) = (backup(&data, "fresh-a"), backup(&data, "fresh-b"));
    let (a, b) = (fs::read(a).unwrap(), fs::read(b).unwrap());
    // fresh salt and nonce every time
    assert_ne!(a[30..HEADER_LEN], b[30..HEADER_LEN]);
    assert_ne!(a[HEADER_LEN..], b[HEADER_LEN..]);
}

#[test]
fn a_multi_megabyte_database_round_trips() {
    let data = data_dir("big");
    let blob: Vec<u8> = (0..5_000_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    fs::write(data.join("siar.db/wal/000002.log"), &blob).unwrap();
    let archive = backup(&data, "big-out");
    let target = scratch("big-target").join("r");
    restore_backup(&archive, PASS, &target).unwrap();
    assert_eq!(
        fs::read(target.join("siar.db/wal/000002.log")).unwrap(),
        blob
    );
}

#[test]
fn restoring_into_a_folder_a_running_app_is_using_is_refused() {
    let archive = backup(&data_dir("busy-src"), "busy-out");
    let busy = scratch("busy-target");
    let _running = InstanceLock::acquire(&busy).unwrap();
    let err = restore_backup(&archive, PASS, &busy).unwrap_err();
    assert!(matches!(err, BackupError::InstanceRunning));
    assert!(!busy.join("identity.bin").exists());
}
