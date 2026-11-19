# Part 08 — Resource Limits & Backpressure Architecture

Source spec: `sys-arch/08-resource-limits-backpressure-architecture.md` — 193 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**✅ ~192/193 (99%) sections completed.**
§187-188 public facade (`ResourceRuntime`) implemented and verified; Definition of Done (§191) executable and passing; §186 and §190 mapped in the crate README.
434 tests passing across unit, property, concurrency, scenario, fuzz, audit, DoD, and facade suites. Clippy `-D warnings` and fmt clean.

## Implementing crate(s)

- `siar-resource-limits` — hierarchical resource limit policies, token buckets, aging priority queues, fair semaphores, admission pipelines, memory pressure monitors, sharded task supervisors, and restart recovery ledgers.

## Round-by-round history

- **Round 4**: Added §58-61, 69, 81/83/87, 91-97, 99-104, 124-126, 129, 169-171.
- **Round 5**: Added ~15 sections: §42-43, 62-67, 78-79, 111-112, 132-136.
- **Round 6**: Added +14 sections: §5-7, 9-16, 141-143 (layers/hard+soft limits, global budget + borrowing, profiles, config validation, dynamic configuration reload).
- **Round 7**: Added +14 sections: §28, 98, 144-145, 153-155, 158-164 (account tree/tenant isolation, fair semaphore, preemption + safe pause, cancel scopes, task registry, supervisor, sharded statistics).
- **Round 8**: Added +15 sections: §68, 70-77, 82, 84-85, 127-128, 131 (routing/discovery/negotiation limits, event-log/projection/outbox pressure, IPC/FFI/plugin fan-out queues, verification budget + handshake pools, OS memory pressure).
- **Round 9**: Added +13 sections: §86-89, 121-122, 146-152 (CPU/thermal/energy limits, effective limits + runtime tightening, composable runtime modes).
- **Round 10**: Added +15 sections: §80, 105-110, 137-140, 165-168 (snapshots, bounded-cardinality metrics, user/developer feedback, batched events + journal filter, restart/crash recovery planning).
- **Round 11**: Added +13 sections: §113-120, 123, 129-130, 156-157 (per-feature limits, pre-allocation frame decoder, unknown-peer + connection admission pipelines, sharded scheduler).
- **Round 12**: Added §45, 169-171, 172-185, 189, 191 (storage classes, error classes, property/concurrency/scenario/fuzz tests, benchmarks; `PeerQuota::clamped_to`, `storage_classes`, `ResourceError` variants).
- **Round 13**: Implemented public facade (`ResourceRuntime`, §187-188), executable Definition of Done (§191) audit tests, real `cargo-fuzz` harness, and multi-sample benchmarks.

### Defects caught and fixed in audit passes

- **Clock regression**: `TokenBucket` moved its refill mark backwards on a regressing clock, granting a free burst on the next forward reading; fixed to monotonic tracking.
- **Queue starvation**: Strict-priority `BoundedPriorityQueue` starved low tiers; introduced `AgingQueue` as a starvation-safe alternative.
- **Counter underflow**: `BoundedCounter::release` used plain `fetch_sub` which could wrap to `u64::MAX`; hardened with saturating counters and debug asserts.
- **Ephemeral IDs**: Identified that `ReservationId` is a per-process counter restarting at 1 and must never be persisted; `restart.rs` keys durable records on `OpId`/`DurableResId` instead.
- **Cross-validation bug**: Detected configuration flaw where embedded DTN max bundle (1 MiB) exceeded hard frame limit (256 KiB); added cross-validation assertions.
- **Device-independent quota clamping**: `PeerQuota::for_trust_class` was device-independent, allowing a trusted peer quota to exceed an embedded node's stream budget; added `clamped_to`.
- **Storage classes**: §191 required separate durable/cache/staging/relay quotas; implemented `storage_classes` and class ledgers.
- **Outbox accounting**: `OutboxAccounting` charged critical entries in reserve against normal capacity; fixed to reserve buckets.
- **Recovery restore collision**: `plan_recovery`/`restore_reservations` could place one durable ID in two buckets and restore the wrong record on repeated IDs (found via fuzz target); resolved.

## Known gaps / remaining items

- Wiring the public facade (`ResourceRuntime`) into the node runtime coordinator once the top-level daemon lifecycle settles.
- Prose-only sections: §1-2, 186, 190, 192-193.
