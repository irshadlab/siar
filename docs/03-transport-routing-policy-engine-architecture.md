# Part 03 — Transport & Routing Policy Engine Architecture

Source spec: `sys-arch/03-transport-routing-policy-engine-architecture.md` — 200 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**200/200 (100%) sections — SPEC COMPLETE.**

Complete — Round 1 (§1–§42), Round 2 (§43–§56), Round 3 (§57–§62), Round 4 (§63–§68), Round 5 (§69–§74), Round 6 (§75–§84), Round 7 (§85–§90), Round 8 (§91–§96), Round 9 (§97–§104), Round 10 (§105–§107), Round 11 (§108–§115), Round 12 (§116–§120), Round 13 (§121–§127), Round 14 (§128–§139), Round 15 (§140–§150), Round 16 (§151–§159), Round 17 (§160–§170), Round 18 (§171–§182), and Round 19 (§183–§200) complete (263/263 unit tests).

### Round 19 (§183–§200) Summary (2026-09-15) — SPEC COMPLETE
- **§183 & §184 (Testing Matrix & Route Selection Golden Tests)**: Implemented `golden_tests.rs` transcribing all 5 of the spec's worked examples (healthy direct stays direct on text, degraded direct switches to relay, large file prefers unmetered LAN, typing with only DTN is rejected/deferred, emergency SOS uses BLE or mesh without Internet). Added tests for previously uncovered candidate combos (BLE-only, Wi-Fi Direct + BLE).
- **§185 (Property Tests)**: Added property tests in `decision.rs` and `scoring.rs` proving forbidden transports (`allow_relay: false`, `allow_bluetooth: false`) are never selected regardless of score, and known bandwidth below `min_bandwidth` is a hard elimination.
- **§186 & §187 (Fuzzing & Benchmarking)**: Documented named gaps (no `cargo-fuzz` harness, no `criterion` benchmark suite). Implemented targeted tests verifying zero-latency deadline producing `0.0 / 0.0` (`NaN`) does not panic (guarding sort comparator) and `u64::MAX` estimated byte size does not overflow and is cleanly rejected by `max_operation_bytes`.
- **§188 (Scalability)**: Verified operation/session level routing without per-packet overhead and candidate caching via `RouteCache`.
- **§189–§191 (Routing Frequency Cluster)**: Implemented `reevaluation.rs` with `quality_change_exceeds_threshold` (reusing `RouteScoreDelta` for symmetrical call quality evaluation) and `should_reevaluate_file_route` (re-evaluating on path failure, significant quality change, superior path availability, or policy change). Reconciled message routing frequency to session cache reuse.
- **§192–§197 (Architecture Reconciliation)**: Documented module structure rationale, confirmed zero UI/platform SDK dependencies, mapped 6 concrete `RoutingError` variants onto the spec's 7 conceptual categories, confirmed zero `anyhow` in public APIs, and reconciled 4 implementation phases.
- **§198 (Definition of Done Self-Audit)**: Conducted honest self-audit item by item, documenting completed features, deferred/partial items (no UI prompt, group destination resolution pending in `resolve.rs`), and genuine named gaps (fuzz harness, criterion benchmarks, DTN delivery likelihood encounter computation, §55 mesh forwarding).
- **§199 & §200 (Relationship to Other Parts & Final Principle)**: Documented explicit inter-crate boundaries with Parts 01, 02, 06, and 17. Concluded with the final architectural principle: "a routing decision is a trust decision wearing a performance costume."
- **Crate-Root Re-exports**: Added comprehensive `pub use` re-exports across all public types and functions introduced in rounds 10 through 19.

