//! A restarted device must be able to keep using an MLS group it was
//! already in. These tests close and reopen the SQLite file between
//! messages — the thing a process restart does.

use std::path::PathBuf;

use siar_crypto_mls::{
    generate_identity, IncomingMlsMessage, MlsGroupSession, SqlitePersistentProvider,
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
    let (path_a, path_b) = (d.join("a.sqlite3"), d.join("b.sqlite3"));
    let (dev_a, dev_b) = (DeviceId::new(), DeviceId::new());

    // A founds the group and adds B; both are persistent.
    let pa = SqlitePersistentProvider::open(&path_a).unwrap();
    let ia = generate_identity(dev_a, &pa).unwrap();
    let mut a = MlsGroupSession::create(pa, &ia).unwrap();
    let pb = SqlitePersistentProvider::open(&path_b).unwrap();
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

    let mut a = MlsGroupSession::load(
        SqlitePersistentProvider::open(&path_a).unwrap(),
        &gid,
        &a_pub,
    )
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
        SqlitePersistentProvider::open(&path_b).unwrap(),
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
    let p = SqlitePersistentProvider::open(&d.join("x.sqlite3")).unwrap();
    let loaded = MlsGroupSession::load(p, b"no-such-group", b"no-such-key").unwrap();
    assert!(loaded.is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_missing_signing_key_is_an_error_not_silence() {
    let d = dir("nokey");
    let path = d.join("g.sqlite3");
    let dev = DeviceId::new();
    let p = SqlitePersistentProvider::open(&path).unwrap();
    let id = generate_identity(dev, &p).unwrap();
    let session = MlsGroupSession::create(p, &id).unwrap();
    let gid = session.group_id_bytes();
    drop(session);
    let reopened = SqlitePersistentProvider::open(&path).unwrap();
    // right group, wrong signing key: openmls can't sign as this member
    let result = MlsGroupSession::load(reopened, &gid, b"someone-elses-public-key");
    assert!(result.is_err(), "must not pretend the group loaded");
    let _ = std::fs::remove_dir_all(&d);
}
