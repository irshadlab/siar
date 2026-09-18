//! A stored, quota-bounded copy of a received `MeshEnvelope`, plus the
//! dedup set that guards it — this binary's own replacement for the
//! retired `siar-dtn` crate (`siar_dtn::bundle::MeshBundle`/
//! `store::BundleStore`/`dedup::SeenBundles`; see `MIGRATION.md`'s
//! device-certificate reconciliation section for how that crate was
//! retired).
//!
//! **Why this lives here, in the binary, rather than being ported into
//! a shared crate like the routing-side gaps were**: `siar_dtn_bundle`
//! (the sys-arch-era replacement `siar-dtn` was originally retired in
//! favor of) is a genuinely different, wire-*incompatible* model —
//! `DtnBundle::destination` is an opaque `RouteToken`, and nothing in
//! this workspace's wire protocol (`siar_protocol::WireMessage`) has
//! ever carried one; the *only* real wire representation of mesh/DTN
//! traffic anywhere in this workspace is `MeshEnvelope`, which still
//! carries a plain `DeviceId` (see that struct's own doc comment — this
//! is itself a real, independent, already-flagged gap, unrelated to
//! `siar-dtn` vs `siar-dtn-bundle`). Forcing this relay's storage onto
//! `DtnBundle`'s shape would mean either fabricating a fake `RouteToken`
//! from a `DeviceId` (defeating the whole reason that type is opaque)
//! or inventing a new `WireMessage` variant no spec text describes yet
//! — both bigger, separate undertakings than "retire a superseded
//! model." A relay-local type that mirrors the wire format it actually
//! receives is the honest scope for this pass; wiring real DTN-bundle
//! privacy onto the network is real follow-up work, named here rather
//! than quietly approximated.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::hash::Hash;

use siar_domain::{DeviceId, MessageId, MessagePriority};

/// A locally stored copy of a received [`siar_protocol::MeshEnvelope`],
/// plus the one bookkeeping field the wire format itself doesn't carry
/// — `replication_budget` is a relay's own outgoing-copy policy, not
/// the original sender's (see `MeshEnvelope::replication_budget`'s
/// absence, and `MessagePriority::default_replication_budget`, which
/// is exactly this policy already built in `siar-domain`).
#[derive(Debug, Clone)]
pub struct StoredBundle {
    pub id: MessageId,
    pub destination: DeviceId,
    pub created_at: u64,
    pub expires_at: u64,
    pub hop_limit: u8,
    pub priority: MessagePriority,
    pub payload_hash: [u8; 32],
    pub ciphertext: Vec<u8>,
    pub replication_budget: u8,
}

impl StoredBundle {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    /// One hop consumed, one unit of replication budget spent — `None`
    /// (drop, don't forward) once either reaches zero. Mirrors the
    /// retired `siar_dtn::bundle::MeshBundle::try_consume_replication`'s
    /// own two-part gate.
    fn consumed(mut self) -> Option<Self> {
        if self.hop_limit == 0 || self.replication_budget == 0 {
            return None;
        }
        self.hop_limit -= 1;
        self.replication_budget -= 1;
        Some(self)
    }
}

/// Approximate size on the wire — `ciphertext`'s own length plus a
/// fixed overhead for the rest of the fields, close enough for a quota
/// eviction policy that only needs to compare bundles against each
/// other, not report a byte-exact figure to anyone.
fn approximate_size(bundle: &StoredBundle) -> u64 {
    bundle.ciphertext.len() as u64 + 96
}

/// A quota-bounded store of [`StoredBundle`]s, evicting lowest-priority
/// bundles first once `quota_bytes` would otherwise be exceeded — same
/// semantics the retired `siar_dtn::store::BundleStore` had. Kept
/// synchronous (unlike `siar_dtn_bundle::store::BundleStore`'s
/// `async_trait`): every real caller in this binary already holds this
/// behind a `std::sync::Mutex` for a short critical section, same as
/// every other piece of state in `main.rs` — introducing `async fn`
/// here would just mean immediately `block_on`-ing it back to
/// synchronous at every call site for no real benefit, since nothing
/// this store does ever actually performs I/O.
pub struct BundleStore {
    quota_bytes: u64,
    used_bytes: u64,
    bundles: std::collections::HashMap<MessageId, StoredBundle>,
}