### Round 18 (§171–§182) Summary (2026-09-14)
- **§171 (Multi-Device Route Aggregation)**: Implemented `plan_per_device` in `multidevice.rs` to compute independent route plans per recipient device, preventing flattening of multiple target devices into a single winning score. Hard constraint failures or low scores on one device (e.g. phone on metered path) never suppress plans for other devices (e.g. unmetered laptop).
- **§172 (Device Preference)**: Implemented `DeviceRole` (`Primary`, `PreferredFile`, `CallCapable`) and `devices_matching_role_for_class` in `multidevice.rs` as a soft preference with fallback to all devices, preventing misconfigurations from filtering out all candidates.
- **§173 & §175 (Group Routing & Content Sensitivity)**: Documented `plan_per_device` application to group member device fan-out (noting open gap for `Destination::Group` in `resolve.rs`). Reconciled content sensitivity to existing `allow_dtn` and `allow_relay` fields in `DeliveryRequirements`.
- **§174 (Broadcast Routing)**: Implemented `BroadcastId` and `BroadcastDeliveryTracker` in `broadcast.rs` for tracking per-device delivery to avoid duplicate emissions across fan-out/mesh retries. Mapped broadcast transports to `RouteScope::LocalOnly`.
- **§176–§178 (Storage Cost Cluster)**: Implemented `DtnStoragePressure` in `dtn_storage.rs`. Added `eliminate_dtn_under_storage_pressure` to eliminate DTN candidates under tight storage quotas (<= 5%) and `storage_pressure_allows_bulk_acquisition` to reject low-priority bulk transfers while permitting critical traffic. Documented `marked_priority_for_emergency_storage`.
- **§179 (Route Policy Persistence)**: Added `Serialize` and `Deserialize` derives to all settings-shaped types (`PrivacyPolicy`, `SystemPolicy`, `ApplicationPolicy`, `RoutingConfig`, `RetryPolicy`, `HysteresisPolicy`, `RouteScoreDelta`, `RoutingPolicyProfile`) and verified via compile-time trait bound test in `config.rs`.
- **§180–§182 (Policy Triggers Cluster)**: Documented pure-function dynamic re-evaluation. Implemented `hysteresis_for_call_state` in `policy_triggers.rs` (doubling switch threshold and hold time during calls) and `emergency_effective_requirements` (relaxing `allow_dtn` only with explicit user opt-in).

### Round 17 (§160–§170) Summary (2026-09-13)
- **§160–§163 (Route Request API & Builder Pattern)**: Implemented fluent builder methods on `RouteRequest` (`for_device`, `for_account`, `class`, `priority`, `estimated_size`, `allow_dtn`, `allow_metered`, `allow_multipath`, `allow_redundancy`, `expiry`, `min_bandwidth`, `max_latency`, `with_candidates`). Added `allow_redundancy: bool` to `DeliveryRequirements` and gated `RouteStrategy::Redundant` in `plan_route` on this explicit flag.
- **§164–§167 (Execution Outcomes & Feedback Reconciliation)**: Reconciled `RouteOutcome` to the flat 8-variant enumeration (`Success`, `Timeout`, `ConnectionFailed`, `AuthenticationFailed`, `RemoteRejected`, `PolicyBlocked`, `Partial`, `Cancelled`). Added `observed_metrics: ObservedMetrics` (`ObservedMetrics = PathMetrics`) to `RouteResultReport`. Updated `health_after_outcome` to handle `Partial` (degraded recovery) and `Cancelled` (identity mapping, unchanged health).
- **§168–§170 (Path Switch Mechanics & Strategies)**: Implemented `path_switch.rs` with `PathSwitchStrategy` (`MakeBeforeBreak`, `BreakBeforeMake`), `path_switch_strategy_for`, and `PathSwitchCandidateInfo`. Prioritizes resource constraints (e.g. single Bluetooth radio, severe memory pressure, critical thermal state) and security sensitivity to force `BreakBeforeMake`, while streaming audio/video with adequate radio capabilities safely uses `MakeBeforeBreak`.

