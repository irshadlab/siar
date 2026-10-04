//! Part 08 — Resource Limits & Backpressure Architecture.
//!
//! Built the same way as this workspace's other Part-01–07 crates
//! (see [[resilient-mesh]] project memory): one real, deliberately-
//! scoped, tested slice per pass, tracked honestly against the spec's
//! own section numbers.
//!
//! ## Round 12 — §45, 169-171, 172-185, 189, 191 (verification & completion)
//!
//! - [`storage_classes`] — durable / cache / staging / relay quotas with a
//!   shared ceiling that excludes the critical reserve, and an eviction
//!   plan (cache first, then relay; durable and staging never
//!   policy-evicted). Added because the Definition of Done requires
//!   separate class quotas and none existed.
//! - [`error`] — the spec's nine `ResourceError` variants and §170's four
//!   [`error::RetryClass`] classes alongside the existing finer-grained
//!   [`error::Retryability`]; `tests/api_audit.rs` enforces §171 (no
//!   string/boxed errors in the public API) and zero `unsafe`.
//! - [`peer_quota::PeerQuota::clamped_to`] — a per-trust quota could exceed
//!   an entire small device's global limit (a `LocalOwnDevice` peer: 512
//!   streams vs Embedded's 16); now clamped, tighten-only.
//! - `tests/`: `properties.rs` (§173, proptest), `concurrency.rs` (§174),
//!   `scenarios.rs` (§175-183, one test per spec scenario),
//!   `fuzzing.rs` (§184, in-tree fuzz targets), `definition_of_done.rs`
//!   (§191, one test per bullet, §189's first-implementation list
//!   included). `benches/resource_bench.rs` (§185, std-only,
//!   `cargo bench`).
//!
//! ## Round 11 — §113-120, 123, 129-130, 156-157 (limits, ingress, scheduler)
//!
//! - [`feature_limits`] — the eight per-feature limit sets, derived from
//!   the profile config and cross-validated against it and the hard
//!   limits (this caught a real default error: an Embedded DTN bundle
//!   larger than the hard frame limit). File offers are checked from
//!   their declared numbers, with manifest size *derived* from file and
//!   chunk size, so a tiny chunk size can't smuggle in a huge manifest.
//! - [`frame`] — length-prefixed decoder that rejects an oversize header
//!   before allocating, grows its body buffer only as bytes arrive, and
//!   poisons itself on error.
//! - [`admission_pipeline`] — unknown-peer gate (length → syntax →
//!   class-wide rate → stateless keyed-BLAKE3 cookie under load →
//!   per-source rate) and connection admission (slot → handshake budget
//!   → per-source rate → class/global storm), refundable resources first.
//! - [`scheduler`] — Ingress → admission (trust-clamped priority, mode
//!   shedding) → aging priority queues → all-or-nothing permit
//!   acquisition → worker → metrics feedback, split into Network / CPU /
//!   Storage shards with independent locks.
//!
//! ## Round 10 — §80, 105-110, 137-140, 165-168 (observability & restart)
//!
//! - [`snapshot`] — `ResourceSnapshot`; reported pressure is never lower
//!   than what the numbers themselves imply.
//! - [`metrics`] — fixed-size, lock-free metrics keyed only by trust
//!   class / priority / resource kind / a bounded extension label, so
//!   series count is a compile-time constant (no raw peer ID can become a
//!   label); log2 wait histograms; opt-in local-only per-peer diagnostics
//!   (no `Serialize`); an on/off leak tracker.
//! - [`feedback`] — `UserMessage` is a fieldless enum (a user can't be
//!   shown a semaphore count) with stable l10n keys; developer lines in
//!   the spec's `Memory: 72/128 MiB` format.
//! - [`events`] — batched, coalescing `ResourceEvent` hub (flaps inside a
//!   window emit nothing, storms become one event with a repeat count),
//!   and a journal filter that records only deferred-for-storage /
//!   paused-for-pressure, once per episode.
//! - [`restart`] — pure recovery planning keyed on durable operation ids
//!   (the per-process `ReservationId` restarts at 1 and must never be
//!   persisted): restore / release expired / release orphans / delete
//!   orphan staging / reset active ops; restoring re-admits against the
//!   *current* quota; persisted settings are the only serialized state.
//!
//! ## Round 9 — §86-89, 121-122, 146-152 (modes & effective limits)
//!
//! - [`effective_limits`] — `min(hard, policy, negotiated, pressure)` with
//!   the binding source reported; pressure is a purely local throttle on
//!   top of the negotiated value (8 → 2 under Critical, back to 8 when it
//!   eases, no renegotiation unless the peer depends on the number).
//! - [`cpu_thermal`] — CPU pressure from queue depth / latency / encoder /
//!   thermal; hash-parallelism, AV1 complexity, compression and indexing
//!   responses; thermal response that can never disable critical
//!   communication; three energy budgets (background work, radio
//!   activation, CPU-heavy) whose retuning keeps remaining tokens instead
//!   of refilling.
//! - [`modes`] — foreground / background / battery saver / emergency /
//!   call active / server overload / headless relay as composable flags.
//!   Boosts, then restrictions, then clamp-to-hard; checked over all 128
//!   flag combinations (no ceiling breached, reserves and control capacity
//!   never reduced, essential work never throttled to zero).
//!
//! ## Round 8 — §68, 70-77, 82, 84-85, 127-128, 131 (subsystem backpressure)
//!
//! - [`routing_limits`] — caps on concurrent connects / discovery jobs /
//!   retry timers, one in-flight connect per target; a coalescing
//!   discovery table (one row per noisy peer, one change per drain).
//! - [`negotiation_limits`] — offer-shape limits checked before a session
//!   is allocated; per-peer and global negotiation concurrency.
//! - [`event_log_pressure`] — bounded writer queue that hands rejected
//!   items back, projection-lag states with hysteresis (Critical slows
//!   writes and sheds secondary work, a hard bound refuses writes), and
//!   outbox accounting that rejects bulk first and keeps a critical reserve.
//! - [`fanout`] — one bounded per-consumer queue for daemon IPC, FFI and
//!   plugins: critical never silently lost, coalesce by key, droppable
//!   first out, and *resync-from-snapshot* when even critical overflows;
//!   callback rate limiter; plugin mailbox with handler timeouts/strikes.
//! - [`verification`] — verification CPU priced in cost units: unknown
//!   senders share one class-wide bucket and a capped slice of the queue
//!   (rotating identities buys nothing); known peers get per-peer buckets;
//!   half-open / authenticated / active handshake pools with promotion
//!   that never leaks or double-counts a slot.
//! - [`os_memory`] — Android trim levels, iOS warnings and budget
//!   exhaustion merged into one `PressureState`, with hold-then-decay for
//!   edge-triggered platform signals. Platform code reports raw facts only.
//!
//! Defect hardening: `BoundedCounter::release` now saturates (loud in
//! debug builds) instead of wrapping to ~`u64::MAX`.
//!
//! ## Round 7 — §28, 98, 144-145, 153-155, 158-164 (concurrency & isolation)
//!
//! - [`accounting`] — generation-tagged account tree: a charge hits the
//!   leaf and every ancestor atomically, and a refusal names the exact
//!   node ("files/bob"); [`accounting::TenantPools`] keeps each tenant's
//!   critical reserve exclusive while normal work shares the remainder.
//! - [`fair_semaphore`] — strict-FIFO semaphore; a task that releases
//!   cannot re-acquire past waiters; dropped waiters leave the line.
//! - [`preempt`] — preemption only of holds that are `Preemptible` *and*
//!   can pause at a boundary (the unsafe combination is rejected at
//!   registration), via a request→boundary→resume handshake.
//! - [`cancel`] — hierarchical cancel tokens and `OperationScope`
//!   (cancel first, release holds LIFO), runtime-agnostic.
//! - [`tasks`] — bounded task registry with RAII guards (§161-162).
//! - [`supervisor`] — pure restart state machine: bounded exponential
//!   backoff, crash-loop → Degraded, single probe per cooldown.
//! - [`sharded`] — cache-line-padded counters for hot statistics only;
//!   limits stay on exact CAS counters.
//!
//! ## Round 6 — §5-7, 9-16, 141-143 (the budget foundation)
//!
//! - [`policy_layers`] — layered limits that can only tighten under a
//!   hard ceiling (order-independent), hard-limit set, and soft limits
//!   that map depth-into-the-band to an ordered adaptation ladder before
//!   outright rejection.
//! - [`budget`] — one global memory budget, per-subsystem guarantees,
//!   capped *revocable* borrowing; a lender refused inside its guarantee
//!   is told which borrowers to claw back; borrowing can be switched off
//!   under pressure.
//! - [`profile`] — Embedded / MobileLow / MobileNormal / Desktop / Server
//!   defaults (MobileNormal reproduces §10's 256 MiB split), the full
//!   `ResourceConfig`, and a validator that reports every inconsistency
//!   at once (§142's three examples are tests).
//! - [`dynamic_config`] — atomic runtime updates: validate the whole
//!   candidate, commit or change nothing; tightening below current usage
//!   yields a drain report instead of killing work.
//!
//! ## Round 5 — §42-43, 62-67, 78-79, 111-112, 132-136
//!
//! - [`connections`] — RAII session limiter (global + per-peer; the
//!   per-peer map is bounded by the global cap) and per-class FD limiter
//!   under a total ceiling.
//! - [`slots`] — transfer slots with an interactive-only reserve, a
//!   read-gate over the downstream stages, a bounded read-ahead window.
//! - [`inbound`] — per-trust message size / rate / pending gate (cheap
//!   checks first, rejected messages cost no token); outbound
//!   persist-don't-lose rule.
//! - [`dtn_intake`] — relay intake order (peer quota → storage pressure →
//!   relay quota/eviction) and the per-encounter byte/bundle budget.
//! - [`cost`] — pre-work cost admission, reserve/actual true-up, and a
//!   per-owner estimate-accuracy ledger (never a discount).
//! - [`wait_policy`] — wait vs fail-with-fallback vs defer-durably.
//! - [`abuse`] — decaying strike ladder (throttle → quarantine →
//!   disconnect → block) and trust-clamped priority authorization (§133-134).
//! - [`op_holds`] — durable vs per-attempt holds across retries and path
//!   switches; release-before-reacquire.
//!
//! ## Round 4 — backpressure core: §58-61, 69, 81, 83, 87, 91-95, 96-97, 99-104, 124-126, 129, 169-171
//!
//! - [`pressure`] — worst-of pressure signals → [`pressure::OverloadMode`]
//!   (Normal / Shedding / Overload / CriticalOnly) with immediate
//!   escalation and stepwise, dwell-gated de-escalation (no flapping);
//!   [`pressure::shed`] is a priority-cutoff load shedder that defers
//!   durable work and drops ephemeral work (§22).
//! - [`coalesce`] — latest-value slot, bounded keyed coalescer (typing,
//!   presence), drop-oldest/stale realtime ring.
//! - [`aging`] — priority queue whose waiting items climb tiers (never
//!   above a floor), fixing the strict-priority starvation of
//!   [`queue::BoundedPriorityQueue`] (§56, §99-100); plus a starvation
//!   report.
//! - [`reservation`] — all-or-nothing multi-resource acquisition in one
//!   canonical order with rollback, RAII bundles (cancel/panic safe),
//!   deadlines.
//! - [`storm`] — per-class connection-rate buckets so an unknown-peer
//!   flood can't consume trusted capacity.
//! - [`safety`] — checked length conversion, decompression bomb guard,
//!   RAII depth limiter.
//! - [`error`] — typed [`error::ResourceError`] with a retryability verdict.
//!
//! Defect fix: [`token_bucket::TokenBucket`] moved its refill mark
//! backwards on a regressing clock, so the next normal reading granted
//! a burst of free tokens; backwards readings are now ignored.
//!
//! ## Earlier — weighted max-min bandwidth fairness (§54-56)
//!
//! - [`bandwidth_fairness`] — §54 "Bandwidth Fairness", §55 "Weighted
//!   Fair Queueing", §56 "Strict Priority Risks":
//!   [`bandwidth_fairness::WfqWeights`] (§55's own literal worked
//!   weight table), [`bandwidth_fairness::CriticalPreemption`] (§57's
//!   bound, sized as a fraction of link capacity, this module's own
//!   reasoned choice since the spec requires *a* bound without stating
//!   one), and [`bandwidth_fairness::allocate_bandwidth`] — a real
//!   weighted max-min fair-share allocator, not a naive single-pass
//!   proportional split, specifically because §56 warns that a naive
//!   split can still leave a low-weight tier starved when a
//!   higher-weight tier under-claims its share; tested directly
//!   against that exact starvation scenario, plus an
//!   allocation-never-exceeds-capacity invariant checked across
//!   several demand shapes. Explicitly distinct from
//!   `siar-protocol-ext`'s `FairScheduler` — dequeue *order* for
//!   discrete items vs. dividing a continuous *byte-rate budget* —
//!   see that module's own doc comment for why neither is built on
//!   the other.
//!
//! ## Earlier this pass — storage watermarks, critical reserve, reservations (§46-50)
//!
//! - [`storage`] — §46 "Storage Watermarks", §47 "Storage Pressure
//!   Actions" (partial), §48 "Critical Storage Reserve", §49 "Storage
//!   Reservation", §50 "Storage Reservation Record":
//!   [`storage::PressureState`] (§81's own enum, reused rather than a
//!   parallel storage-only one, since §46's "Full" label and §81's
//!   `Exhausted` are the same concept under two names),
//!   [`storage::StorageWatermarks`] with a real `classify()` using
//!   §46's own worked thresholds (70/85/95%) as the literal default —
//!   unusual for this crate, since the spec gives concrete numbers
//!   here rather than leaving them unstated,
//!   [`storage::CriticalStorageReserve`] enforcing §48's exact rule
//!   ("bulk files must not consume this reserve"), and
//!   [`storage::StorageReservations`] — a real reservation lifecycle
//!   (reserve/commit/cancel/expire) implementing §49's "reservation
//!   itself must expire if transfer never starts," tested directly:
//!   a reservation blocks a competing request while live, then
//!   correctly frees its bytes once its TTL lapses.
//!
//! ## Earlier this pass — CPU worker semaphores (§33-37)
//!
//! - [`cpu_pool`] — §33 "CPU Budgeting", §34 "CPU Work Classes", §35
//!   "Blocking Work Pool", §36 "Hashing Concurrency", §37 "AV1
//!   Software Encoding": [`cpu_pool::CpuWorkClass`] (verbatim),
//!   [`cpu_pool::CpuWorkPool`]/[`cpu_pool::CpuJobPermit`] — §33's
//!   "worker semaphore" built by reusing `permits.rs`'s own
//!   `BoundedCounter` (now widened to `pub(crate)`) rather than
//!   hand-rolling a second bounded-counting primitive — and
//!   [`cpu_pool::CpuWorkPools`], one independent pool per work class
//!   (§34's "separate semaphores/pools when needed" made structural,
//!   the same reasoning `queue::BoundedPriorityQueue` already applied
//!   to priority tiers). §36/§37 are deliberately not separate types:
//!   the spec gives neither any field beyond "acquire before doing
//!   this work, degrade/queue if unavailable," which
//!   `CpuWorkPools::acquire` already provides — see that module's own
//!   doc comment.
//!
//! ## Earlier this pass — RAII memory/stream permits (§38-41)
//!
//! - [`permits`] — §38 "Memory Pool", §39 "Buffer Ownership", §40
//!   "Memory Permit", §41 "Stream Permit": [`permits::BufferPool`]/
//!   [`permits::MemoryPermit`] and [`permits::StreamLimiter`]/
//!   [`permits::StreamPermit`], real RAII admission — capacity is
//!   returned automatically on `Drop`, never through a manual
//!   `release()` call (§39/§40's own explicit instruction), and the
//!   underlying atomic reservation is a real compare-exchange loop
//!   verified under actual thread contention, not just sequential
//!   single-threaded calls, so two racing acquires can't both slip
//!   past the cap. §41's "same pattern" is implemented as literal
//!   code reuse (`BoundedCounter`) rather than two parallel
//!   hand-copied admission implementations that could drift apart.
//!
//! ## Earlier this pass — per-extension quotas (§31-32)
//!
//! - [`extension_quota`] — §31 "Per-Extension Quotas", §32 "Extension
//!   Admission": [`extension_quota::ExtensionResourceLimits`]
//!   (verbatim field list), [`extension_quota::ExtensionResourceLimits::tightened_by`]
//!   implementing §32's own closing line ("Runtime can tighten
//!   these") as a real per-field minimum rather than leaving it
//!   unenforced, and [`extension_quota::ExtensionUsageCounters`] with a
//!   real, tested `try_charge` mirroring `peer_quota.rs`'s shape
//!   (deliberately a separate module, not a shared generic — see that
//!   module's own doc comment for why the two limit structs' fields
//!   don't actually line up). §31 names a fifth budget category ("CPU
//!   work budget") that §32's own concrete struct never turns into a
//!   field — consistent with, not an oversight next to, `types::
//!   ResourceBudget`'s own already-documented absence of a CPU field.
//!
//! ## Earlier this pass — per-peer trust-aware quotas (§29-30)
//!
//! - [`peer_quota`] — §29 "Per-Peer Quotas", §30 "Trust-Aware Quotas":
//!   [`peer_quota::TrustClass`] (verbatim, in the spec's own low-to-
//!   high order), [`peer_quota::PeerQuota`] (verbatim field list) with
//!   a real [`peer_quota::PeerQuota::for_trust_class`] scaling table —
//!   every number is this module's own reasoned choice, since the
//!   spec gives none, but §30's "never make trusted peers unlimited"
//!   is checked directly by a test, not just assumed — and
//!   [`peer_quota::PeerUsageCounters`] with a real, tested
//!   [`peer_quota::PeerUsageCounters::try_charge`] that enforces all
//!   five non-rate dimensions independently and reuses this crate's
//!   own §21-22 durable/ephemeral split. `max_requests_per_sec` is
//!   deliberately not a counter on `PeerUsageCounters` — it's a
//!   [`crate::token_bucket::TokenBucket`] via
//!   [`peer_quota::PeerQuota::request_rate_bucket`], the same
//!   "a rate isn't a one-shot pool" reasoning `admission::admit`
//!   already established for `bandwidth_class`.
//!
//! Deliberately not attempted: §28's full hierarchical ledger
//! (runtime → extension → peer → operation) — its third level needs
//! an `OperationId`-shaped type nothing in this workspace defines yet,
//! left for a future pass rather than guessed at now.
//!
//! ## Earlier this pass — bounded priority queue (§17-21)
//!
//! - [`queue`] — §17 "Bounded Queue Principle", §18 "Queue
//!   Categories", §19 "Priority Queues", §20 "Queue Capacity by
//!   Priority", §21 "Backpressure Semantics":
//!   [`queue::BoundedPriorityQueue`], six independently-capacitated
//!   tiers (one per [`admission::WorkPriority`] variant) so §20's "Bulk
//!   must not consume capacity needed for Critical/Control" is
//!   structural, not a policy a caller could forget — enqueue reuses
//!   [`admission::AdmissionResult`]/[`admission::DeferredReason`]/
//!   [`admission::DropReason`] directly rather than a parallel result
//!   type, and the durable/ephemeral split from §22 (defer vs drop) is
//!   real, not just named. Deliberately not a fairness/dispatch
//!   scheduler — see that module's own doc comment for why that stays
//!   `siar-protocol-ext`'s `FairScheduler`'s job
//!
//! Also fixed this pass: [`admission::AdmissionResult`]'s first
//! variant was named `Admitted` in the previous round without the
//! full §23 text in hand — it's `Accepted` in the actual spec text,
//! corrected here (and in every test/caller in `admission.rs`).
//!
//! ## Earlier this pass — admission control (§22-27)
//!
//! - [`admission`] — §22 "Bounded Queue Principle", §23 "Backpressure
//!   Semantics", §24 "Admission Control", §25 "Admission Controller",
//!   §26 "Resource Request", §27 "Resource Owner":
//!   [`admission::ResourceOwner`] (verbatim), [`admission::ResourceRequest`]
//!   (verbatim), [`admission::WorkPriority`] (a real integration — a
//!   type alias onto `siar-protocol-ext`'s `TrafficPriority` rather
//!   than a duplicate enum, since §19 explicitly frames this type as
//!   aligning with the other parts' priority schemes), and a real
//!   [`admission::admit`] function implementing the decision §25's
//!   trait only sketches — grounded three-reason [`admission::AdmissionResult`]
//!   (§23 names the three variants but defines none of the reason
//!   enums; each one here is grounded in specific spec text, cited in
//!   its own doc comment, not invented from nothing)
//!
//! ## Earlier this pass — core types + token bucket
//! - [`types`] — §3 "Resource Dimensions", §4 "Resource Classes", §8
//!   "Resource Budget": [`types::ResourceKind`] (all ten dimensions
//!   §3 lists, so pressure is never collapsed into one generic "busy"
//!   flag), [`types::ResourceBudget`] (the concrete-quantity subset of
//!   those dimensions §8 gives a number shape for)
//! - [`token_bucket`] — §52 "Traffic Shaping", §53 "Token Bucket":
//!   [`token_bucket::TokenBucket`], a real (not conceptual-sketch)
//!   rate limiter — exact integer refill arithmetic (no `f64` drift),
//!   all-or-nothing consumption, and a refill clock that keeps
//!   advancing even on a rejected request. Usable directly for any of
//!   §53's three named purposes (per-peer bytes/sec, per-extension
//!   bandwidth, unknown-peer intake)
//!
//! ## Deliberately not attempted this pass
//!
//! Everything past these ten modules: resource policy layering
//! (§5-7), the global/sub-budget hierarchy and borrowing (§9-11),
//! device resource profiles (§12-16), §18's other queue categories
//! beyond priority tiering (DTN/notification/projection/IPC as
//! *separate named queues*), §28's full hierarchical ledger, the full
//! §80 `ResourceSnapshot`/`AdmissionController` trait, connection
//! limits and file descriptor limits (§42-43), §44-45's storage class
//! hierarchy, the rest of §47's pressure *actions*, §57/§61's fuller
//! emergency preemption/drop policy beyond `CriticalPreemption`'s
//! bandwidth-specific bound, per-subsystem backpressure (messaging/
//! files/DTN/routing/discovery/capability/event-log/outbox/IPC/FFI/
//! plugins, §62-77), and cost-estimate admission (§78 onward). None
//! of these are guessed at here.

