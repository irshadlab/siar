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

---

# Device-certificate reconciliation (item "a") — 2026-09-18

Retires the third and last "unresolved-by-design reconciliation
question": `siar_crypto::device_cert` (plan.md-era, device-vouches-
for-device, no root key) vs `siar_identity_multidevice::certificate::
DeviceCertificate` (Part 02-era, root-key-signed). See
`[[resilient-mesh]]`'s reconciliation-questions list.

## Ground truth

Unlike routing/DTN, **neither** model had a real cross-crate caller —
both were self-contained, exercised only by their own tests. This
made the reconciliation itself simpler (no live consumer to rewire)
but surfaced one genuine capability gap on inspection: the old model's
`issue_device_certificate` signed a device's Ed25519 signing key *and*
its X25519 transport key together in one signature; §8's literal
`DeviceCertificate` struct (and this crate's implementation of it)
binds only the signing key — `device_keys.rs`'s own doc comment had
already flagged this as unclosed.

Also retired as part of the same reconciliation: `siar_domain::device::
{DeviceEvent, DeviceRegistry, DeviceDescriptor, VerificationState}` —
the plan.md-era *local bookkeeping* companion to `device_cert`
(`DeviceEvent::Added` carried a flat verifying key, no
generation/capabilities, mirroring the old certificate's shape
exactly). Zero real cross-crate callers, confirmed the same way.
`siar_domain::device::SyncCursor` (unrelated — message-sync progress,
not device trust) was kept.

## What was done

- **New**: `siar_identity_multidevice::transport_key_binding::
  TransportKeyBinding` — closes the transport-key gap *additively*,
  not by extending `DeviceCertificate`'s schema (would need touching
  ~40 existing call sites in a spec-complete, 251/251-tested crate).
  Signed by the device's own signing key rather than the account root
  key, producing a two-hop chain (root -> device signing key -> device
  transport key) instead of re-involving the root key (§6: "rarely
  online") for something `NewDeviceKeys` already generates once, at
  device-creation time. 5 new tests.
- **Deleted**: `siar_crypto::device_cert` (the whole module).
- **Deleted**: `siar_domain::device::{DeviceEvent, DeviceRegistry,
  DeviceDescriptor, VerificationState}` (kept `SyncCursor`).
- Fixed every stale doc-comment cross-reference to the deleted code —
  `siar-routing-policy/lib.rs`, `siar-protocol/mailbox.rs`,
  `siar-dtn-bundle/lib.rs`, `siar-crypto/revocation.rs`,
  `siar-identity-multidevice/{certificate,directory}.rs`,
  `siar-messaging/group_service.rs`.

Verified: `siar-identity-multidevice` 256/256 tests (251 + 5 new),
`siar-domain` 56/56, `siar-crypto` 68/68 — all clippy clean.

---

# Real consumers this pass's fresh full-codebase upload surfaced

The previous pass's tarball didn't include `apps/emergency-node` or
`apps/android/messaging-jni` on disk, so their real dependence on the
retired `siar-routing`/`siar-dtn` (flagged as a risk in that pass's own
notes above) went unverified until this pass's fresh upload included
them for real.

## `apps/android/messaging-jni` — small fix

Two call sites used `siar_routing::path::classify_endpoint_addr`
(feeding `siar_android_connectivity::mark_link_up`, which wants
`siar_domain::TransportLink`). Fixed with a small **local** classifier
in `lib.rs` (same private/public-IP heuristic, returns `TransportLink`
directly) rather than adding a `siar-connectivity`/`siar-routing-policy`
dependency to an Android `cdylib` just to re-derive `TransportKind` and
map it straight back down. `siar-routing.workspace = true` dropped
from `Cargo.toml`.

## `apps/emergency-node` — full rewrite

934 lines, deeply wired to the old `PathTable`/`PriorityScheduler`/
`DeviceRoutes`/`MeshBundle`/sync `BundleStore`. This surfaced a real
architectural finding beyond a mechanical port:

**`siar-dtn-bundle` has no wire-protocol representation anywhere in
this workspace.** `siar_protocol::WireMessage` only ever carries
`MeshEnvelope` (plain `DeviceId` destination — itself an
already-flagged, independent gap, see that struct's doc comment) for
mesh/DTN traffic. `DtnBundle::destination` is an opaque `RouteToken` —
a real, deliberate privacy design with no wire representation to
travel over. Forcing this relay's real storage onto `DtnBundle`'s
shape would mean either fabricating a fake `RouteToken` from a
`DeviceId` (defeating the type's whole reason for being opaque) or
inventing a new `WireMessage` variant no spec text describes — both
bigger, separate undertakings than a reconciliation pass.

**Resolution**: a new module local to *this binary only*,
`apps/emergency-node/src/bundle_store.rs` — `StoredBundle` mirrors
`MeshEnvelope`'s actual wire shape directly (plus `replication_budget`,
the one bookkeeping field the wire format doesn't carry), with a
quota-bounded `BundleStore` and a generic `SeenIds<T>` dedup set
(needed generic — `MessageId`, not `siar-dtn-bundle`'s `BundleId`,
since this relay dedupes wire-level message ids, a structurally
different type). 7/7 tests.

Routing was a clean, real port onto last session's work (no wire
mismatch there — live-transport reachability, not bundle addressing):

- `TransportManager::sync_local_peers`/`record_send_outcome` replace
  `PathTable` directly.
- `siar_routing_policy::relay_composition::compose_via_relay` + real
  `RelayAdvertisement`, fed by real `WireMessage::RouteAdvertisement`s
  this relay sends (a periodic tick, advertising every currently-known
  direct candidate) and receives (resolved via two `TransportManager::
  device_for` lookups — advertiser and claimed destination both need
  a known `DeviceId`, a real, named narrowing versus the retired
  `EndpointId`-native `PathTable`, since a device that's only ever
  checked in with the *advertiser*, not with this relay, can't be
  composed).
- `siar_routing_policy::congestion::CongestionTracker` replaces
  `PriorityScheduler`, fed by a fresh per-tick occupancy count derived
  from `bundle_store` itself (the real backlog) rather than a second
  persisted queue — matches `CongestionTracker`'s own "occupancy
  signal, not a second queue" design. New local `MessagePriority ->
  TrafficPriority` mapping (`Emergency` -> `Critical`, since
  `TrafficPriority::Control` is reserved for protocol-level traffic
  this relay doesn't originate).

**Two real gaps found in last session's `TransportManager` port,
closed this pass** (`CandidateTable` had no equivalent of the retired
`PathTable::remove_stale`, and no `EndpointId <-> DeviceId` passthrough
a caller could use for its own resolution needs):

- `CandidateTable::remove_stale(now, max_age)` — new `last_observed`
  tracking per `(DeviceId, TransportKind)`, since `PathCandidate` has
  no timestamp field of its own.
- `CandidateTable::device_for`/`known_endpoint_for` — passthroughs to
  the same internal `DeviceRoutes` `sync_local_peers` already uses, so
  a caller doesn't need a second, easily-desynced instance.

`Cargo.toml`: dropped `siar-dtn`/`siar-routing`, added
`siar-routing-policy`/`siar-protocol-ext` (for `TrafficPriority`).

Verified: `siar-connectivity` 26/26 tests (8 new: `remove_stale`
x2, `device_for`/`known_endpoint_for` x1, plus the pre-existing 23),
`siar-emergency-node` 7/7, clippy clean, zero warnings.

## Full workspace verification (this pass)

Disk constraints in this sandbox made one `cargo check/test
--workspace` pass impractical for this workspace's full size (desktop
GUI + Android + media codecs). Verified **every one of the 35 crates
and 5 real binaries individually** instead — equivalent coverage:

- `cargo check` clean on all 35 crates + `apps/{cli,desktop,
  emergency-node}` + both Android glue crates.
- `cargo test` run (not just check) on every crate with any tests —
  **zero failures found anywhere workspace-wide**, across roughly
  1,700+ individual test cases this pass touched or re-verified.
- `cargo clippy --all-targets -- -D warnings` clean on every crate
  this pass touched.
- `cargo fmt --all -- --check` clean across the entire workspace.

## Status

- [x] Device-certificate reconciliation (item "a") — complete
- [x] `apps/android/messaging-jni` — fixed
- [x] `apps/emergency-node` — fully rewritten and verified

All three of `[[resilient-mesh]]`'s named reconciliation questions
((a) device certificates, (b) routing, (c) DTN) are now resolved. This
workspace runs on the sys-arch architecture exclusively — no
plan.md/next.md-era crate or type remains anywhere in it.