impl BundleStore {
    pub fn new(quota_bytes: u64) -> Self {
        Self {
            quota_bytes,
            used_bytes: 0,
            bundles: std::collections::HashMap::new(),
        }
    }

    /// Inserts `bundle`, evicting existing bundles — lowest
    /// [`MessagePriority`] first, then oldest `created_at` within the
    /// same priority — until it fits within `quota_bytes`. Returns the
    /// ids of anything evicted to make room, same as the retired
    /// `BundleStore::insert`'s own return value.
    pub fn insert(&mut self, bundle: StoredBundle) -> Vec<MessageId> {
        let size = approximate_size(&bundle);
        let mut evicted = Vec::new();
        while self.used_bytes + size > self.quota_bytes {
            let Some(victim_id) = self
                .bundles
                .values()
                .min_by_key(|b| (priority_rank(b.priority), b.created_at))
                .map(|b| b.id)
            else {
                break; // store is empty but a single bundle still doesn't fit — accepted anyway below
            };
            if let Some(victim) = self.bundles.remove(&victim_id) {
                self.used_bytes -= approximate_size(&victim);
                evicted.push(victim_id);
            }
        }
        self.used_bytes += size;
        self.bundles.insert(bundle.id, bundle);
        evicted
    }

    pub fn get(&self, id: MessageId) -> Option<StoredBundle> {
        self.bundles.get(&id).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = &StoredBundle> {
        self.bundles.values()
    }

    /// Removes and returns `id`'s bundle with one hop/replication unit
    /// consumed — `None` either if `id` isn't stored, or if consuming
    /// would exhaust `hop_limit`/`replication_budget` (in which case
    /// the bundle is dropped from the store entirely, not just left
    /// alone — matching the retired type's own semantics: a bundle
    /// that can no longer be forwarded has no further reason to occupy
    /// quota).
    pub fn consume_for_forward(&mut self, id: MessageId) -> Option<StoredBundle> {
        let bundle = self.bundles.remove(&id)?;
        self.used_bytes -= approximate_size(&bundle);
        bundle.consumed()
    }

    /// Removes `id` outright — a direct delivery succeeded, so this
    /// copy no longer needs to occupy quota waiting for a forward
    /// attempt that will never come (matches the retired store's own
    /// `mark_delivered`, which likewise just removed the entry: this
    /// relay keeps no delivery receipt or history).
    pub fn mark_delivered(&mut self, id: MessageId) {
        if let Some(bundle) = self.bundles.remove(&id) {
            self.used_bytes -= approximate_size(&bundle);
        }
    }
}

fn priority_rank(priority: MessagePriority) -> u8 {
    match priority {
        MessagePriority::Emergency => 4,
        MessagePriority::Critical => 3,
        MessagePriority::Interactive | MessagePriority::Normal => 2,
        MessagePriority::Background => 1,
    }
}

/// Bounded seen-id deduplication — ported from the retired
/// `siar_dtn::dedup::SeenBundles`, generic here (`T` rather than a
/// fixed id type) since this relay needs to dedup two structurally
/// unrelated id types against two separate instances
/// (`MessageId` for `MeshEnvelope`/`TokenMailboxEnvelope`) — see
/// `siar_dtn_bundle::dedup::SeenBundles` (this workspace's *other*
/// post-reconciliation copy of the same logic, fixed to that crate's
/// own `BundleId`) for why a single shared generic type isn't used
/// across both: pulling in `siar-dtn-bundle` here, whose whole
/// `DtnBundle`/`RouteToken` model this file's own top doc comment just
/// explained doesn't apply to this binary's actual wire format, for
/// nothing but this one dedup helper isn't a trade worth making.
pub struct SeenIds<T> {
    capacity: usize,
    order: VecDeque<T>,
    set: HashSet<T>,
}

impl<T: Copy + Eq + Hash> SeenIds<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity >= 1,
            "a zero-capacity seen-set can never remember anything"
        );
        Self {
            capacity,
            order: VecDeque::with_capacity(capacity),
            set: HashSet::with_capacity(capacity),
        }
    }

    /// `true` if `id` had already been seen (caller should drop it);
    /// `false` on first sighting, in which case it's now recorded.
    pub fn check_and_record(&mut self, id: T) -> bool {
        if self.set.contains(&id) {
            return true;
        }
        if self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        self.order.push_back(id);
        self.set.insert(id);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(
        id: MessageId,
        priority: MessagePriority,
        created_at: u64,
        size: usize,
    ) -> StoredBundle {
        StoredBundle {
            id,
            destination: DeviceId::new(),
            created_at,
            expires_at: created_at + 1_000_000,
            hop_limit: 5,
            priority,
            payload_hash: [0u8; 32],
            ciphertext: vec![0u8; size],
            replication_budget: priority.default_replication_budget(),
        }
    }

    #[test]
    fn insert_and_get_round_trip() {
        let mut store = BundleStore::new(1_000_000);
        let id = MessageId::new();
        let evicted = store.insert(bundle(id, MessagePriority::Normal, 0, 10));
        assert!(evicted.is_empty());
        assert!(store.get(id).is_some());
    }

    #[test]
    fn quota_pressure_evicts_lowest_priority_first() {
        let mut store = BundleStore::new(200);
        let low = MessageId::new();
        let high = MessageId::new();
        store.insert(bundle(low, MessagePriority::Background, 0, 100));
        let evicted = store.insert(bundle(high, MessagePriority::Emergency, 1, 100));
        assert_eq!(evicted, vec![low]);
        assert!(store.get(high).is_some());
    }

    #[test]
    fn consume_for_forward_decrements_hop_limit_and_replication_budget() {
        let mut store = BundleStore::new(1_000_000);
        let id = MessageId::new();
        let mut b = bundle(id, MessagePriority::Normal, 0, 10);
        b.hop_limit = 2;
        b.replication_budget = 2;
        store.insert(b);
        let forwarded = store.consume_for_forward(id).expect("should forward once");
        assert_eq!(forwarded.hop_limit, 1);
        assert_eq!(forwarded.replication_budget, 1);
        // Removed from the store after consuming — a second call finds nothing.
        assert!(store.consume_for_forward(id).is_none());
    }

    #[test]
    fn consume_for_forward_returns_none_once_hop_limit_is_exhausted() {
        let mut store = BundleStore::new(1_000_000);
        let id = MessageId::new();
        let mut b = bundle(id, MessagePriority::Normal, 0, 10);
        b.hop_limit = 0;
        store.insert(b);
        assert!(store.consume_for_forward(id).is_none());
    }

    #[test]
    fn mark_delivered_removes_the_bundle() {
        let mut store = BundleStore::new(1_000_000);
        let id = MessageId::new();
        store.insert(bundle(id, MessagePriority::Normal, 0, 10));
        store.mark_delivered(id);
        assert!(store.get(id).is_none());
    }

    #[test]
    fn seen_ids_dedup_round_trip() {
        let mut seen: SeenIds<MessageId> = SeenIds::new(4);
        let id = MessageId::new();
        assert!(!seen.check_and_record(id));
        assert!(seen.check_and_record(id));
    }

    #[test]
    fn seen_ids_evicts_oldest_once_at_capacity() {
        let mut seen: SeenIds<MessageId> = SeenIds::new(2);
        let a = MessageId::new();
        let b = MessageId::new();
        let c = MessageId::new();
        seen.check_and_record(a);
        seen.check_and_record(b);
        seen.check_and_record(c);
        assert!(
            !seen.check_and_record(a),
            "a should have been evicted, so this is a fresh sighting"
        );
    }
}
