# Routing/DTN reconciliation — migration plan

Tracks retiring the two next.md-era crates (`siar-routing`, `siar-dtn`)
into their sys-arch-era replacements (`siar-routing-policy`,
`siar-dtn-bundle`). See `[[resilient-mesh]]`'s "Unresolved-by-design
reconciliation questions" for how this gap was first named.

## Ground truth (verified against real code, 2026-09-17)

Neither spec text (`03-transport-routing-policy-engine-architecture.md`,
`06-dtn-store-carry-forward-architecture.md`) names the old crates —
this reconciliation is entirely this workspace's own decision, not
spec-mandated.

Real dependency edges as of this pass:
- `siar-routing` → `siar-domain`, `siar-dtn` (only for a
  `MessagePriority` re-export), `iroh`
- `siar-connectivity::TransportManager` → the only real consumer of
  `siar-routing` (`PathTable`/`PathEntry`/`LinkHealth`/`SendOutcome`/
  `NextHop`)
- `siar-routing-policy` → `siar-domain`, `siar-identity-multidevice`,
  `siar-protocol-ext`. Per `ROADMAP.md`: *"confirmed no other crate in
  the workspace depends on `siar-routing-policy` besides
  `siar-dtn-bundle`."*
- `siar-dtn` → `siar-domain` only. Real consumers: `siar-routing`
  (trivial re-export) and `siar-testkit::mesh_sim` (a simulation
  harness built directly against `MeshBundle`/`BundleStore`)
- `siar-dtn-bundle` → `siar-blob-manifest`, `siar-event-log`,
  `siar-routing-policy` (via `routing_bridge::select_dtn_bundle_policy`
  only — never `decide_route`/`plan_route`/`RoutingEngine`).
  Deliberately no dependency on `siar-dtn` (stated in its own
  `Cargo.toml`).
- `apps/emergency-node` is a declared workspace member (`Cargo.toml`)
  not present in the uploaded tarball this plan was built from — the
  only binary actually on disk is `apps/cli`, which touches none of
  the six crates above. This plan assumes `emergency-node`'s real
  calls into `TransportManager` (`sync_local_peers`/
  `record_send_outcome`/`path_table`) exist in the real tree and
  preserves those exact signatures.

## Item-by-item disposition

### Routing (`siar-routing` → `siar-routing-policy` + `siar-connectivity`)