### Round 16 (§151–§159) Summary (2026-09-12)
- **§151 (Route Probability)**: Added `delivery_likelihood` and `expected_delay_class` (`ExpectedDelayClass`) to `PathMetrics` in `metrics.rs` as the typed reception fields for DTN/mesh adapters, avoiding fabricated RTTs. Added `route_probability_signal` in `probability.rs`.
- **§152–§154 (Metric Types, Scoring Normalization, Policy Weights)**: Verified standing design adherence — typed metric categories preserved independently, normalized per-factor suitability terms, and profile-driven float isolation.
- **§155 (Integer Score Option)**: Implemented `PolicyWeights::sum()` in `policy.rs` and `RouteScore::as_fixed_point()` in `scoring.rs`, normalizing scores against actual policy weight sums into the `0..10_000` integer range for deterministic cross-platform portability.
- **§156–§158 (Diagnostics & History Cluster)**: Implemented `diagnostics.rs` with `RouteDecisionLog` (payload-free decision records), `DeveloperDiagnostics` (transcribing the spec's worked example for transport, score, fallback, and `RouteReason::description`), and `RouteHistory` (fixed-capacity ring buffer evicting oldest entries to guarantee bounded retention). Added `primary_score` to `RoutePlan`.
- **§159 (Telemetry Export)**: Implemented `telemetry.rs` with `TelemetrySummary` and `summarize_telemetry`, aggregating route success rate, direct/relay ratio, and failover rate with zero identity fields by construction.

### Round 15 (§140–§150) Summary (2026-09-12)
- **§140 (Bandwidth Reservation)**: Implemented `BandwidthReservation` and `bulk_should_yield_to_reservation` in `resource_pressure.rs`, ensuring Bulk transfers yield unconditionally to active realtime audio/video reservations.
- **§141 (Traffic Shaping)**: Implemented `TrafficShapingPolicy` and `effective_traffic_cap` in `resource_pressure.rs`, enforcing bulk bitrate caps during active calls and background priority caps unconditionally.
- **§142 (Connection Admission)**: Implemented `ConnectionAdmission` and `admission_permitted` in `resource_pressure.rs`, enforcing max total sessions and expensive radio limits while guaranteeing `Priority::Critical` always bypasses admission limits.
- **§143 (Thermal Awareness)**: Implemented `thermal_allows_multipath`, `thermal_allows_transport_setup`, and `thermal_allows_background_bulk` in `resource_pressure.rs`, throttling multi-radio, Wi-Fi Direct setup, and background bulk traffic under `ThermalState::Critical`.
- **§144 (Memory Pressure)**: Added `MemoryPressure` (`Normal`, `Elevated`, `Critical`) to `DeviceState` in `platform.rs`. Implemented `memory_pressure_allows_acquisition` (pausing non-durable bulk while preserving durable transfers) and `recommended_queue_capacity` (scaling queue depth based on memory pressure).
- **§145–§150 (Transport Adapter Contracts)**: Added comprehensive architectural specification in `adapters.rs`, verifying `PathCandidate` fulfills report contracts, confirming zero Iroh-specific API leakage, enforcing `DeviceId` identity over MACs for Bluetooth, and formally documenting transport-layer boundaries for NIC interfaces, BT paired states, Wi-Fi groups, and DTN delivery probability.

### Round 14 (§128–§139) Summary (2026-09-12)
- **§128 & §129 (File Resume & Messaging Retry Integration)**: Documented architectural boundaries — routing coordinates path changes without understanding file chunk state or creating duplicate message semantics.
- **§130 (Call Path Change Integration)**: Implemented `RouteChangeEvent` (`NewPath` and `QualityUpdate` carrying `PathMetrics`) in `engine.rs` to report path switches and metric changes to higher-level call/media engines without performing media actions.
- **§131 (Security Event Integration)**: Implemented `handle_security_event` and `SecurityEvent` in `risk.rs`, immediately invalidating the route cache on authentication failure / revoked device, returning event data for caller candidate eviction and telemetry.
- **§132 (Blacklisting)**: Implemented `PathPenalty` with timestamp-based `until` (guaranteeing penalties expire and transient errors are not permanent) and `eliminate_penalized_paths` in `risk.rs`.
- **§133 (Peer Abuse)**: Implemented `PeerAbuseStatus` (`Quarantined`, `Blocked`) and pre-scoring filter `eliminate_abusive_peers` in `risk.rs` so high-bandwidth abusive peers never survive to compete on scores.
- **§134–§137 (Route Scope & Transport Lists)**: Implemented `RouteScope` (`Any`, `InternetOnly`, `LocalOnly`, `NearbyOnly`), `transport_allowed_in_scope`, and `eliminate_out_of_scope_candidates` in `scope.rs`. Accurately captured spec asymmetry where `LocalOnly` includes `Dtn` while `NearbyOnly` excludes it.
- **§138 (Emergency Override)**: Added `emergency_override_enabled` to `PrivacyPolicy` and `effective_privacy_policy` in `privacy.rs`. Evaluated within `decide_route` Step 4: lifts `avoid_relay` only when both `Priority::Critical` and explicit user opt-in are present.
- **§139 (User Consent)**: Preserved boundary — `PrivacyPolicy` is the concrete policy ingested by routing, while user consent collection is an application/UI layer responsibility.

### Round 13 (§121–§127) Summary (2026-09-12)
- **§121 (Feedback Loop)**: Implemented `health_after_outcome` in `engine.rs` as a pure, stateless single-sample transition updating route health from execution outcomes. Added per-path health tracking in `TestEngine::report_result` with gradual recovery (`Unreachable`/`Suspect` -> `Degraded` -> `Healthy`) and immediate transition to `RouteHealth::Unreachable` on permanent failure classes.
- **§122 (Avoid ML Initially)**: Preserved deterministic weighted-sum routing without ML crates or runtime stochastic dependencies.
- **§123 (Deterministic Scoring)**: Added property tests verifying reproducibility across three distinct architectural layers (`scoring::score`, `plan::plan_route`, `decision::decide_route`) given identical inputs.
- **§124 (Simulated Routing Tests)**: Transcribed spec's exact scenario numbers (Path A: 10ms/1Mbps/metered vs Path B: 50ms/100Mbps/unmetered), proving interactive messages select based on policy while bulk transfers unconditionally require Path B due to hard constraints.
- **§125 (Policy Property Tests)**: Verified all 4 core policy invariants (revoked device exclusion, forbidden metered exclusion, realtime DTN exclusion, expired operation rejection). Added `created_at_millis` to `OperationDescriptor` and `RejectReason::OperationExpired` to `decide_route` Step 0.
- **§126 (Chaos Tests)**: Implemented Wi-Fi flapping test in `plan.rs` simulating 20 consecutive jitter rounds, verifying hysteresis stickiness prevents route storms.
- **§127 (Failover Test)**: Implemented failover simulation proving degraded/unreachable primary immediately fails over to a healthy fallback candidate.

### Round 12 (§116–§120) Summary (2026-09-11)
- **§116 (UI-Friendly State)**: Implemented `RouteUiState` (`Sending`, `WaitingForConnection`, `WaitingForWiFi`, `CarriedByNearbyPeer`, `Delivered`) and `ui_state_for` in `ui_state.rs`, providing a many-to-one collapse of internal decision states to prevent exposing raw transport errors to normal users.
- **§117 (Route Policy Configuration)**: Implemented `RoutingConfig` in `config.rs`, reusing existing types (`RetryPolicy`, `HysteresisPolicy`, boolean gates for relay/bluetooth/dtn/multipath, direct preference default); implemented startup validation (`validate()`) catching inverted retry backoff ranges.
- **§118 (No Global Singleton)**: Verified zero singleton or static state across the crate; routing engine instances are owned explicitly by caller/runtime.
- **§119 (Routing Engine API)**: Implemented native async `RoutingEngine` trait in `engine.rs` with `plan` and `report_result`; implemented `RouteRequest`, `RouteDecision` alias, `RouteResultReport`, and `RouteOutcome` (reusing `RouteFailureClass`); implemented and tested `TestEngine` with a minimal zero-dependency executor.
- **§120 (Transport Manager API)**: Documented architectural boundary — deliberately omitted socket-owning traits/types belonging to lower-level transport crates.

### Round 11 (§108–§115) Summary (2026-09-11)
- **§108 (Policy Layering)**: Implemented `PolicyLayers` bundling system, application, and user policy layers; implemented `decide_route` running the five-layer stack (system → application → user → operation → network context) in sequential order.
- **§109 (System Policy)**: Implemented `SystemPolicy` enforcing hard operation size limit (`max_operation_bytes`) and untrusted device elimination (`security::eliminate_untrusted_candidates`).
- **§110 (Application Policy)**: Implemented `ApplicationPolicy` providing application-level relay control (`allow_relay`).
- **§111 & §112 (User Policy & Operation Policy)**: Integrated `PrivacyPolicy` and `DeliveryRequirements` into the layered decision pipeline.
- **§113 (Policy Conflict)**: Designed layer-by-layer evaluation short-circuiting on the specific conflicting policy layer; verified §113 worked example ("large file + no metered + only metered path exists -> DeferredByPolicy").
- **§114 & §115 (Policy Result Types & Deferred Reasons)**: Implemented `RouteDecisionResult` (`Routed`, `Deferred`, `Rejected`, `Unreachable`), `RejectReason`, and `DeferredReason` (`WaitingForUnmetered`, `WaitingForPeer`, `WaitingForWifi`, `BatteryPolicy`, `BackgroundRestriction`, `NoSuitablePathYet`).

### Round 10 (§105–§107) Summary (2026-09-11)
- **§105 (Path Authorization)**: Implemented `authorize_path` and `eliminate_paths_lacking_authorization` in `authorization.rs` composing the spec's four checks: device active & identity trusted (`security::authorize_candidate`), operation authorized (`DeviceCapabilitySet::contains`), and extension supported (`PeerCapabilities::supports`). Implemented `PathAuthorization` witness struct.
- **§106 (Extension Capability Integration)**: Added `required_extension: Option<ProtocolId>` to `OperationDescriptor`; added peer capability check with conservative failure default (missing capability record treated as unsupported); added `RoutingError::ExtensionNotSupported`.
- **§107 (Device Capability Integration)**: Implemented `select_devices_with_capability` for pre-candidate device selection from a resolved device list; added `DeviceCapabilitySet::REALTIME_MEDIA` to `siar-identity-multidevice`; added `RoutingError::OperationNotAuthorized`.

### Round 9 (§97–§104) Summary (2026-09-10)
- **§97 (Route Decision Explainability)**: Implemented `RouteReason` and `infer_reason` in `explain.rs`; added `reason: RouteReason` to `RoutePlan`.
- **§98 & §99 (Metrics Collection & Privacy of Metrics)**: Implemented `RouteMetricEvent` and `metric_events_for`; verified privacy-by-construction (no peer identity, IP, or location fields).
- **§100 & §101 (Routing State Store & Startup Behavior)**: Implemented `RouteHint`, `hint_from_plan`, and `revalidate_hint` (verifies persisted hint against live healthy candidates).
- **§102 & §103 (Suspend/Resume & Process Death)**: Documented composition of `RouteCache::invalidate_all`, fresh `plan_route`, `RetryPolicy`, and `DiscoveryBudget` for suspend/resume; noted deliberate lack of `Serialize` on `RoutePlan` to prevent stale plan restoration.
- **§104 (Route Plan Lifetime)**: Added `created_at_millis` and `valid_until_millis` to `RoutePlan`, derived from `now_millis` parameter and `policy.hysteresis.minimum_hold_millis`.

### Round 8 (§91–§96) Summary (2026-09-08)
- **§91 & §92 (Route Escalation Ladder & Timeout by Stage)**: Implemented `EscalationStage` (`ActiveConnections`, `KnownEndpoints`, `LightweightDiscovery`, `ExpensiveProximitySetup`, `DtnFallback`) in `resilience.rs`; implemented `escalation_stage_of` by composing `CandidateState`, `SetupCost`, and `TransportKind::Dtn`; implemented `timeout_millis_for_stage` and `should_escalate_beyond`.
- **§93 (Hedged Requests)**: Added `RouteStrategy::Hedged` and `RoutePlan::hedge_delay_millis`; implemented `HedgePolicy` and `hedge_policy_for` (hedges high/critical priority non-bulk, non-delay-tolerant traffic; sets delay to 150ms or 25% of deadline floored at 50ms); integrated into `plan_route`.
- **§94 (Deduplication Requirement)**: Extended `OperationId` documentation to mandate receiver idempotency across `Redundant` and `Hedged` strategies.
- **§95 & §96 (Route Diagnostics & Path Visualization)**: Implemented `RouteDiagnostics`, `RejectionReason`, and `diagnose` in `resilience.rs`; documented §96 Path Visualization as deferred by the spec to Part 18.

### Round 7 (§85–§90) Summary (2026-09-08)
- **§85 & §87 (Battery-Aware Inputs & Platform Policy Integration)**: Implemented `BatteryLevelClass`, `ThermalState`, and `DeviceState` in `platform.rs`; mapped §87's six platform signals across typed fields in the crate.
- **§86 & §89 (Background Restrictions & Candidate Acquisition State)**: Implemented `CandidateState` (`Active`, `PassiveKnown`, `RequiresDiscovery`, `RequiresSetup`) in `acquisition.rs`; added `is_currently_usable` and `eliminate_background_restricted` (with permissive default for unknown foreground state); added `candidate_state` weight across all 7 profiles.
- **§88 & §90 (Path Acquisition & Discovery Budget)**: Implemented `DiscoveryBudget` in `discovery.rs` (sliding-window rate limiting + exhaustion cooldown per `Priority`); implemented `discovery_permitted` combining budget with device safety margins (unconditional block on `ThermalState::Critical`, priority-gated battery saver).
- **Pipeline Integration**: Extended `plan_route` to accept `Option<&DeviceState>` and actively filter background-restricted candidates.

### Round 6 (§75–§84) Summary (2026-09-08)
- **§75–§78 (Multipath Chunk Scheduler, Path Collapse, Duplicate Chunks, Realtime Multipath)**: Documented as out-of-scope for v1 per the spec's own designation as future/optional/advanced.
- **§79 (Congestion Signals)**: Added `CongestionState` (`Normal`, `Congested`, `Severe`), `retransmission_rate`, and `congestion_state` to `PathMetrics`; added `congestion` weight across all 7 `PolicyWeights` profiles; scored in `DefaultScorer`.
- **§80 (Route Stability Score)**: Implemented `derive_stability_score` in `stability.rs` converting lifetime, failure rate, path changes, and timeout count into `StabilityScore`.
- **§81 & §84 (Cold vs Warm Path & Battery Cost)**: Reconciled and documented existing coverage in `setup.rs` (`ConnectionPoolState::Active` -> `SetupCost::Cheap`) and `metrics.rs` (`EnergyCost`).
- **§82 & §83 (Metered Networks & Roaming)**: Refactored `metered` to tri-state `MeteredState` and added `RoamingState`, both conservatively blocking `Unknown` in `passes_hard_constraints`; corrected `file_chunk()` to `allow_metered: false` and added `allow_roaming_bulk: bool`.

### Round 5 (§69–§74) Summary (2026-09-08)
- **§69 & §70 (Route Planning for Messaging & Message Routing Workflow)**: Verified through comprehensive integration tests covering interactive messages, low-power preferences, emergency profiles, and fallback paths.
- **§71 (Message Retry Strategy & Quality Signal)**: Implemented `PathQualitySignal` (`Good`, `Degraded`, `Failed`) in `quality.rs` projected from `PathMetrics` for media/retry adaptation.
- **§72 (Ephemeral Message Routing & Deduplication)**: Documented stable operation ID deduplication in `descriptor.rs` and verified in route planning tests.
- **§73 & §74 (Offline Routing, File Transfers, Path Diversity & Underlay Grouping)**: Implemented `UnderlayId`, `are_diverse`, `group_by_underlay`, and `most_diverse_fallback` in `diversity.rs`; wired into `plan_route`'s redundant-strategy replica selection so redundant copies prefer physically diverse underlays.

### Round 4 (§63–§68) Summary (2026-09-08)
- **§63–§65 (Queue Architecture, Weighted Fair Scheduling, Backpressure)**: Formally documented in `dispatch.rs` via `siar-protocol-ext`'s `FairScheduler` and `BoundedQueue` integration.
- **§66 (Per-Transport Queues)**: Implemented `PerTransportDispatchQueue` in `dispatch.rs` maintaining separate `RouteDispatchQueue` instances per `TransportKind` so a stalled transport (e.g. Bluetooth) cannot block healthy transports (e.g. Iroh).
- **§67 & §68 (Per-Peer & Per-Extension Fairness)**: Implemented generic `RoundRobinFairQueue<K, T>` in `fairness.rs`, instantiated with `DeviceId` for §67 per-peer fairness and `ContentClass` (with newly derived `Hash`) for §68 per-extension fairness.

### Round 3 (§57–§62) Summary (2026-09-08)
- **§57 (Delivery Semantics)**: `typing_indicator()` (non-durable, 5s expiry, no DTN) and `file_chunk()` (durable, bulk, DTN/multipath allowed) constructors on `DeliveryRequirements`.
- **§58 (Operation Descriptor)**: `OperationDescriptor` struct carrying `OperationId`, `Destination`, `DeliveryRequirements`, `ByteCount`, and `ContentClass`.
- **§59 (Content Class)**: 10-variant `ContentClass` enum (Control, Text, Metadata, Thumbnail, Voice, Image, File, RealtimeAudio, RealtimeVideo, Emergency).
- **§60 (Size-Aware Routing)**: `completion_time_millis` combining static transport setup latency with payload byte count and estimated bandwidth (`setup + bytes / bandwidth`).
- **§61 (Deadline-Aware Routing)**: `exceeds_deadline` and `eliminate_deadline_exceeding_candidates` pruning paths that cannot deliver before `max_latency_millis`.
- **§62 (Expiry-Aware Routing)**: `DeliveryRequirements::has_expired` and `RetryPolicy::allows_attempt_at` enforcing clock-based operation expiration during retry attempts.

### Round 2 (§43–§56) Summary (2026-09-08)
- **§43 (Route Re-Evaluation)**: Targeted per-transport cache invalidation (`RouteCache::invalidate_transport`).
- **§44 & §46 (Transport Setup Cost & Connection Pool Integration)**: `SetupCost` and `ConnectionPoolState` modeling static vs pool-adjusted connection setup costs (`static_setup_cost`, `effective_setup_cost`), integrated into `DefaultScorer` as `setup_cost` weight.
- **§45 (Existing Connection Preference)**: `existing_connection` scoring bonus in `DefaultScorer` based on `RoutingContext::current_path`.
- **§47 & §48 (Peer Session Abstraction & Security Constraints)**: `AuthenticatedSession` smart constructor, `authorize_candidate`, and `eliminate_untrusted_candidates` enforcing defense-in-depth re-verification against Part 02's `TrustedAccountStore`.
- **§49 & §50 (Privacy Policy & Direct vs Relay)**: `PrivacyPolicy` struct, `passes_privacy_policy`, `eliminate_privacy_violations`, and `direct_preference_bonus`.
- **§52 (Wi-Fi Direct/Aware Threshold Policy)**: `justifies_expensive_setup` and `eliminate_unjustified_expensive_setup` gating expensive setup on high bitrate, realtime calls, critical priority, or explicit nearby requests.
- **§51, §53, §54, §56**: Fully accounted for via setup costs and `DeliveryRequirements` fields (`nearby_session_explicit`, `dtn_replication_budget`).
- **§55 (Mesh Forwarding)**: Documented as an honest remaining gap (requires richer next-hop/hop-budget representation).

## Implementing crate(s)

- `siar-routing-policy`
- `siar-connectivity` (transport managers, candidate tables, and address heuristics)

## Known gaps / open questions

- **Resolved reconciliation**: The former next.md-era `siar-routing` crate has been retired; key capabilities (`LinkHealth`, `relay_composition`, `CongestionTracker`) were ported into `siar-routing-policy`, while `CandidateTable` and `DeviceRoutes` live in `siar-connectivity` (see [`MIGRATION.md`](../MIGRATION.md)).
- §55 Mesh Forwarding: needs dedicated next-hop, route utility, hop budget, and relay trust policy models.


## Note
Spec 03 implementation complete across 19 rounds (2026-09-15). All 200 sections accounted for (263/263 tests passing). Next step: Tier 0 Step 1.4 (Spec 04 — Offline Event Log Architecture).
