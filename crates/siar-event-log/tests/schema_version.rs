//! §91 "schema versioning" for the storage layer itself: a database
//! stamped by a newer build is refused, an older or unmarked one is
//! migrated forward in place, and neither loses data.

use siar_domain::DeviceId;
use siar_event_log::stoolap_store::STORAGE_SCHEMA_VERSION;
use siar_event_log::{
    AppendRequest, EventId, EventOrigin, EventStore, EventTypeId, LocalLogOffset, NewEvent,
    StoolapEventStore, StreamId, Timestamp,
};

fn temp() -> String {
    std::env::temp_dir()
        .join(format!("siar-schema-{}", uuid::Uuid::new_v4()))
        .to_str()
        .unwrap()
        .to_string()
}
fn cleanup(p: &str) {
    let _ = std::fs::remove_dir_all(p);
    let _ = std::fs::remove_file(p);
}
fn ev() -> NewEvent {
    NewEvent {
        event_id: EventId::new(),
        event_type: EventTypeId(1),
        schema_version: 1,
        created_at: Timestamp(0),
        origin: EventOrigin::LocalDevice(DeviceId::new()),
        correlation_id: None,
        causation_id: None,
        payload: b"kept".to_vec(),
    }
}
fn raw(path: &str) -> stoolap::Database {
    stoolap::Database::open(&format!("file://{path}")).unwrap()
}
fn stamped(path: &str) -> Option<i64> {
    let db = raw(path);
    let mut rows = db
        .query(
            "SELECT value FROM schema_meta WHERE name = 'storage_schema'",
            (),
        )
        .unwrap();
    rows.next().map(|r| r.unwrap().get::<i64>(0).unwrap())
}

#[tokio::test]
async fn a_new_database_is_stamped_with_the_current_version() {
    let path = temp();
    {
        let _ = StoolapEventStore::open(&path).unwrap();
    }
    assert_eq!(stamped(&path), Some(STORAGE_SCHEMA_VERSION));
    // Reopening is a no-op.
    {
        let _ = StoolapEventStore::open(&path).unwrap();
    }
    assert_eq!(stamped(&path), Some(STORAGE_SCHEMA_VERSION));
    cleanup(&path);
}

#[tokio::test]
async fn a_database_from_a_newer_build_is_refused_untouched() {
    let path = temp();
    let stream = StreamId::from_name("s");
    {
        let store = StoolapEventStore::open(&path).unwrap();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![ev()],
            })
            .await
            .unwrap();
    }
    raw(&path)
        .execute(
            "UPDATE schema_meta SET value = $1 WHERE name = 'storage_schema'",
            (STORAGE_SCHEMA_VERSION + 1,),
        )
        .unwrap();

    let err = StoolapEventStore::open(&path).err().expect("must refuse");
    let msg = err.to_string();
    assert!(msg.contains("newer"), "{msg}");
    // Refusal changed nothing: the stamp is still the newer value.
    assert_eq!(stamped(&path), Some(STORAGE_SCHEMA_VERSION + 1));
    cleanup(&path);
}

#[tokio::test]
async fn an_unmarked_legacy_database_is_migrated_in_place_and_keeps_its_data() {
    let path = temp();
    let stream = StreamId::from_name("s");
    {
        let store = StoolapEventStore::open(&path).unwrap();
        store
            .append(AppendRequest {
                stream_id: stream,
                expected_version: 0,
                events: vec![ev(), ev()],
            })
            .await
            .unwrap();
    }
    // Make it look like a pre-versioning database: no marker, and none
    // of the later tables.
    {
        let db = raw(&path);
        db.execute("DROP TABLE schema_meta", ()).unwrap();
        db.execute("DROP TABLE outbox", ()).unwrap();
        db.execute("DROP TABLE redactions", ()).unwrap();
    }
    {
        let store = StoolapEventStore::open(&path).unwrap();
        // Data intact, and the new tables work.
        assert_eq!(
            store.read_log(LocalLogOffset(0), 10).await.unwrap().len(),
            2
        );
        assert_eq!(store.stream_head(stream).await.unwrap(), 2);
        assert_eq!(store.latest_offset().await.unwrap(), LocalLogOffset(2));
        use siar_event_log::outbox::Outbox;
        assert_eq!(store.outbox().unwrap().counts().await.unwrap().pending, 0);
    }
    assert_eq!(stamped(&path), Some(STORAGE_SCHEMA_VERSION));
    cleanup(&path);
}

#[tokio::test]
async fn an_older_stamp_is_raised_to_current() {
    let path = temp();
    {
        let _ = StoolapEventStore::open(&path).unwrap();
    }
    raw(&path)
        .execute(
            "UPDATE schema_meta SET value = 1 WHERE name = 'storage_schema'",
            (),
        )
        .unwrap();
    {
        let _ = StoolapEventStore::open(&path).unwrap();
    }
    assert_eq!(stamped(&path), Some(STORAGE_SCHEMA_VERSION));
    cleanup(&path);
}
