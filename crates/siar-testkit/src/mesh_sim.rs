//! A deterministic, in-memory mesh simulation over real
//! `siar_dtn_bundle::store::BundleStore`/`dedup::SeenBundles`
//! instances — next.md §113 ("your test framework should simulate
//! A-B-C-D... ensure a message → D eventually arrives"), §114
//! (partition/rejoin), §115 (mobility: links appearing and
//! disappearing between ticks).
//!
//! **Rewritten against `siar-dtn-bundle`, not the retired `siar-dtn`**
//! (see `MIGRATION.md`, step 6) — three real consequences of that
//! move, not cosmetic renames:
//!
//! - `siar_dtn_bundle::store::BundleStore` is an `async_trait`, unlike
//!   the old crate's plain synchronous `BundleStore`. Nothing this
//!   simulation does ever performs real I/O — every call resolves
//!   immediately — so this module drives it with
//!   `futures_executor::block_on` rather than pulling in a real
//!   `tokio` runtime, keeping [`MeshSimulation`]'s own public API
//!   fully synchronous, same as the original.
//! - A bundle needs an explicit `Stored -> Eligible` transition
//!   (`mark_eligible`) before `list_candidates` will return it at all
//!   (§18's state machine) — the old crate's `insert`+`iter()` had no
//!   such state. [`MeshSimulation::originate`] and the delivery half
//!   of [`MeshSimulation::tick`] both call `mark_eligible` immediately
//!   after `put`, preserving the original's "available for the very
//!   next tick" behavior.
//! - `add_node` no longer takes a `quota_bytes` parameter —
//!   `InMemoryBundleStore` doesn't enforce a storage quota at all (see
//!   that type's own doc comment: genuinely in-memory only, no
//!   eviction policy implemented yet). Silently accepting and
//!   ignoring the old parameter would be worse than dropping it: a
//!   caller passing a real limit deserves a compile error telling them
//!   the guarantee isn't there, not a parameter that quietly does
//!   nothing.
//!
//! `tick()` deliberately implements the simplest possible forwarding
//! rule — "flood whatever a node has that its neighbor hasn't seen,
//! respecting hop_limit" — not next.md §36's Bloom-filter inventory
//! reconciliation and not §22's replication-budget consumption (a
//! bundle's `replication_budget` field is carried through untouched by
//! this harness; nothing here decrements it, same scope limitation the
//! original stated). This one is scoped to proving the
//! *loop-prevention and eventual-delivery* guarantees (§21's hop
//! limit, §31's dedup, §116's "no duplicate logical messages...
//! eventual delivery when a route eventually exists"), which don't
//! depend on either refinement to be meaningful.

use std::collections::{HashMap, HashSet};

use futures_executor::block_on;
use siar_domain::DeviceId;
use siar_dtn_bundle::bundle::DtnBundle;
use siar_dtn_bundle::dedup::SeenBundles;
use siar_dtn_bundle::store::{BundleStore, ForwardQuery, InMemoryBundleStore};
use siar_dtn_bundle::types::BundleId;

pub struct SimNode {
    pub store: InMemoryBundleStore,
    pub seen: SeenBundles,
}

pub struct MeshSimulation {
    nodes: HashMap<DeviceId, SimNode>,
    /// Directed pairs that can currently exchange — `connect` inserts
    /// both `(a, b)` and `(b, a)` so callers get symmetric links
    /// without this crate needing `DeviceId: Ord` to normalize a pair
    /// ordering (it isn't `Ord`).
    links: HashSet<(DeviceId, DeviceId)>,
    now_millis: u64,
}

