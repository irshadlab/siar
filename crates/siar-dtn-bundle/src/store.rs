//! §15 "Durable Store", §16 "Bundle Store Interface", §14 "Bundle
//! Envelope vs Local Record" (the local-record half — see
//! [`StoredBundle`]'s own doc comment).

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

use crate::bundle::DtnBundle;
use crate::state::BundleState;
use crate::types::{BundleId, RouteToken};

/// §14's own naming for the wire-facing half of the split — a type
/// alias, not a new struct, since [`DtnBundle`]'s own fields already
/// are exactly the wire form (see that type's own doc comment). Having
/// this alias means code that wants to talk in the spec's own §14
/// vocabulary can, without this crate maintaining two structs with
/// identical fields.
pub type WireBundle = DtnBundle;

/// §14's own local-record half, plus §18's existing `state` field
/// (already local-only — a peer never needs to know this device's own
/// state-machine position for someone else's bundle). Field-for-field
/// against §14's own four named items:
/// - `storage path` → [`StoredBundle::storage_path`]. `Option<String>`,
///   not a real filesystem path type — `None` for a bundle that only
///   ever lived in this in-memory store (see
///   [`InMemoryBundleStore`]'s own doc comment); a real disk-backed
///   store would populate it.
/// - `local retry count` → [`StoredBundle::retry_count`], bumped by
///   [`StoredBundle::record_retry`].
/// - `peer history` → [`StoredBundle::peer_history`], the ordered list
///   of [`RouteToken`]s this bundle has been handed to so far —
///   `RouteToken`, not a `DeviceId`, for the same non-correlation
///   reason [`crate::types::DtnSource`]/[`crate::types::DtnDestination`]
///   already use it. Appended to (not deduplicated) by
///   [`StoredBundle::record_peer`] — the same peer may legitimately
///   receive a bundle more than once across separate encounters, and
///   a full history, not just a set, is what §143 "Forward History"
///   (not yet built) will eventually want to read.
/// - `custody state` → the pre-existing `state: `[`BundleState`] field,
///   already local-only; §14 doesn't ask for anything new here, just
///   confirmation it belongs on this side of the split, not the wire
///   side.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredBundle {
    pub bundle: DtnBundle,
    pub state: BundleState,
    pub storage_path: Option<String>,
    pub retry_count: u32,
    pub peer_history: Vec<RouteToken>,
}

impl StoredBundle {
    fn new(bundle: DtnBundle) -> Self {
        Self {
            bundle,
            state: BundleState::Stored,
            storage_path: None,
            retry_count: 0,
            peer_history: Vec::new(),
        }
    }

    pub fn record_retry(&mut self) {
        self.retry_count += 1;
    }

    pub fn record_peer(&mut self, peer: RouteToken) {
        self.peer_history.push(peer);
    }
}

