//! Regression: stoolap's top-N operator panics ("capacity overflow") on a
//! `LIMIT` near `i64::MAX`, which the durable stores used to pass straight
//! through for "give me everything" requests. Both durable stores now page
//! internally; these tests cross the page boundaries with real data on the
//! real engine, so the in-memory stores' quiet success cannot hide it again.

use siar_blob_manifest::ids::BlobId;
use siar_blob_manifest::metadata_store::{BlobMetadataStore, StoolapBlobMetadata};
use siar_blob_manifest::reference_store::{
    BlobReference, BlobRole, ObjectId, RefOp, ReferenceStore, StoolapReferenceStore,
};
use siar_blob_manifest::schema_rows::{BlobLifecycleState, BlobRow};
use siar_blob_manifest::storage_lifecycle::BlobClassification;
use siar_event_log::namespace::NamespaceId;

const N: usize = 2_600; // > two 1024-row pages, not a multiple of one

fn id(i: usize) -> BlobId {
    BlobId::from_ciphertext(&(i as u64).to_be_bytes())
}

fn sorted_ids() -> Vec<BlobId> {
    let mut v: Vec<BlobId> = (0..N).map(id).collect();
    v.sort_by_key(|b| *b.as_bytes());
    v
}

fn row(i: usize, class: BlobClassification) -> BlobRow {
    BlobRow {
        blob_id: id(i),
        size: 1,
        state: BlobLifecycleState::Verified,
        created_at_millis: 0,
        last_accessed_millis: 0,
        storage_class: class,
        reference_count: 0,
    }
}

fn metadata() -> StoolapBlobMetadata {
    let m = StoolapBlobMetadata::open_in_memory().unwrap();
    let class = BlobClassification::Durable;
    for i in 0..N {
        m.put(row(i, class)).unwrap();
    }
    m
}

#[test]
fn listing_everything_from_the_durable_metadata_store_does_not_panic_and_is_complete_and_ordered() {
    let m = metadata();
    let all = m.list(usize::MAX).unwrap();
    assert_eq!(all.len(), N);
    let want = sorted_ids();
    assert!(all.iter().map(|r| r.blob_id).eq(want.iter().copied()));
    assert_eq!(
        m.list_after(None, usize::MAX).unwrap().len(),
        N,
        "list_after with a huge limit too"
    );
}

#[test]
fn every_limit_around_a_page_boundary_returns_exactly_that_many() {
    let m = metadata();
    let want = sorted_ids();
    for limit in [
        0usize,
        1,
        1023,
        1024,
        1025,
        2047,
        2048,
        2049,
        N - 1,
        N,
        N + 1,
    ] {
        let got = m.list(limit).unwrap();
        assert_eq!(got.len(), limit.min(N), "limit {limit}");
        assert!(got
            .iter()
            .map(|r| r.blob_id)
            .eq(want.iter().copied().take(limit)));
    }
}

#[test]
fn cursor_paging_in_odd_sizes_visits_every_row_exactly_once() {
    let m = metadata();
    for page in [1usize, 7, 1000, 1024, 1500, 5000] {
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let rows = m.list_after(cursor, page).unwrap();
            let Some(last) = rows.last().map(|r| r.blob_id) else {
                break;
            };
            let short = rows.len() < page;
            seen.extend(rows.into_iter().map(|r| r.blob_id));
            if short {
                break;
            }
            cursor = Some(last);
        }
        assert_eq!(seen, sorted_ids(), "page size {page}");
    }
}

#[test]
fn the_durable_reference_store_pages_referenced_blob_ids_the_same_way() {
    let refs = StoolapReferenceStore::open_in_memory().unwrap();
    let ns = NamespaceId::new("personal").unwrap();
    for i in 0..N {
        refs.apply(&[RefOp::Add(BlobReference {
            owner_namespace: ns.clone(),
            object_id: ObjectId::new(&format!("o{i}")).unwrap(),
            blob_id: id(i),
            role: BlobRole::MessageAttachment,
        })])
        .unwrap();
    }
    let all = refs.referenced_blob_ids(None, usize::MAX).unwrap();
    assert_eq!(all, sorted_ids());
    let mid = refs
        .referenced_blob_ids(Some(all[1_500]), usize::MAX)
        .unwrap();
    assert_eq!(mid.len(), N - 1_501);
    assert_eq!(refs.referenced_blob_ids(None, 1025).unwrap().len(), 1025);
    // A blob with many references still lists under a huge limit.
    assert_eq!(refs.references_for(id(0), usize::MAX).unwrap().len(), 1);
}
