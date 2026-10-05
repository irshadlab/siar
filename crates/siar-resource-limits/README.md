# siar-resource-limits — spec 08 (Resource Limits & Backpressure)

Entry point for feature code: [`runtime::ResourceRuntime`] (§187-188).

```rust
let rt = ResourceRuntime::new(ResourceProfile::MobileNormal)?;   // validated up front
let permit = rt.admit(&request, now)?;                           // generic
let msg    = rt.messaging().acquire_message(trust, bytes, attachments, priority, now)?;
let xfer   = rt.files().acquire_transfer(peer, trust, size, chunk, interactive, now)?;
let bundle = rt.dtn().reserve_bundle(peer, trust, bytes, replication, hops, priority)?;
// platform adapters report raw facts only:
rt.report_android_trim(level, now);  rt.report_ios_memory_warning(now);
rt.evaluate_pressure(other_signals, now);
```

Everything is non-blocking and RAII: a refusal is a typed `ResourceError`
(`Deferred` = park durably and retry), dropping a permit releases
everything, including on cancellation or panic.

## §186 suggested structure → where it lives

The spec sketches one `comm-resource` crate with 20 files. The names below
are the spec's; the modules are this crate's (split finer where a topic
grew past one file).

| spec file | module(s) here |
|---|---|
| `budget.rs` | `budget`, `policy_layers`, `effective_limits`, `dynamic_config` |
| `profile.rs` | `profile`, `feature_limits`, `modes` |
| `owner.rs`, `request.rs` | `admission` (`ResourceOwner`, `ResourceRequest`), `types` |
| `admission.rs` | `admission`, `admission_pipeline`, `inbound`, `dtn_intake`, `cost`, `runtime` |
| `permit.rs` | `permits`, `reservation`, `op_holds`, `connections`, `slots` |
| `memory.rs` | `budget`, `os_memory` |
| `storage.rs` | `storage`, `storage_classes`, `restart` |
| `network.rs` | `bandwidth_fairness`, `storm`, `routing_limits` |
| `connection.rs`, `stream.rs` | `connections`, `slots`, `peer_quota` |
| `queue.rs` | `queue`, `aging`, `coalesce`, `fanout`, `event_log_pressure`, `frame` |
| `scheduler.rs` | `scheduler`, `fair_semaphore`, `preempt`, `cancel`, `tasks`, `supervisor` |
| `fairness.rs` | `bandwidth_fairness`, `peer_quota`, `extension_quota`, `accounting`, `abuse` |
| `pressure.rs` | `pressure`, `cpu_thermal`, `os_memory` |
| `quota.rs` | `peer_quota`, `extension_quota`, `accounting` |
| `metrics.rs`, `diagnostics.rs` | `metrics`, `snapshot`, `feedback`, `events` |
| `error.rs` | `error`, `safety` |

## §190 implementation phases → status

| phase | covers | modules |
|---|---|---|
| 1 Resource model | `ResourceKind/Profile/Budget/Owner`, `PressureState` | `types`, `profile`, `admission`, `storage` |
| 2 Permits | memory, connections, streams, tasks | `budget`, `connections`, `slots`, `reservation`, `tasks` |
| 3 Queues | bounded, priority, critical reserve | `queue`, `aging`, `event_log_pressure`, `fanout` |
| 4 Storage | quotas, reservations, watermarks, eviction signals | `storage`, `storage_classes` |
| 5 Fairness | peer, extension, priority, bandwidth | `peer_quota`, `extension_quota`, `bandwidth_fairness`, `aging` |
| 6 Platform pressure | Android, iOS, thermal, background | `os_memory`, `cpu_thermal`, `modes` |
| 7 Hardening | property, concurrency, flood, fuzz, bench | `tests/`, `fuzz/`, `benches/` |

## Verifying

```sh
cargo test -p siar-resource-limits                 # unit + property + concurrency + scenarios + DoD
cargo test -p siar-resource-limits --release       # same, debug assertions off
cargo bench -p siar-resource-limits                # median/min/max over 7 samples
cd crates/siar-resource-limits && cargo +nightly fuzz run frame_decoder   # see fuzz/README.md
```