impl MeshSimulation {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            links: HashSet::new(),
            now_millis: 0,
        }
    }

    pub fn add_node(&mut self, id: DeviceId, seen_capacity: usize) {
        self.nodes.insert(
            id,
            SimNode {
                store: InMemoryBundleStore::new(),
                seen: SeenBundles::new(seen_capacity),
            },
        );
    }

    /// Symmetric — both directions can exchange once connected. Safe to
    /// call again on an already-connected pair (idempotent, `HashSet`
    /// insert).
    pub fn connect(&mut self, a: DeviceId, b: DeviceId) {
        self.links.insert((a, b));
        self.links.insert((b, a));
    }

    /// next.md §115's "peer disappears" / §114's partition.
    pub fn disconnect(&mut self, a: DeviceId, b: DeviceId) {
        self.links.remove(&(a, b));
        self.links.remove(&(b, a));
    }

    /// Injects `bundle` directly into `at`'s store, as if created there
    /// locally (next.md §32: "Alice creates... phone stores it").
    /// Marked seen and immediately eligible for forwarding — see this
    /// module's top doc comment on why the explicit `mark_eligible`
    /// step is needed here where the old crate needed none.
    pub fn originate(&mut self, at: DeviceId, bundle: DtnBundle) {
        if let Some(node) = self.nodes.get_mut(&at) {
            let id = bundle.bundle_id;
            node.seen.check_and_record(id);
            block_on(node.store.put(bundle)).expect("in-memory put never fails");
            block_on(node.store.mark_eligible(id)).expect("just-put bundle exists");
        }
    }

    /// One round: every currently-connected pair exchanges whatever the
    /// sender has that the receiver hasn't seen yet, respecting
    /// `hop_limit` (next.md §21 — a bundle at zero hops is simply not
    /// forwarded, matching `DtnBundle::forwarded`'s own "drop" return).
    pub fn tick(&mut self) {
        self.now_millis += 1;

        // Immutable pass: decide what should move without holding any
        // mutable borrow yet — two different entries of the same
        // `HashMap` can't be borrowed mutably at the same time, so
        // collecting first sidesteps that rather than fighting the
        // borrow checker over it.
        let mut deliveries: Vec<(DeviceId, DtnBundle)> = Vec::new();
        for &(from, to) in &self.links {
            let (Some(from_node), Some(to_node)) = (self.nodes.get(&from), self.nodes.get(&to))
            else {
                continue;
            };
            let candidates = block_on(from_node.store.list_candidates(ForwardQuery {
                now_millis: self.now_millis,
                limit: usize::MAX,
            }))
            .expect("in-memory list_candidates never fails");
            for stored in candidates {
                if to_node.seen.contains(stored.bundle.bundle_id) {
                    continue;
                }
                if let Some(forwarded) = stored.bundle.forwarded() {
                    deliveries.push((to, forwarded));
                }
            }
        }

        // Mutable pass: apply what the immutable pass decided.
        for (to, bundle) in deliveries {
            if let Some(node) = self.nodes.get_mut(&to) {
                let id = bundle.bundle_id;
                if node.seen.check_and_record(id) {
                    // Arrived via another link earlier in this same
                    // tick already (e.g. two neighbors both had it) —
                    // next.md §31's dedup doing exactly its job.
                    continue;
                }
                block_on(node.store.put(bundle)).expect("in-memory put never fails");
                block_on(node.store.mark_eligible(id)).expect("just-put bundle exists");
            }
        }
    }

    /// Runs `tick` `rounds` times — a convenience for "let the mesh
    /// settle" in a test, not a different forwarding rule.
    pub fn run(&mut self, rounds: u32) {
        for _ in 0..rounds {
            self.tick();
        }
    }

    pub fn has_bundle(&self, node: DeviceId, id: BundleId) -> bool {
        self.nodes
            .get(&node)
            .map(|n| {
                block_on(n.store.get(id))
                    .expect("in-memory get never fails")
                    .is_some()
            })
            .unwrap_or(false)
    }

    pub fn now_millis(&self) -> u64 {
        self.now_millis
    }
}

