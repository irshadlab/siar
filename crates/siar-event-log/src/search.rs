//! `04-offline-event-log-architecture.md` §44 "Search as Projection".
//!
//! "Full-text search is a rebuildable projection: `MessageCreated`,
//! `MessageEdited`, `MessageDeleted` → FTS index. If search corrupts,
//! rebuild it."
//!
//! [`SearchProjection`] is that projection, generic over *what* gets
//! indexed. The domain supplies a [`SearchExtractor`] — a pure mapping
//! from a stored event to [`SearchOp`]s (`Upsert` a document's text,
//! `Remove` it) — and the projection maintains an inverted index. The
//! messaging domain's extractor turns `MessageCreated`/`MessageEdited`
//! into upserts and `MessageDeleted` into a remove; this crate has no
//! message types and needs none.
//!
//! ## Why this is a projection and nothing more
//!
//! The index is **derived, disposable state**. It implements
//! [`Projection`], so [`crate::projection::ProjectionRunner`] rebuilds
//! it from the log like any other: `reset` empties it, replay refills
//! it, and [`SearchProjection::fingerprint`] lets a test (or a
//! corruption check) prove that a rebuilt index is *identical* to the
//! live one. There is no second source of truth and no "repair" code —
//! a corrupted index is deleted and rebuilt, which is the entire
//! recovery story §44 asks for.
//!
//! Two consequences worth stating:
//!
//! - `Upsert` **replaces** a document and `Remove` of an absent
//!   document is a no-op, so applying an event twice leaves the index
//!   unchanged (§83's "duplicate event does not duplicate projection
//!   state"), with no dedup bookkeeping.
//! - A redacted event ([`crate::redaction`]) has an empty payload; an
//!   extractor given one yields no ops, so a *rebuild after deletion*
//!   drops the deleted content from the index — search cannot resurrect
//!   what [`crate::deletion`] removed. (The live index of the *deleting*
//!   device must be told too: reset and rebuild, as
//!   [`crate::deletion::DeletionReport::affected_streams`] says.)
//!
//! ## Bounded by construction (§59, §85)
//!
//! Indexed text is hostile-capable input (it arrives by replication).
//! [`SearchLimits`] caps document bytes, distinct terms per document,
//! term length, and query terms; an over-limit document is *truncated*,
//! never rejected, so a giant message degrades to "searchable by its
//! first part" instead of stalling the projection's checkpoint.
//!
//! The tokenizer is intentionally plain (ASCII/Unicode alphanumeric
//! runs, lower-cased): real linguistic search (stemming, CJK
//! segmentation, ranking) is a replaceable concern behind the same
//! extractor seam and does not change anything above.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::projection::{Projection, ProjectionError, ProjectionId};
use crate::store::StoredEvent;

/// A change to the index, produced from one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchOp {
    /// Make `doc_id` searchable by `text`, replacing any previous text.
    Upsert { doc_id: String, text: String },
    /// Remove `doc_id`. Absent document: no-op.
    Remove { doc_id: String },
}

/// The domain's event → index-change mapping. Must be pure and
/// deterministic: a rebuild must reproduce the live index exactly.
pub trait SearchExtractor: Send + Sync {
    fn extract(&self, event: &StoredEvent) -> Vec<SearchOp>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_doc_bytes: usize,
    pub max_terms_per_doc: usize,
    pub max_term_len: usize,
    pub max_query_terms: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_doc_bytes: 64 * 1024,
            max_terms_per_doc: 4096,
            max_term_len: 64,
            max_query_terms: 16,
        }
    }
}

#[derive(Debug, Default)]
struct Index {
    /// doc -> its terms (needed to remove a doc's postings).
    docs: BTreeMap<String, BTreeSet<String>>,
    /// term -> docs containing it.
    postings: BTreeMap<String, BTreeSet<String>>,
}

impl Index {
    fn remove(&mut self, doc: &str) {
        if let Some(terms) = self.docs.remove(doc) {
            for t in terms {
                if let Some(set) = self.postings.get_mut(&t) {
                    set.remove(doc);
                    if set.is_empty() {
                        self.postings.remove(&t);
                    }
                }
            }
        }
    }

    fn upsert(&mut self, doc: &str, terms: BTreeSet<String>) {
        self.remove(doc);
        for t in &terms {
            self.postings
                .entry(t.clone())
                .or_default()
                .insert(doc.to_string());
        }
        self.docs.insert(doc.to_string(), terms);
    }
}