| Old item | Fate | Notes |
|---|---|---|
| `path::PathEntry`, `score::best_route`/`route_score`, `PayloadSizeClass` | **Delete** | Fully superseded by `PathCandidate`/`decide_route`+`plan_route`'s 7-layer stack |
| `path::capabilities_for` (`BandwidthClass`/`LatencyClass`) | **Delete**, shape-superseded | `PathMetrics.rtt_millis`/`estimated_bandwidth` are already numeric; `PathCapabilities` already covers the boolean feature side |
| `link_health::LinkHealth`/`SendOutcome` | **Port** → `siar-routing-policy::link_health` | Retargeted to write `PathMetrics.rtt_millis`/`.packet_loss` instead of a separate struct; adds `health_from_reliability` closing the "qualitative-only `RouteHealth`" gap |
| `path::compose_via_relay`/`RelayAdvertisement` | **Port** → `siar-routing-policy::relay_composition` | Produces a `PathCandidate` with `transport: TransportKind::MeshRelay` (already exists in `types.rs`, unused until now) — no `PathCandidate` schema change needed |
| `scheduler::PriorityScheduler`/`congestion_ceiling` | **Port, redesigned** → `siar-routing-policy::congestion` | Old type bundled storage+occupancy; new `CongestionTracker` is occupancy-only (real storage already belongs to `siar-protocol-ext::FairScheduler`, per `dispatch.rs`'s own "deliberately not wired into" precedent for `fairness::RoundRobinFairQueue`). Rebuilt on `TrafficPriority` (6 tiers, already used throughout this crate) instead of a third priority enum |
| `path::classify_endpoint_addr` | **Move** → `siar-connectivity` | Real transport-address heuristic; `siar-routing-policy` deliberately has zero transport dependency |
| `device_routes::DeviceRoutes` | **Move** → `siar-connectivity` | Still needed as the `EndpointId ↔ DeviceId` join — live transport only ever hands out `EndpointId`, `PathCandidate.peer` is `DeviceId` |
| `siar-connectivity::TransportManager` | **Rewritten** | Feeds `siar-routing-policy` (via the four ports above) instead of `siar-routing::PathTable`; public method names/signatures preserved for `emergency-node` |
| `siar-routing` crate | **Deleted** | Once all of the above land and compile clean |

### DTN (`siar-dtn` → `siar-dtn-bundle`)

| Old item | Fate | Notes |
|---|---|---|
| `bundle::MeshBundle`, sync `store::BundleStore` | **Delete** | Superseded by `DtnBundle` (opaque `RouteToken` destination — real privacy upgrade over plain `DeviceId`) and the async `store::BundleStore` trait / `InMemoryBundleStore` |
| `dedup::SeenBundles` | **Port** → `siar-dtn-bundle::dedup` | Genuine gap — `siar-dtn-bundle` has no seen-set/dedup module at all; re-keyed on `types::BundleId` |
| `siar-testkit::mesh_sim` | **Rewritten** | Against `DtnBundle`/`store::BundleStore` trait once `siar-dtn` is retired |
| `siar-routing`'s `MessagePriority` re-export dependency | **Dropped** | Import `siar_domain::MessagePriority` directly — nothing to port |
| `siar-dtn` crate | **Deleted** | Once the above land and compile clean |

## Sequencing

1. `siar-dtn-bundle::dedup` (smallest, no downstream rewrite attached)
2. `siar-routing-policy::link_health`
3. `siar-routing-policy::relay_composition`
4. `siar-routing-policy::congestion`
5. `siar-connectivity::TransportManager` rewrite (depends on 2–4)
6. `siar-testkit::mesh_sim` rewrite (depends on 1)
7. Remove `siar-routing`/`siar-dtn` from `Cargo.toml` workspace members; delete the two crate directories
8. Full workspace `cargo check --workspace --all-targets` + `cargo test --workspace` + `clippy -D warnings` + `fmt --check`

## Status

- [x] Step 1 — `siar-dtn-bundle::dedup` (6/6 new tests, 43/43 crate-wide, clippy clean)
- [x] Step 2 — `siar-routing-policy::link_health` (9/9 new tests)
- [x] Step 3 — `siar-routing-policy::relay_composition` (5/5 new tests)
- [x] Step 4 — `siar-routing-policy::congestion` (9/9 new tests; found+fixed one real "at-or-above 0.5" boundary test bug)
- [x] Step 5 — `siar-connectivity::TransportManager` rewrite (split into pure-logic `CandidateTable` + thin `TransportManager`; moved `DeviceRoutes`, extended with a new reverse `EndpointId -> DeviceId` lookup; moved `classify_endpoint_addr`/`capabilities_for`; 23/23 tests)
- [x] Step 6 — `siar-testkit::mesh_sim` rewrite (against `siar-dtn-bundle`'s async `BundleStore` via `futures-executor::block_on`; dropped the no-longer-meaningful `quota_bytes` param; 5/5 tests including a new expiry test)
- [x] Step 7 — deleted `siar-routing`/`siar-dtn`, removed from workspace `Cargo.toml` members + `[workspace.dependencies]`; confirmed zero remaining external references first
- [x] Step 8 — full workspace verification: `cargo check --workspace --all-targets` clean, `cargo test --workspace` **1,316 passed, 0 failed**, `cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo fmt --all -- --check` clean

All 8 steps complete as of this pass (2026-09-17). `siar-routing` and `siar-dtn` no longer exist in this workspace.

## Notes for next time

- `TransportManager`/`CandidateTable`'s candidate store is keyed by `(DeviceId, TransportKind)`, not a single `PathCandidate` per device — a peer reachable over two transports now has two independent candidates.
- **Real, still-open gap, named rather than papered over**: `sync_local_peers`/`record_send_outcome` can only act on an `EndpointId` that `DeviceRoutes` already has a recorded `DeviceId` for (via `record_device_endpoint`, e.g. a real `MailboxCheckIn`). An `EndpointId` observed from mDNS with no prior disclosure is silently skipped. Closing this needs a real `siar-identity-multidevice` integration — out of scope for this pass.
- If `apps/emergency-node`'s real source calls `TransportManager`'s old method names (`path_table()`, or a different `record_send_outcome` signature), those call sites will need updating to the new `candidates()`/`candidates_for()`/`record_send_outcome(destination, kind, outcome)` API — this pass only had `apps/cli` on disk to verify against, not `emergency-node`.
- New workspace dependency added: `futures-executor = "0.3"` (used only by `siar-testkit::mesh_sim` to drive `siar-dtn-bundle`'s async `BundleStore` trait synchronously — no tokio runtime pulled in).