/// §16's own query type — not detailed further in the source text
/// beyond `ForwardQuery`'s name and use in `list_candidates`; this
/// crate's own reasonable reading: candidates for forwarding are ones
/// in [`BundleState::Eligible`] (or already `Forwarded`, for
/// SprayAndWait's multiple-hop case) that haven't expired as of `now`.
#[derive(Debug, Clone, Copy)]
pub struct ForwardQuery {
    pub now_millis: u64,
    pub limit: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DtnStoreError {
    #[error("bundle {0:?} not found")]
    NotFound(BundleId),
}

/// §16, verbatim method set plus one real addition: `mark_eligible`.
/// The spec's own snippet elides `mark_forwarded`/`mark_delivered`'s
/// parameters with `...` and doesn't show a method for the
/// `Stored → Eligible` transition at all — but §18 lists `Eligible` as
/// a real state between `Stored` and `Forwarded`, and without some way
/// to reach it, [`BundleStore::list_candidates`] could never return
/// anything (confirmed by this crate's own test suite hitting exactly
/// that gap while exercising the trait for real). Added rather than
/// silently worked around.
#[async_trait]
pub trait BundleStore: Send + Sync {
    async fn put(&self, bundle: DtnBundle) -> Result<(), DtnStoreError>;
    async fn get(&self, id: BundleId) -> Result<Option<StoredBundle>, DtnStoreError>;
    async fn mark_eligible(&self, id: BundleId) -> Result<(), DtnStoreError>;
    async fn mark_forwarded(&self, id: BundleId) -> Result<(), DtnStoreError>;
    async fn mark_delivered(&self, id: BundleId) -> Result<(), DtnStoreError>;
    async fn remove(&self, id: BundleId) -> Result<(), DtnStoreError>;
    async fn list_candidates(
        &self,
        query: ForwardQuery,
    ) -> Result<Vec<StoredBundle>, DtnStoreError>;
}

/// §15: "Every accepted DTN bundle must be persisted before claiming it
/// is being carried" — real, if in-memory: [`InMemoryBundleStore::put`]
/// stores the bundle in `Stored` state (not yet `Eligible`) before
/// returning, matching §15's own two-step "write durable record →
/// eligible for forwarding" sequence rather than collapsing them.
///
/// Genuinely in-memory only — the actual durable half of §15's own
/// requirement (surviving process death) needs real disk/SQLite
/// storage this crate doesn't implement; see this crate's own top doc
/// comment.
#[derive(Default)]
pub struct InMemoryBundleStore {
    bundles: Mutex<HashMap<BundleId, StoredBundle>>,
}

impl InMemoryBundleStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl BundleStore for InMemoryBundleStore {
    async fn put(&self, bundle: DtnBundle) -> Result<(), DtnStoreError> {
        let mut bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        bundles.insert(bundle.bundle_id, StoredBundle::new(bundle));
        Ok(())
    }

    async fn get(&self, id: BundleId) -> Result<Option<StoredBundle>, DtnStoreError> {
        let bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        Ok(bundles.get(&id).cloned())
    }

    async fn mark_eligible(&self, id: BundleId) -> Result<(), DtnStoreError> {
        let mut bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        let stored = bundles.get_mut(&id).ok_or(DtnStoreError::NotFound(id))?;
        stored.state = stored
            .state
            .transition(crate::state::BundleEvent::BecomeEligible)
            .unwrap_or(stored.state);
        Ok(())
    }

    async fn mark_forwarded(&self, id: BundleId) -> Result<(), DtnStoreError> {
        let mut bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        let stored = bundles.get_mut(&id).ok_or(DtnStoreError::NotFound(id))?;
        stored.state = stored
            .state
            .transition(crate::state::BundleEvent::Forward)
            .unwrap_or(stored.state);
        Ok(())
    }

    async fn mark_delivered(&self, id: BundleId) -> Result<(), DtnStoreError> {
        let mut bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        let stored = bundles.get_mut(&id).ok_or(DtnStoreError::NotFound(id))?;
        stored.state = stored
            .state
            .transition(crate::state::BundleEvent::ReachDestination)
            .unwrap_or(stored.state);
        Ok(())
    }

    async fn remove(&self, id: BundleId) -> Result<(), DtnStoreError> {
        let mut bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        bundles.remove(&id);
        Ok(())
    }

