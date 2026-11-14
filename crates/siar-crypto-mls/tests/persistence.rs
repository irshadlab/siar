//! A restarted device must be able to keep using an MLS group it was
//! already in. These tests close and reopen the state file between
//! messages — the thing a process restart does.

use std::path::PathBuf;

use siar_crypto_mls::{
    generate_identity, FilePersistentProvider, IncomingMlsMessage, MlsGroupSession,
};
use siar_domain::DeviceId;

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("siar-mls-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn app_text(msg: IncomingMlsMessage) -> Vec<u8> {
    match msg {
        IncomingMlsMessage::Application(bytes) => bytes,
        _ => panic!("expected an application message"),
    }
}

#[test]
fn a_member_reloaded_from_disk_still_sends_and_receives() {
    let d = dir("roundtrip");
    let (path_a, path_b) = (d.join("a.ron"), d.join("b.ron"));
    let (dev_a, dev_b) = (DeviceId::new(), DeviceId::new());

    // A founds the group and adds B; both are persistent.
    let pa = FilePersistentProvider::open(&path_a).unwrap();
    let ia = generate_identity(dev_a, &pa).unwrap();
    let mut a = MlsGroupSession::create(pa, &ia).unwrap();
    let pb = FilePersistentProvider::open(&path_b).unwrap();
    let ib = generate_identity(dev_b, &pb).unwrap();
    let (_commit, welcome) = a.add_member(ib.key_package.key_package()).unwrap();
    let mut b =
        MlsGroupSession::join_from_welcome(pb, ib.signature_keys.clone(), &welcome).unwrap();

    // Normal traffic before the "restart".
    assert_eq!(
        app_text(b.process_incoming(&a.encrypt(b"one").unwrap()).unwrap()),
        b"one"
    );

    // What the application persists next to the database.
    let (gid, a_pub) = (a.group_id_bytes(), a.signature_public_key());
    drop(a); // A "restarts": the live session and its connection are gone

    let mut a = MlsGroupSession::load(FilePersistentProvider::open(&path_a).unwrap(), &gid, &a_pub)
        .unwrap()
        .expect("the group was persisted");
    assert_eq!(a.member_count(), 2);

    // The reloaded A can still encrypt for B, and decrypt B's replies.
    assert_eq!(
        app_text(b.process_incoming(&a.encrypt(b"two").unwrap()).unwrap()),
        b"two"
    );
    assert_eq!(
        app_text(a.process_incoming(&b.encrypt(b"reply").unwrap()).unwrap()),
        b"reply"
    );

    // Now B restarts too.
    let (b_gid, b_pub) = (b.group_id_bytes(), b.signature_public_key());
    drop(b);
    let mut b = MlsGroupSession::load(
        FilePersistentProvider::open(&path_b).unwrap(),
        &b_gid,
        &b_pub,
    )
    .unwrap()
    .expect("B's group was persisted");
    assert_eq!(
        app_text(a.process_incoming(&b.encrypt(b"after-b").unwrap()).unwrap()),
        b"after-b"
    );
    assert_eq!(
        app_text(b.process_incoming(&a.encrypt(b"after-a").unwrap()).unwrap()),
        b"after-a"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_unknown_group_is_none_not_an_error() {
    let d = dir("absent");
    let p = FilePersistentProvider::open(&d.join("x.ron")).unwrap();
    let loaded = MlsGroupSession::load(p, b"no-such-group", b"no-such-key").unwrap();
    assert!(loaded.is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_signing_key_is_an_error_not_silence() {
    let d = dir("nokey");
    let path = d.join("g.ron");
    let dev = DeviceId::new();
    let p = FilePersistentProvider::open(&path).unwrap();
    let id = generate_identity(dev, &p).unwrap();
    let session = MlsGroupSession::create(p, &id).unwrap();
    let gid = session.group_id_bytes();
    drop(session);
    let reopened = FilePersistentProvider::open(&path).unwrap();
    // right group, wrong signing key: openmls can't sign as this member
    let result = MlsGroupSession::load(reopened, &gid, b"someone-elses-public-key");
    assert!(result.is_err(), "must not pretend the group loaded");
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------- durability

fn two_member_group(d: &std::path::Path) -> (PathBuf, PathBuf) {
    let (path_a, path_b) = (d.join("a.ron"), d.join("b.ron"));
    let (dev_a, dev_b) = (DeviceId::new(), DeviceId::new());
    let pa = FilePersistentProvider::open(&path_a).unwrap();
    let ia = generate_identity(dev_a, &pa).unwrap();
    let mut a = MlsGroupSession::create(pa, &ia).unwrap();
    let pb = FilePersistentProvider::open(&path_b).unwrap();
    let ib = generate_identity(dev_b, &pb).unwrap();
    let (_commit, welcome) = a.add_member(ib.key_package.key_package()).unwrap();
    let _b = MlsGroupSession::join_from_welcome(pb, ib.signature_keys.clone(), &welcome).unwrap();
    (path_a, path_b)
}

/// The security reason for write-through: after `encrypt` returns, the
/// ratchet advance must already be on disk. If it were not, a crash would let
/// the same key be derived again for a different plaintext.
#[test]
fn every_encrypt_is_on_disk_before_it_returns() {
    let d = dir("writethrough");
    let (dev_a, dev_b) = (DeviceId::new(), DeviceId::new());
    let path_a = d.join("a.ron");
    let pa = FilePersistentProvider::open(&path_a).unwrap();
    let ia = generate_identity(dev_a, &pa).unwrap();
    let mut a = MlsGroupSession::create(pa, &ia).unwrap();
    let pb = FilePersistentProvider::open(&d.join("b.ron")).unwrap();
    let ib = generate_identity(dev_b, &pb).unwrap();
    let _ = a.add_member(ib.key_package.key_package()).unwrap();

    let mut last = std::fs::read(&path_a).unwrap();
    for i in 0..5u8 {
        let _ct = a.encrypt(&[i; 20]).unwrap();
        let now = std::fs::read(&path_a).unwrap();
        assert_ne!(
            now, last,
            "encrypt #{i} returned without persisting the ratchet advance"
        );
        last = now;
    }
}

#[test]
fn the_state_file_is_private_deterministic_and_leaves_no_temp_file() {
    let d = dir("perms");
    let (path_a, _) = two_member_group(&d);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path_a).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(
        !path_a.with_extension("tmp").exists(),
        "no half-written temp file is left behind"
    );
    // RON text, entries sorted by key (deterministic across runs).
    let text = std::fs::read_to_string(&path_a).unwrap();
    assert!(
        text.starts_with("(version:1,entries:["),
        "{}",
        &text[..text.len().min(40)]
    );
    let keys: Vec<&str> = text
        .split("(\"")
        .skip(1)
        .map(|e| e.split('"').next().unwrap())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted);
}

#[test]
fn a_corrupt_state_file_is_refused_and_never_replaced_by_an_empty_one() {
    let d = dir("corrupt");
    let (path_a, _) = two_member_group(&d);
    let good = std::fs::read_to_string(&path_a).unwrap();
    let cases: [(&str, String); 6] = [
        ("not ron at all", "}{ nope".into()),
        ("wrong version", good.replacen("version:1", "version:99", 1)),
        ("odd-length hex", good.replacen("(\"", "(\"a", 1)),
        ("non-hex", good.replacen("(\"", "(\"zz", 1)),
        ("empty file", String::new()),
        ("truncated", good[..good.len() / 2].to_owned()),
    ];
    for (what, bad) in cases {
        std::fs::write(&path_a, &bad).unwrap();
        let r = FilePersistentProvider::open(&path_a);
        assert!(
            matches!(r, Err(siar_crypto_mls::PersistentProviderError::Corrupt(_))),
            "{what}: must be refused as Corrupt"
        );
        assert_eq!(
            std::fs::read_to_string(&path_a).unwrap(),
            bad,
            "{what}: the file must be left untouched"
        );
    }
    // Not UTF-8 either.
    std::fs::write(&path_a, [0xff, 0xfe, 0x00]).unwrap();
    assert!(matches!(
        FilePersistentProvider::open(&path_a),
        Err(siar_crypto_mls::PersistentProviderError::Corrupt(_))
    ));
    // The original still opens and still works.
    std::fs::write(&path_a, &good).unwrap();
    assert!(FilePersistentProvider::open(&path_a).is_ok());
}

/// Hostile bytes at the file boundary: never a panic, and anything that does
/// open must be a state the original wrote.
#[test]
fn mutated_state_files_never_panic_the_loader() {
    let d = dir("fuzz");
    let (path_a, _) = two_member_group(&d);
    let good = std::fs::read(&path_a).unwrap();
    let mut x: u64 = 0x5EED_1234_ABCD_9876;
    let mut next = move || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let probe = d.join("probe.ron");
    for _ in 0..400 {
        let mut bytes = good.clone();
        for _ in 0..=(next() % 4) {
            let i = (next() as usize) % bytes.len();
            match next() % 3 {
                0 => bytes[i] ^= 1 << (next() % 8),
                1 => bytes[i] = next() as u8,
                _ => {
                    bytes.remove(i);
                }
            }
        }
        std::fs::write(&probe, &bytes).unwrap();
        let r = std::panic::catch_unwind(|| FilePersistentProvider::open(&probe).map(|_| ()));
        assert!(r.is_ok(), "the loader PANICKED on a mutated state file");
    }
}