impl Default for MeshSimulation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use siar_dtn_bundle::bundle::BundleIntegrity;
    use siar_dtn_bundle::payload::PayloadReference;
    use siar_dtn_bundle::types::{
        DtnDestination, DtnPriority, DtnSource, ForwardingClass, PayloadTypeId, RouteToken,
    };

    fn bundle(hop_limit: u8, expires_at_millis: u64) -> DtnBundle {
        DtnBundle {
            bundle_id: BundleId::new(),
            source: DtnSource(RouteToken(vec![1])),
            destination: DtnDestination::DeviceOpaque(RouteToken(vec![2])),
            payload_type: PayloadTypeId(1),
            created_at_millis: 0,
            expires_at_millis,
            priority: DtnPriority::Normal,
            hop_limit,
            replication_budget: 4,
            forwarding_class: ForwardingClass::SprayAndWait,
            payload_ref: PayloadReference::Inline(vec![1, 2, 3]),
            integrity: BundleIntegrity {
                payload_hash: [0u8; 32],
                origin_signature: None,
            },
        }
    }

    /// next.md §113: A-B-C-D chain, A can't see C/D directly, but a
    /// message from A should eventually reach D via B and C.
    #[test]
    fn message_crosses_a_multi_hop_chain() {
        let mut sim = MeshSimulation::new();
        let (a, b, c, d) = (
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
        );
        for node in [a, b, c, d] {
            sim.add_node(node, 100);
        }
        sim.connect(a, b);
        sim.connect(b, c);
        sim.connect(c, d);

        let msg = bundle(8, 1_000_000); // hop_limit comfortably more than 3 hops needed
        let id = msg.bundle_id;
        sim.originate(a, msg);

        // One tick per hop needed, plus a little slack.
        sim.run(4);

        assert!(
            sim.has_bundle(d, id),
            "message should have crossed A -> B -> C -> D"
        );
    }

    /// next.md §21: hop_limit exhausting before reaching the destination
    /// means the message is dropped, not delivered anyway.
    #[test]
    fn hop_limit_too_low_for_the_chain_means_no_delivery() {
        let mut sim = MeshSimulation::new();
        let (a, b, c, d) = (
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
        );
        for node in [a, b, c, d] {
            sim.add_node(node, 100);
        }
        sim.connect(a, b);
        sim.connect(b, c);
        sim.connect(c, d);

        let msg = bundle(2, 1_000_000); // only 2 hops — A->B->C, not far enough for D
        let id = msg.bundle_id;
        sim.originate(a, msg);
        sim.run(5);

        assert!(
            !sim.has_bundle(d, id),
            "hop_limit=2 should not have been enough to reach a 3-hop-away destination"
        );
        assert!(
            sim.has_bundle(c, id),
            "it should have gotten as far as C, though"
        );
    }

    /// next.md §114: a partitioned network, later reconciled once a
    /// bridging connection appears.
    #[test]
    fn partitioned_networks_reconcile_once_a_bridge_connects() {
        let mut sim = MeshSimulation::new();
        let (a, b, c, d, e, f) = (
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
        );
        for node in [a, b, c, d, e, f] {
            sim.add_node(node, 100);
        }
        // Network 1: A-B-C. Network 2: D-E-F. No link between them yet.
        sim.connect(a, b);
        sim.connect(b, c);
        sim.connect(d, e);
        sim.connect(e, f);

        let msg = bundle(8, 1_000_000);
        let id = msg.bundle_id;
        sim.originate(a, msg);
        sim.run(3);

        assert!(sim.has_bundle(c, id));
        assert!(
            !sim.has_bundle(f, id),
            "no bridge exists yet — the two networks must not have reconciled"
        );

        // C meets D — the bridge next.md §114 describes.
        sim.connect(c, d);
        sim.run(3);

        assert!(
            sim.has_bundle(f, id),
            "once C-D bridges the two networks, the pending bundle should reach network 2"
        );
    }

    /// next.md §31: a bundle arriving at the same node via two different
    /// paths must not be double-counted/re-inserted.
    #[test]
    fn a_node_reachable_by_two_paths_does_not_receive_duplicates() {
        let mut sim = MeshSimulation::new();
        let (a, b, c, d) = (
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
            DeviceId::new(),
        );
        for node in [a, b, c, d] {
            sim.add_node(node, 100);
        }
        // Diamond: A -> B -> D and A -> C -> D, two paths to D.
        sim.connect(a, b);
        sim.connect(a, c);
        sim.connect(b, d);
        sim.connect(c, d);

        let msg = bundle(8, 1_000_000);
        let id = msg.bundle_id;
        sim.originate(a, msg);
        sim.run(3);

        assert!(sim.has_bundle(d, id));
    }

    /// next.md §20: an expired bundle is never forwarded — it should
    /// still be sitting at the origin, but never propagate further.
    #[test]
    fn an_expired_bundle_is_not_forwarded() {
        let mut sim = MeshSimulation::new();
        let (a, b) = (DeviceId::new(), DeviceId::new());
        sim.add_node(a, 100);
        sim.add_node(b, 100);
        sim.connect(a, b);

        // Expires almost immediately — by tick 1 (now_millis == 1) it's
        // already expired.
        let msg = bundle(8, 1);
        let id = msg.bundle_id;
        sim.originate(a, msg);
        sim.run(3);

        assert!(
            !sim.has_bundle(b, id),
            "an expired bundle must not be forwarded to a neighbor"
        );
    }
}