pub mod abuse;
pub mod accounting;
pub mod admission;
pub mod admission_pipeline;
pub mod aging;
pub mod bandwidth_fairness;
pub mod budget;
pub mod cancel;
pub mod coalesce;
pub mod connections;
pub mod cost;
pub mod cpu_pool;
pub mod cpu_thermal;
pub mod dtn_intake;
pub mod dynamic_config;
pub mod effective_limits;
pub mod error;
pub mod event_log_pressure;
pub mod events;
pub mod extension_quota;
pub mod fair_semaphore;
pub mod fanout;
pub mod feature_limits;
pub mod feedback;
pub mod frame;
pub mod inbound;
pub mod metrics;
pub mod modes;
pub mod negotiation_limits;
pub mod op_holds;
pub mod os_memory;
pub mod peer_quota;
pub mod permits;
pub mod policy_layers;
pub mod preempt;
pub mod pressure;
pub mod profile;
pub mod queue;
pub mod reservation;
pub mod restart;
pub mod routing_limits;
pub mod safety;
pub mod scheduler;
pub mod sharded;
pub mod slots;
pub mod snapshot;
pub mod storage;
pub mod storage_classes;
pub mod storm;
pub mod supervisor;
pub mod tasks;
pub mod token_bucket;
pub mod types;
pub mod verification;
pub mod wait_policy;

pub use admission::{
    admit, AdmissionResult, BandwidthClass, DeferredReason, DropReason, RejectReason,
    ResourceOwner, ResourceRequest, WorkPriority,
};
pub use bandwidth_fairness::{allocate_bandwidth, CriticalPreemption, WfqWeights};
pub use cpu_pool::{CpuJobPermit, CpuWorkCapacities, CpuWorkClass, CpuWorkPool, CpuWorkPools};
pub use extension_quota::{ExtensionResourceLimits, ExtensionUsageCounters, ExtensionUsageDelta};
pub use peer_quota::{PeerQuota, PeerUsageCounters, PeerUsageDelta, TrustClass};
pub use permits::{BufferPool, MemoryPermit, StreamLimiter, StreamPermit};
pub use queue::{BoundedPriorityQueue, QueueCapacities};
pub use storage::{
    CriticalStorageReserve, PressureState, ReservationId, StorageReservation, StorageReservations,
    StorageWatermarks,
};
pub use token_bucket::TokenBucket;
pub use types::{ResourceBudget, ResourceKind};