    async fn list_candidates(
        &self,
        query: ForwardQuery,
    ) -> Result<Vec<StoredBundle>, DtnStoreError> {
        let bundles = self
            .bundles
            .lock()
            .expect("InMemoryBundleStore lock poisoned");
        let mut candidates: Vec<StoredBundle> = bundles
            .values()
            .filter(|stored| {
                matches!(stored.state, BundleState::Eligible | BundleState::Forwarded)
                    && !stored.bundle.is_expired(query.now_millis)
            })
            .cloned()
            .collect();
        candidates.truncate(query.limit);
        Ok(candidates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleIntegrity;
    use crate::payload::PayloadReference;
    use crate::types::{DtnDestination, DtnPriority, DtnSource, ForwardingClass, PayloadTypeId};

    fn bundle(expires_at_millis: u64) -> DtnBundle {
        DtnBundle {
            bundle_id: BundleId::new(),
            source: DtnSource(RouteToken(vec![1])),
            destination: DtnDestination::DeviceOpaque(RouteToken(vec![2])),
            payload_type: PayloadTypeId(1),
            created_at_millis: 0,
            expires_at_millis,
            priority: DtnPriority::Normal,
            hop_limit: 4,
            replication_budget: 2,
            forwarding_class: ForwardingClass::SprayAndWait,
            payload_ref: PayloadReference::Inline(vec![9]),
            integrity: BundleIntegrity {
                payload_hash: [0u8; 32],
                origin_signature: None,
            },
        }
    }

    #[tokio::test]
    async fn a_put_bundle_is_retrievable_and_starts_in_stored_state() {
        let store = InMemoryBundleStore::new();
        let b = bundle(1_000);
        let id = b.bundle_id;
        store.put(b).await.unwrap();
        let stored = store.get(id).await.unwrap().unwrap();
        assert_eq!(stored.state, BundleState::Stored);
    }

    #[tokio::test]
    async fn an_eligible_unexpired_bundle_is_a_forward_candidate() {
        let store = InMemoryBundleStore::new();
        let b = bundle(1_000);
        let id = b.bundle_id;
        store.put(b).await.unwrap();
        store.mark_eligible(id).await.unwrap();

        let candidates = store
            .list_candidates(ForwardQuery {
                now_millis: 0,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].state, BundleState::Eligible);
    }

    #[tokio::test]
    async fn a_stored_but_not_yet_eligible_bundle_is_not_a_forward_candidate() {
        let store = InMemoryBundleStore::new();
        store.put(bundle(1_000)).await.unwrap();
        let candidates = store
            .list_candidates(ForwardQuery {
                now_millis: 0,
                limit: 10,
            })
            .await
            .unwrap();
        assert!(candidates.is_empty()); // Stored, not Eligible
    }

    #[tokio::test]
    async fn an_eligible_but_expired_bundle_is_never_a_forward_candidate() {
        let store = InMemoryBundleStore::new();
        let b = bundle(500);
        let id = b.bundle_id;
        store.put(b).await.unwrap();
        store.mark_eligible(id).await.unwrap();

        let candidates = store
            .list_candidates(ForwardQuery {
                now_millis: 999,
                limit: 10,
            })
            .await
            .unwrap();
        assert!(candidates.is_empty()); // §20: expired bundles are never forwarded
    }

    #[tokio::test]
    async fn removing_a_bundle_makes_it_unretrievable() {
        let store = InMemoryBundleStore::new();
        let b = bundle(1_000);
        let id = b.bundle_id;
        store.put(b).await.unwrap();
        store.remove(id).await.unwrap();
        assert!(store.get(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn marking_an_unknown_bundle_forwarded_is_a_real_error() {
        let store = InMemoryBundleStore::new();
        let result = store.mark_forwarded(BundleId::new()).await;
        assert!(matches!(result, Err(DtnStoreError::NotFound(_))));
    }

    #[test]
    fn a_fresh_stored_bundle_has_no_local_record_history_yet() {
        let record = StoredBundle::new(bundle(1_000));
        assert_eq!(record.storage_path, None);
        assert_eq!(record.retry_count, 0);
        assert!(record.peer_history.is_empty());
    }

    #[test]
    fn record_retry_and_record_peer_accumulate_independently() {
        let mut record = StoredBundle::new(bundle(1_000));
        record.record_retry();
        record.record_retry();
        record.record_peer(RouteToken(vec![7]));
        record.record_peer(RouteToken(vec![7])); // same peer twice: a real, separate encounter
        assert_eq!(record.retry_count, 2);
        assert_eq!(
            record.peer_history,
            vec![RouteToken(vec![7]), RouteToken(vec![7])]
        );
    }
}