/// Lower-cased alphanumeric runs, bounded per [`SearchLimits`].
pub fn tokenize(text: &str, limits: &SearchLimits) -> BTreeSet<String> {
    // Truncate on a char boundary: a giant document degrades, it does
    // not stall the projection.
    let mut end = text.len().min(limits.max_doc_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = BTreeSet::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut BTreeSet<String>| {
        if !cur.is_empty() {
            if out.len() < limits.max_terms_per_doc {
                out.insert(std::mem::take(cur));
            } else {
                cur.clear();
            }
        }
    };
    for ch in text[..end].chars() {
        if ch.is_alphanumeric() {
            if cur.chars().count() < limits.max_term_len {
                cur.extend(ch.to_lowercase());
            }
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub struct SearchProjection<X: SearchExtractor> {
    id: ProjectionId,
    version: u16,
    extractor: X,
    limits: SearchLimits,
    index: Mutex<Index>,
}

impl<X: SearchExtractor> SearchProjection<X> {
    pub fn new(id: &'static str, version: u16, extractor: X) -> Self {
        Self::with_limits(id, version, extractor, SearchLimits::default())
    }

    pub fn with_limits(id: &'static str, version: u16, extractor: X, limits: SearchLimits) -> Self {
        Self {
            id: ProjectionId(id),
            version,
            extractor,
            limits,
            index: Mutex::new(Index::default()),
        }
    }

    /// Documents containing **all** of the query's terms, sorted by id.
    /// An empty query (no terms after tokenizing) matches nothing, and
    /// terms beyond [`SearchLimits::max_query_terms`] are ignored.
    pub fn search(&self, query: &str) -> Vec<String> {
        let terms: Vec<String> = tokenize(query, &self.limits)
            .into_iter()
            .take(self.limits.max_query_terms)
            .collect();
        if terms.is_empty() {
            return Vec::new();
        }
        let index = self.index.lock().expect("search index poisoned");
        let mut sets: Vec<&BTreeSet<String>> = Vec::with_capacity(terms.len());
        for t in &terms {
            match index.postings.get(t) {
                Some(s) => sets.push(s),
                None => return Vec::new(),
            }
        }
        // Intersect starting from the smallest posting list.
        sets.sort_by_key(|s| s.len());
        let (first, rest) = sets.split_first().expect("terms is non-empty");
        first
            .iter()
            .filter(|d| rest.iter().all(|s| s.contains(*d)))
            .cloned()
            .collect()
    }

    pub fn document_count(&self) -> usize {
        self.index.lock().expect("search index poisoned").docs.len()
    }

    /// A digest of the entire index state: equal fingerprints mean
    /// identical indexes. This is how "rebuild equals live" and
    /// corruption checks are made precise.
    pub fn fingerprint(&self) -> [u8; 32] {
        let index = self.index.lock().expect("search index poisoned");
        let mut h = blake3::Hasher::new();
        h.update(b"siar/search-index/v1");
        for (doc, terms) in &index.docs {
            h.update(&(doc.len() as u64).to_le_bytes());
            h.update(doc.as_bytes());
            h.update(&(terms.len() as u64).to_le_bytes());
            for t in terms {
                h.update(&(t.len() as u64).to_le_bytes());
                h.update(t.as_bytes());
            }
        }
        *h.finalize().as_bytes()
    }
}

#[async_trait]
impl<X: SearchExtractor> Projection for SearchProjection<X> {
    fn projection_id(&self) -> ProjectionId {
        self.id
    }

    fn projection_version(&self) -> u16 {
        self.version
    }

    async fn apply(&self, event: &StoredEvent) -> Result<(), ProjectionError> {
        let ops = self.extractor.extract(event);
        if ops.is_empty() {
            return Ok(());
        }
        let mut index = self.index.lock().expect("search index poisoned");
        for op in ops {
            match op {
                SearchOp::Upsert { doc_id, text } => {
                    index.upsert(&doc_id, tokenize(&text, &self.limits));
                }
                SearchOp::Remove { doc_id } => index.remove(&doc_id),
            }
        }
        Ok(())
    }

    async fn reset(&self) -> Result<(), ProjectionError> {
        *self.index.lock().expect("search index poisoned") = Index::default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EventOrigin;
    use crate::ids::{EventId, EventTypeId, StreamId, Timestamp};
    use crate::memory_store::InMemoryEventStore;
    use crate::projection::{InMemoryCheckpointStore, ProjectionRunner};
    use crate::redaction::RedactableStore;
    use crate::store::{AppendRequest, EventStore, NewEvent};
    use proptest::prelude::*;

    const CREATED: EventTypeId = EventTypeId(1);
    const EDITED: EventTypeId = EventTypeId(2);
    const DELETED: EventTypeId = EventTypeId(3);

    /// payload = "<doc>\n<text>" for create/edit; "<doc>" for delete.
    struct Msg;
    impl SearchExtractor for Msg {
        fn extract(&self, e: &StoredEvent) -> Vec<SearchOp> {
            let body = String::from_utf8_lossy(&e.envelope.payload).to_string();
            if body.is_empty() {
                return vec![];
            }
            match e.envelope.event_type {
                CREATED | EDITED => {
                    let (doc, text) = body.split_once('\n').unwrap_or((&body, ""));
                    vec![SearchOp::Upsert {
                        doc_id: doc.into(),
                        text: text.into(),
                    }]
                }
                DELETED => vec![SearchOp::Remove { doc_id: body }],
                _ => vec![],
            }
        }
    }

    fn ev(ty: EventTypeId, payload: &str) -> NewEvent {
        NewEvent {
            event_id: EventId::new(),
            event_type: ty,
            schema_version: 1,
            created_at: Timestamp(0),
            origin: EventOrigin::System,
            correlation_id: None,
            causation_id: None,
            payload: payload.as_bytes().to_vec(),
        }
    }

    async fn log(events: Vec<NewEvent>) -> InMemoryEventStore {
        let store = InMemoryEventStore::new();
        store
            .append(AppendRequest {
                stream_id: StreamId::from_name("s"),
                expected_version: 0,
                events,
            })
            .await
            .unwrap();
        store
    }

    async fn rebuilt(store: &InMemoryEventStore) -> SearchProjection<Msg> {
        let p = SearchProjection::new("search", 1, Msg);
        let cps = InMemoryCheckpointStore::new();
        ProjectionRunner::new(store, &cps)
            .catch_up(&p, 100)
            .await
            .unwrap();
        p
    }

    #[test]
    fn tokenizing_lowercases_splits_and_bounds() {
        let l = SearchLimits::default();
        let t = tokenize("Hello, WORLD! hello-again 42", &l);
        assert_eq!(
            t.into_iter().collect::<Vec<_>>(),
            vec!["42", "again", "hello", "world"]
        );
        // Bounds.
        let tiny = SearchLimits {
            max_term_len: 4,
            max_terms_per_doc: 2,
            max_doc_bytes: 20,
            max_query_terms: 4,
        };
        assert!(tokenize("abcdefghij", &tiny).iter().all(|t| t.len() <= 4));
        assert_eq!(tokenize("a b c d e f", &tiny).len(), 2);
        assert!(tokenize(&"word ".repeat(1000), &tiny).len() <= 2);
        // Truncation never splits a multi-byte char.
        let s = "é".repeat(50);
        let _ = tokenize(
            &s,
            &SearchLimits {
                max_doc_bytes: 5,
                ..SearchLimits::default()
            },
        );
    }

    #[tokio::test]
    async fn create_edit_delete_drive_the_index() {
        let store = log(vec![
            ev(CREATED, "m1\nmeet at the harbour tomorrow"),
            ev(CREATED, "m2\nharbour closes at six"),
            ev(CREATED, "m3\nunrelated note"),
        ])
        .await;
        let p = rebuilt(&store).await;
        assert_eq!(p.search("harbour"), vec!["m1", "m2"]);
        assert_eq!(p.search("harbour tomorrow"), vec!["m1"], "AND semantics");
        assert_eq!(p.search("HARBOUR"), vec!["m1", "m2"], "case-insensitive");
        assert!(p.search("zzz").is_empty());
        assert!(p.search("   ").is_empty(), "empty query matches nothing");

        // Edit replaces; delete removes.
        let p = SearchProjection::new("search", 1, Msg);
        for e in [
            ev(CREATED, "m1\nold words here"),
            ev(EDITED, "m1\nbrand new text"),
            ev(DELETED, "m3"),
            ev(DELETED, "never-existed"),
        ] {
            let store = log(vec![e]).await;
            let stored = store
                .read_log(crate::ids::LocalLogOffset(0), 1)
                .await
                .unwrap();
            p.apply(&stored[0]).await.unwrap();
        }
        assert!(p.search("old").is_empty(), "edit removed the old terms");
        assert_eq!(p.search("brand new"), vec!["m1"]);
        assert_eq!(p.document_count(), 1);
    }

    #[tokio::test]
    async fn a_corrupted_index_is_fixed_by_rebuilding_and_the_result_is_identical() {
        let store = log(vec![
            ev(CREATED, "a\nalpha beta"),
            ev(CREATED, "b\nbeta gamma"),
            ev(EDITED, "a\nalpha delta"),
            ev(DELETED, "b"),
        ])
        .await;
        let live = rebuilt(&store).await;
        let before = live.fingerprint();

        // Corrupt it (simulating a damaged/stale index).
        {
            let mut idx = live.index.lock().unwrap();
            idx.postings.clear();
            idx.docs
                .insert("ghost".into(), BTreeSet::from(["boo".to_string()]));
        }
        assert_ne!(live.fingerprint(), before);
        assert!(live.search("alpha").is_empty());

        // §44: "rebuild it."
        live.reset().await.unwrap();
        let cps = InMemoryCheckpointStore::new();
        ProjectionRunner::new(&store, &cps)
            .catch_up(&live, 100)
            .await
            .unwrap();
        assert_eq!(
            live.fingerprint(),
            before,
            "rebuild equals the original live index"
        );
        assert_eq!(live.search("alpha"), vec!["a"]);
        assert!(live.search("gamma").is_empty());
        assert_eq!(
            rebuilt(&store).await.fingerprint(),
            before,
            "and a second independent rebuild"
        );
    }

    #[tokio::test]
    async fn applying_the_same_event_twice_changes_nothing() {
        let store = log(vec![ev(CREATED, "m\nhello world")]).await;
        let stored = store
            .read_log(crate::ids::LocalLogOffset(0), 1)
            .await
            .unwrap();
        let p = SearchProjection::new("search", 1, Msg);
        p.apply(&stored[0]).await.unwrap();
        let once = p.fingerprint();
        p.apply(&stored[0]).await.unwrap();
        assert_eq!(p.fingerprint(), once);
        assert_eq!(p.search("hello"), vec!["m"]);
    }

    #[tokio::test]
    async fn a_rebuild_after_redaction_cannot_resurrect_deleted_content() {
        let created = ev(CREATED, "secret\nthe launch code is swordfish");
        let id = created.event_id;
        let store = log(vec![created, ev(CREATED, "other\nunrelated words")]).await;
        assert_eq!(rebuilt(&store).await.search("swordfish"), vec!["secret"]);

        store.redact_payloads(&[id]).await.unwrap();
        let after = rebuilt(&store).await;
        assert!(after.search("swordfish").is_empty());
        assert_eq!(after.search("unrelated"), vec!["other"]);
    }

    #[test]
    fn query_terms_are_bounded() {
        let p = SearchProjection::with_limits(
            "s",
            1,
            Msg,
            SearchLimits {
                max_query_terms: 2,
                ..SearchLimits::default()
            },
        );
        // Must not blow up or scan unboundedly on a huge query.
        assert!(p.search(&"x ".repeat(10_000)).is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]
        /// Live application and a from-scratch rebuild agree for any
        /// sequence of creates, edits and deletes.
        #[test]
        fn live_index_equals_rebuilt_index(
            ops in proptest::collection::vec((0u8..3, 0u8..4, "[a-d ]{0,12}"), 1..30),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            rt.block_on(async {
                let events: Vec<NewEvent> = ops.iter().map(|(k, d, t)| match k {
                    0 => ev(CREATED, &format!("d{d}\n{t}")),
                    1 => ev(EDITED, &format!("d{d}\n{t}")),
                    _ => ev(DELETED, &format!("d{d}")),
                }).collect();
                let store = log(events).await;
                let live = SearchProjection::new("l", 1, Msg);
                for s in store.read_log(crate::ids::LocalLogOffset(0), 1000).await.unwrap() {
                    live.apply(&s).await.unwrap();
                }
                let again = rebuilt(&store).await;
                prop_assert_eq!(live.fingerprint(), again.fingerprint());
                Ok(())
            })?;
        }
    }
}
