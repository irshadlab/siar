#![forbid(unsafe_code)]

//! siar-routing-policy: a first slice of "Part 03 — Transport Routing
//! Policy Engine Architecture" (the third of three architecture
//! documents supplied this pass; Part 01 has its own crate,
//! `siar-protocol-ext`; Part 02 has `siar-identity-multidevice`, which
//! this crate depends on for real — see [`resolve`]). Unlike Parts 01
//! and 02, this workspace had **no** crate built against Part 03's
//! specific spec text before this session — the existing
//! `siar-routing` crate is a related but independently-evolved system
//! built against a different, earlier document ("next.md"), and is
//! left untouched here (see this comment's own closing section for how
//! the two relate).
//!
//! ## Scope: §196 "Initial Production Scope" + §197 Phase 1/3/4, partial
//!
//! Implemented, with real tests exercising actual logic (not just type
//! shapes):
//!
//! - [`types`], [`metrics`], [`requirements`], [`candidate`], [`plan`]
//!   — §197 Phase 1 ("Types and policy"): `DeliveryRequirements`,
//!   `PathCandidate`, `PathMetrics`, `RoutingPolicy`, `RoutePlan`, named
//!   exactly as §5 "Main Abstractions" lists them.
//! - [`scoring`] — §197 Phase 3 ("Scoring"): hard constraints (§25 step
//!   1), a real weighted scorer implementing §24's formula for the
//!   terms this crate can actually compute (see that module's own doc
//!   comment for the two terms — congestion, failure penalty — it
//!   doesn't model).
//! - [`plan::plan_route`] and [`policy`] — §197 Phase 4 ("Failover"):
//!   §25's full four-step evaluation order end to end, including §34/§35
//!   stickiness/hysteresis, plus [`retry`]'s backoff policy and
//!   [`failure`]'s failure classification (§36-39).
//! - [`cache`] — §41 "Route Cache" with §42's invalidation triggers as
//!   callable methods (this crate doesn't listen for the underlying OS/
//!   network events itself), plus §43 "Route Re-Evaluation"'s targeted
//!   per-transport invalidation (`RouteCache::invalidate_transport`),
//!   narrower than §42's already-existing "clear everything."
//! - [`setup`] — §44 "Transport Setup Cost", §46 "Connection Pool
//!   Integration": an ordered `SetupCost` per transport, adjusted by a
//!   caller-reported `ConnectionPoolState`, now a real weighted term in
//!   [`scoring::DefaultScorer`] (previously that formula had nothing
//!   for setup cost at all).
//! - [`security`] — §47 "Peer Session Abstraction", §48 "Security
//!   Constraints": a smart-constructor `AuthenticatedSession` that can
//!   only be built by re-checking a candidate's peer against Part 02's
//!   `TrustedAccountStore`, plus a list-level elimination function —
//!   defense-in-depth on top of [`resolve`]'s own device-list-stage
//!   trust filtering.
//! - [`privacy`] — §49 "Privacy Policy", §50 "Direct vs Relay
//!   Preference", and §52 "Wi-Fi Direct/Aware"'s setup-threshold gate
//!   (also covers §53's Bluetooth-pairing side of that same threshold).
//!   §51/§54/§56 are accounted for in that module's own doc comment
//!   without needing new functions; §55 "Mesh Forwarding" is named
//!   there as a real, un-implemented gap rather than papered over.
//! - [`descriptor`] — §57 "Delivery Semantics" (closed via two new
//!   [`requirements::DeliveryRequirements`] constructors,
//!   `typing_indicator`/`file_chunk`, alongside the two constructors
//!   that already existed), §58 "Operation Descriptor", §59 "Content
//!   Class" — named exactly as the spec lists them.
//! - [`estimate`] — §60 "Size-Aware Routing" (a real, labeled-as-
//!   coarse `completion_time ≈ setup + bytes / bandwidth` estimate)
//!   and §61 "Deadline-Aware Routing" (hard elimination of candidates
//!   whose estimated completion exceeds `max_latency_millis`, rather
//!   than queuing them indefinitely).
//! - §62 "Expiry-Aware Routing" — [`requirements::DeliveryRequirements::has_expired`]
//!   plus [`retry::RetryPolicy::allows_attempt_at`], which combines it
//!   with the existing attempt-count cap. Not its own module — both
//!   pieces extend types that already existed.
//! - [`dispatch`] — beyond its existing priority-fair bridge to
//!   `siar-protocol-ext`, this is also where §63 "Queue Architecture",
//!   §64 "Weighted Fair Scheduling", and §65 "Backpressure" are
//!   accounted for (all three are already real one layer down, in
//!   `siar-protocol-ext`'s own `FairScheduler`/`BoundedQueue` — see
//!   this module's own doc comment), plus §66 "Per-Transport Queues"
//!   (`PerTransportDispatchQueue`, new this round): one independent
//!   dispatch queue per [`types::TransportKind`], so a stalled
//!   Bluetooth backlog cannot block a healthy Iroh one.
//! - [`fairness`] — §67 "Per-Peer Fairness", §68 "Per-Extension
//!   Fairness": one generic `RoundRobinFairQueue<K, T>` covering both,
//!   composable rather than wired into [`dispatch`] (see that module's
//!   own doc comment for why: `siar-protocol-ext`'s per-tier queue
//!   implementation is fixed and not swappable from this crate).
//! - §69 "Route Planning for Messaging", §70 "Route Planning for
//!   Files", §72 "Route Planning for Emergency" — verified, not
//!   separately implemented: this crate's own `plan` module test suite
//!   has integration tests
//!   checking that [`plan::plan_route`]'s existing scoring actually
//!   produces the compositions/orderings these sections describe
//!   (where that's robustly true without fabricated measurements —
//!   see those tests' own doc comments for the one case, DTN's
//!   relative position in §69, this crate deliberately does *not*
//!   claim to get right without real caller-supplied data).
//! - [`quality`] — §71 "Route Planning for Calls"'s "routing supplies
//!   path quality signals to media adaptation" half (the video →
//!   audio → voice-message fallback ladder itself is a media-
//!   adaptation decision, out of this crate's scope; the *selection*
//!   half of §71 was already covered by existing scoring before this
//!   round).
//! - [`diversity`] — §73 "Path Diversity", §74 "Underlay Group": a
//!   real `UnderlayId` newtype (not the bare unit-struct stub §74's
//!   own code block shows — see this module's own doc comment),
//!   `are_diverse`/`group_by_underlay`, and `most_diverse_fallback`,
//!   now actually used by [`plan::plan_route`] to pick which fallback
//!   becomes a `Redundant` plan's replica.
//! - §75-78 "Multipath Chunk Scheduler"/"Path Collapse"/"Duplicate
//!   Chunk Handling"/"Realtime Multipath" — read and deliberately not
//!   implemented; the spec itself names all four as future/optional/
//!   advanced work, not v1, matching [`plan::plan_route`]'s own doc
//!   comment on why it never produces `RouteStrategy::Multipath`.
//! - [`metrics::CongestionState`] plus two new [`metrics::PathMetrics`]
//!   fields — §79 "Congestion Signals," closing a gap
//!   [`scoring::DefaultScorer`]'s own doc comment named since this
//!   crate's first phase ("the two it doesn't: congestion, failure
//!   penalty"). Only congestion; failure penalty still needs failure
//!   *history* this crate keeps no record of. "RTT trend" (also named
//!   by §79) is deliberately not a field — a trend needs samples over
//!   time this crate has no clock to collect (see this crate's own top
//!   doc comment on scope).
//! - [`stability`] — §80 "Route Stability Score":
//!   `derive_stability_score` turns connection lifetime/failure rate/
//!   path changes/timeout count into the [`metrics::StabilityScore`]
//!   [`scoring::DefaultScorer`] already consumes — the half of §80
//!   that didn't already exist (the other half, "a path with slightly
//!   worse RTT but much better stability may win," was already true of
//!   existing per-term weighting).
//! - §81 "Cold vs Warm Path" — already covered before this round:
//!   `warm` is exactly [`setup::ConnectionPoolState::Active`], which
//!   [`setup::effective_setup_cost`] already collapses to
//!   [`setup::SetupCost::Cheap`] regardless of the transport's own
//!   static cost (§46, this crate's own round-2 work) — precisely
//!   §81's "warm path gets a setup-cost advantage."
//! - [`types::MeteredState`], [`types::RoamingState`] — §82 "Metered
//!   Networks," §83 "Roaming": real tri-state enums (not the `bool`
//!   [`candidate::PathCandidate::capabilities`] used to carry), each
//!   with an `Unknown` case a platform that hasn't reported yet can
//!   express, wired into [`scoring::passes_hard_constraints`] via two
//!   new functions that both treat `Unknown` conservatively (same
//!   direction as [`metrics::Confidence::Stale`]'s own "don't trust
//!   what you can't currently verify" reasoning) — `Unknown` metered
//!   state is blocked exactly like confirmed-metered; `Unknown`
//!   roaming state is blocked for bulk transfers exactly like
//!   confirmed-roaming. §82's own "do not use one global allow/deny"
//!   was already satisfied structurally by `allow_metered` being a
//!   per-`DeliveryRequirements` field, not a crate-wide setting — this
//!   round's real fix was that [`requirements::DeliveryRequirements::file_chunk`]
//!   had `allow_metered: true`, contradicting §82's own "block bulk"
//!   worked example; now corrected, alongside a new
//!   `allow_roaming_bulk` field for §83's own "allow cellular but
//!   forbid roaming bulk transfer" example specifically.
//! - §84 "Battery Cost" — already covered before this round by
//!   [`metrics::EnergyCost`] (see that type's own doc comment for why
//!   its variant names differ cosmetically from §84's own list without
//!   being a gap).
//! - [`resolve`] — §16/§17 "Destination Resolution"/"Account-Level
//!   Routing", the one piece of this crate that reaches into another
//!   real crate (`siar-identity-multidevice`) rather than staying
//!   self-contained, exactly because §16 says "Part 02 provides device
//!   membership" and Part 02 now has a real crate to mean that with.
//! - [`dispatch`] — a second real cross-crate integration, this time
//!   with `siar-protocol-ext`'s already-built `FairScheduler`/
//!   `BoundedQueue` (its own §21-22 "Fair Scheduling"): maps this
//!   crate's [`types::Priority`] onto that crate's `TrafficPriority`
//!   and queues `(RoutePlan, payload)` pairs for priority-fair
//!   dispatch. Not itself named by either spec (see that module's own
//!   doc comment), but a real gap this closes: previously nothing
//!   connected an application's delivery priority to actual queueing
//!   behavior once a route was chosen.
//!
//! §197's own Phase 5 ("DTN") is touched only incidentally — a `Dtn`
//! [`types::TransportKind`] variant exists and competes in scoring like
//! any other transport, but nothing here implements actual bundle
//! persistence or opportunistic forwarding (§23's own "persist
//! operation → select DTN bundle policy → wait for peer encounter →
//! forward opportunistically" pipeline).
//!
//! ## What's explicitly NOT here
//!
//! - **§197 Phase 2 (candidate collection).** Nothing here integrates
//!   Iroh, LAN, Bluetooth, or Wi-Fi discovery — [`candidate::PathCandidate`]
//!   is a value type a caller constructs from real discovery data this
//!   crate has no dependency on producing.
//! - **§197 Phase 6 (resource scheduling)** — §63-68 (queue
//!   architecture, weighted fair scheduling, backpressure, per-transport/
//!   peer/extension fairness) — [`dispatch`] now covers priority-tier
//!   fair scheduling and backpressure end to end (via
//!   `siar-protocol-ext`'s `FairScheduler`/`BoundedQueue`); per-transport
//!   and per-peer/extension fairness weighting is still not attempted.
//! - **§197 Phase 7 (diagnostics/testing)** — §95-99 (route
//!   diagnostics, path visualization, decision explainability, metrics
//!   collection/privacy), §124-127 (simulated routing/property/chaos/
//!   failover tests beyond this crate's own unit tests) — not
//!   attempted.
//! - [`platform`] — §85 "Battery-Aware Inputs" (`DeviceState`), §87
//!   "Platform Policy Integration" (see that module's own doc comment
//!   for how §87's six named signals map across this crate — three
//!   already existed, three land in this module and [`acquisition`]).
//! - [`acquisition`] — §86 "Background Restrictions"
//!   (`is_currently_usable`/`eliminate_background_restricted`, now
//!   actually called by [`plan::plan_route`]), §89 "Passive vs Active
//!   Candidates" (`CandidateState`, transcribed exactly from that
//!   section's own code block, now a real [`policy::PolicyWeights::candidate_state`]
//!   scoring term).
//! - [`discovery`] — §88 "Path Acquisition", §90 "Discovery Budget":
//!   a real, stateful `DiscoveryBudget` (sliding window + cooldown, a
//!   caller persists it across calls) plus `discovery_permitted`,
//!   which combines the budget with [`platform::DeviceState`] —
//!   thermal-critical blocks unconditionally (a hardware safety
//!   margin, no priority override); battery-saver blocks unless
//!   `Priority::Critical`, the same override shape
//!   [`privacy::justifies_expensive_setup`] (§52) already uses.
//! - [`resilience`] — §91 "Route Escalation Ladder"
//!   (`EscalationStage`/`escalation_stage_of`, built entirely from
//!   existing types: [`acquisition::CandidateState`] for stages 1-2,
//!   [`setup::SetupCost`] for the 3-vs-4 split, `TransportKind::Dtn`
//!   for stage 5), §92 "Timeout by Stage"
//!   (`timeout_millis_for_stage`), §93 "Hedged Requests"
//!   (`HedgePolicy`/`hedge_policy_for`, now a real
//!   [`plan::RouteStrategy::Hedged`] [`plan::plan_route`] can actually
//!   produce, with `RoutePlan::hedge_delay_millis` carrying the
//!   delay), §95 "Route Diagnostics" (`RouteDiagnostics`/`diagnose`,
//!   scoped to the two rejection checks this crate can run without an
//!   external policy object — see that module's own doc comment for
//!   why security/privacy checks aren't included). §94
//!   "Deduplication Requirement" has no function of its own — see
//!   [`descriptor::OperationId`]'s own doc comment, extended this
//!   round to cover `Hedged` alongside `Redundant`. §96 "Path
//!   Visualization" is explicitly deferred by the spec itself to
//!   "Part 18" — nothing to implement yet.
//! - [`explain`] — §97 "Route Decision Explainability"
//!   (`RouteReason`, transcribed exactly, now a real
//!   `RoutePlan::reason` field), §98 "Metrics Collection"
//!   (`RouteMetricEvent`/`metric_events_for` — event classification
//!   only, no counters kept, since this crate has no history of its
//!   own; see that module's own doc comment for which of §98's named
//!   metrics have no equivalent here at all, and why), §100 "Routing
//!   State Store" (`RouteHint`/`hint_from_plan`), §101 "Startup
//!   Behavior" (`revalidate_hint` — the one step of that section's own
//!   three-step startup sequence that's actually this crate's to
//!   implement). §99 "Privacy of Metrics" needed no function: it's
//!   enforced by construction, not filtering — see [`explain::RouteMetricEvent`]'s
//!   own doc comment for why there's no field to redact in the first
//!   place. §102 "Suspend/Resume" turned out to already be covered
//!   entirely by composing existing pieces: [`cache::RouteCache::invalidate_all`]
//!   (invalidate stale metrics), a fresh [`plan::plan_route`] call
//!   (reassess active sessions), [`retry::RetryPolicy`] (retry durable
//!   operations), and [`discovery::DiscoveryBudget`]'s own cooldown
//!   (do not immediately launch every discovery mechanism) — nothing
//!   new needed. §103 "Process Death" is likewise already true by
//!   design, not by this round's addition — see [`plan::RoutePlan`]'s
//!   own doc comment for why it deliberately has no
//!   `Serialize`/`Deserialize` derive. §104 "Route Plan Lifetime" adds
//!   real fields: `RoutePlan::created_at_millis`/`valid_until_millis`,
//!   the latter derived from `RoutingPolicy::hysteresis.minimum_hold_millis`
//!   rather than a new invented duration.
//! - [`authorization`] — §105 "Path Authorization" (`authorize_path`,
//!   composing the four checks the spec lists — see that module's own
//!   doc comment for why three of the four were already real one
//!   layer down and only "extension supported" is new code), §106
//!   "Extension Capability Integration" (the new
//!   `siar-protocol-ext::peer::PeerCapabilities` check itself, plus
//!   `descriptor::OperationDescriptor::required_extension` to carry
//!   the fact an operation needs one), §107 "Device Capability
//!   Integration" (`select_devices_with_capability`, and a new
//!   `siar_identity_multidevice::DeviceCapabilitySet::REALTIME_MEDIA`
//!   bit that §107's own "phone supports video, headless relay does
//!   not" example needed and didn't have before this round).
//! - [`decision`] — §108 "Policy Layering" (`PolicyLayers`, a real
//!   type for the spec's own five-layer diagram), §109 "System
//!   Policy" (`SystemPolicy`), §110 "Application Policy"
//!   (`ApplicationPolicy` — the one example of its three with no
//!   existing home; see that module's own doc comment for why the
//!   other two were already `DeliveryRequirements` fields), §111
//!   "User Policy" and §112 "Operation Policy" (both already fully
//!   covered by [`privacy::PrivacyPolicy`] and
//!   [`requirements::DeliveryRequirements`] respectively — zero new
//!   code for either), §113 "Policy Conflict" (`decide_route`'s own
//!   layer-by-layer elimination order, tested against the spec's
//!   exact large-file/no-metered/only-metered-path-exists example),
//!   §114 "Policy Result Types" (`RouteDecisionResult`, transcribed
//!   with its four variants named exactly as listed), §115 "Deferred
//!   Reasons" (`DeferredReason`'s six variants, each wired to a real
//!   condition rather than left inert).
//! - [`ui_state`] — §116 "UI-Friendly State" (`RouteUiState`,
//!   `ui_state_for` — a genuine many-to-one collapse of
//!   `RouteDecisionResult`/`DeferredReason` down to five neutral
//!   states; see that module's own doc comment for the two inferred
//!   collapsing choices it makes).
//! - [`config`] — §117 "Route Policy Configuration" (`RoutingConfig`,
//!   transcribed field-for-field; five of its seven fields reuse an
//!   existing type outright — see that module's own doc comment —
//!   plus `validate()`'s one real startup check).
//! - §118 "No Global Singleton" needed no new code at all: this
//!   crate has never had a `static`/`lazy_static!`/`once_cell`/
//!   `thread_local!` anywhere in `src/` (verified by grep, not
//!   assumed) — every function takes every piece of state it needs
//!   as an explicit parameter, which is what "each `CommunicationRuntime`
//!   owns its routing engine" actually requires structurally. [`engine`]'s
//!   own `TestEngine` (in that module's tests) is a worked example of
//!   one caller owning one engine instance with its own
//!   `TrustedAccountStore`/`RoutingPolicy`/`DiscoveryBudget`.
//! - [`engine`] — §119 "Routing Engine API" (`RoutingEngine` trait,
//!   `RouteRequest`/`RouteDecision`/`RouteResultReport`/`RouteOutcome`,
//!   plus a real implementation and a from-scratch minimal executor
//!   in that module's own tests, since this crate has no async
//!   runtime dependency to reach for). §120 "Transport Manager API"
//!   is the one section this round with no code — see [`engine`]'s
//!   own doc comment for why: its trait names three types
//!   (`ResolvedDestination`/`TransportSession`/`TransportError`) that
//!   belong to whichever crate owns actual sockets, not this one.
//! - [`engine::health_after_outcome`] — §121 "Feedback Loop"'s own
//!   "health update" step, real as a pure single-sample transition;
//!   the "metrics update" step immediately before it in the spec's
//!   own diagram needs history this crate has kept out of scope since
//!   round 9 (see that function's own doc comment).
//! - §122 "Avoid ML Initially" needed no new code: this crate has
//!   never depended on anything ML-shaped (no such crate in
//!   `Cargo.toml`, ever) and [`scoring::DefaultScorer`] is, and has
//!   always been, an ordinary deterministic weighted sum.
//! - §123 "Deterministic Scoring" — no new production code either;
//!   this round added property tests at three layers proving what
//!   was already true by construction (no RNG, no clock reads inside
//!   scoring itself): [`scoring`]'s own `score()`, [`plan::plan_route`],
//!   and [`decision::decide_route`] each have a test calling the same
//!   function twice with identical inputs and asserting identical
//!   output.
//! - §124 "Simulated Routing Tests" — [`plan`]'s own test module now
//!   has the spec's exact worked example transcribed with its exact
//!   numbers (Path A: 10ms/1Mbps/metered, Path B: 50ms/100Mbps/
//!   unmetered), covering both of its outcomes: text message
//!   genuinely depends on policy (proven both ways), large file
//!   always lands on B regardless of policy (a hard constraint, not
//!   a preference).
//! - §125 "Policy Property Tests" — all four transcribed as tests in
//!   [`decision`]: revoked device never selected, forbidden metered
//!   path never selected, realtime operation never uses DTN (tested
//!   adversarially: even a DTN candidate that falsely claims realtime
//!   capability is still excluded), and expired operation never
//!   routed. That fourth one **surfaced a real, previously-unnoticed
//!   gap** — `decide_route` had no way to know when an operation was
//!   created at all, so nothing before this round actually enforced
//!   it; fixed by adding
//!   [`descriptor::OperationDescriptor::created_at_millis`] and a
//!   `RejectReason::OperationExpired` check, not merely a test
//!   confirming something already worked.
//! - §126 "Chaos Tests" — [`plan`]'s own
//!   `spec_126_wifi_flapping_does_not_cause_a_route_storm` simulates
//!   20 re-plans with a narrowly alternating "which path looks
//!   slightly better" measurement and asserts stickiness absorbs
//!   nearly all of it. This crate's other two named chaos properties
//!   ("no infinite retry loop," "bounded queues") already had
//!   dedicated tests from earlier rounds — see that test's own doc
//!   comment for exactly which ones — so nothing new was needed for
//!   those two.
//! - §127 "Failover Test" — [`plan`]'s own
//!   `spec_127_a_failed_primary_fails_over_to_a_healthy_fallback`
//!   proves the spec's exact scenario end to end. Its second half —
//!   "operation resumes if semantics allow... routing only
//!   coordinates path change, feature layer owns semantic resume" —
//!   is a statement about a layer above this one; `plan_route` has no
//!   concept of "resume," so nothing here could honestly test that
//!   part.
//! - §128 "File Resume Integration" and §129 "Messaging Retry
//!   Integration" needed no code — both are pure boundary statements
//!   ("do not make routing understand file chunk state," "routing
//!   does not create duplicate message semantics") that this crate
//!   already respects by never having a chunk-state or message-ID
//!   concept anywhere in it.
//! - [`engine::RouteChangeEvent`] — §130 "Call Path Change
//!   Integration"'s reporting half (`NewPath`/`QualityUpdate`); the
//!   call/media-layer actions the spec's own text pairs with it
//!   ("rebind, renegotiate, adapt bitrate") are, like
//!   [`engine::RouteResultReport`]'s feedback direction, a layer this
//!   crate reports to rather than performs.
//! - [`risk`] — §131 "Security Event Integration"
//!   (`handle_security_event`, real cache invalidation plus data a
//!   caller uses to remove the candidate and emit its own event),
//!   §132 "Blacklisting" (`PathPenalty`, transcribed with the spec's
//!   own two fields), §133 "Peer Abuse" (`PeerAbuseStatus`,
//!   deliberately without a "rate limit" variant — see that module's
//!   own doc comment for why that one stays
//!   [`fairness::RoundRobinFairQueue`]'s job instead).
//! - [`scope`] — §134-136's three named transport-list modes plus
//!   §137's `RouteScope` enum tying them together; that module's own
//!   doc comment names the one place §134 and §136 disagree (`Dtn`)
//!   rather than silently reconciling it.
//! - §138 "Emergency Override" —
//!   [`privacy::effective_privacy_policy`], checked inside
//!   [`decision::decide_route`]'s own user-policy step: lifts
//!   `avoid_relay` specifically, and only when both
//!   `Priority::Critical` and the user's own explicit opt-in
//!   (`PrivacyPolicy::emergency_override_enabled`) are true — proved
//!   both ways (with and without the opt-in) at the full
//!   `decide_route` level, not just in isolation.
//! - §139 "User Consent" needed no new code:
//!   [`privacy::PrivacyPolicy`] already **is** "the resulting policy"
//!   routing consumes, per the spec's own last line — the "explicit
//!   product-level consent settings" it names are an onboarding/UI
//!   concern that produces a `PrivacyPolicy`, not something this
//!   crate collects itself.
//! - [`resource_pressure`] — §140 "Bandwidth Reservation"
//!   (`BandwidthReservation`/`bulk_should_yield_to_reservation`, kept
//!   deliberately thin per the spec's own "future coexistence"
//!   framing), §141 "Traffic Shaping" (`TrafficShapingPolicy`, the
//!   spec's own two named caps — Bulk-during-a-call and
//!   Background-priority — kept as two independently-triggered
//!   conditions, not one), §142 "Connection Admission"
//!   (`ConnectionAdmission`/`admission_permitted`, with the same
//!   `Priority::Critical`-always-bypasses shape §138's emergency
//!   override established for a different resource),
//!   §143 "Thermal Awareness" (three named reductions — multipath,
//!   Wi-Fi Direct setup, background bulk — all gated on the same
//!   `ThermalState::Critical` threshold [`discovery::discovery_permitted`]
//!   already established rather than a second severity line), §144
//!   "Memory Pressure" (`MemoryPressure` on
//!   [`platform::DeviceState`], `memory_pressure_allows_acquisition`
//!   honoring "durable operations remain persisted" as an override
//!   rather than an exception, `recommended_queue_capacity`). None of
//!   these five perform the actual resource action (shaping traffic,
//!   dropping a packet) — each computes the decision; see that
//!   module's own doc comment.
//! - [`adapters`] — §145 "Transport Adapter Contract" through §150
//!   "DTN Adapter," almost entirely documentation rather than new
//!   code: §145's own "report" contract is already exactly what
//!   [`candidate::PathCandidate`] carries, field for field, and its
//!   "support" contract (connect/close/send) is the same kind of gap
//!   §120 already named for the same reason. §146-150's own per-field
//!   lists are checked one by one in that module's own doc comment —
//!   most already have a home; the genuine gaps (Bluetooth
//!   proximity/paired state, Wi-Fi group/session, LAN interface) are
//!   named rather than faked with a field that wouldn't actually mean
//!   anything yet. ("DTN delivery probability," also named a gap when
//!   this bullet was first written, got its representation half
//!   filled in by [`probability`] below — see that module's own doc
//!   comment.)
//! - [`probability`] — §151 "Route Probability"
//!   (`PathMetrics::delivery_likelihood`/`expected_delay_class`, the
//!   place an adapter with real encounter history reports an estimate
//!   this crate still doesn't compute itself), §152 "Metric Types
//!   Must Match Reality" and §153 "Scoring Normalization" (both
//!   already true by this crate's standing design — typed metric
//!   fields, normalized per-factor suitability terms — zero new
//!   code), §154 "Policy Weight Example" ([`policy::PolicyWeights`]
//!   already the real superset of the spec's own simplified 5-term
//!   example).
//! - [`scoring::RouteScore::as_fixed_point`] — §155 "Integer Score
//!   Option," normalized against [`policy::PolicyWeights::sum`]
//!   rather than an assumed constant, since that struct's own weights
//!   aren't required to sum to 1.0.
//! - [`diagnostics`] — §156 "Route Decision Logging"
//!   (`RouteDecisionLog`), §157 "Developer Diagnostics"
//!   (`DeveloperDiagnostics`, transcribing the spec's own worked
//!   example field-for-field except "Destination: Bob Phone," which
//!   has no honest source here — see that module's own doc comment;
//!   [`explain::RouteReason::description`] added specifically to
//!   produce the example's "Reason:" line), §158 "Route History"
//!   (`RouteHistory`, a real bounded ring buffer — "do not retain
//!   indefinitely" enforced structurally by its own `push`, not by a
//!   caller's discipline).
//! - [`telemetry`] — §159 "Telemetry Export" (`TelemetrySummary`,
//!   with no identity field anywhere in it to redact in the first
//!   place — the same "can't leak what it has no field for" shape
//!   §99's `RouteMetricEvent` already established).
//! - [`engine::RouteRequest`]'s builder methods — §160-163 "API
//!   Example: Text Message/Large File/SOS/Video Call," matching each
//!   example's own exact method names and call shape
//!   (`RouteRequest::for_device(...).class(...)...`, no separate
//!   builder type). §162's own `.allow_redundancy(true)` needed a
//!   real new field —
//!   [`requirements::DeliveryRequirements::allow_redundancy`] — since
//!   `plan_route`'s `RouteStrategy::Redundant` trigger had no way to
//!   be turned off before this round, in tension with §21's "use
//!   redundancy sparingly." `with_candidates` bridges the one real
//!   gap between the spec's own illustrative snippets and this
//!   crate's actual requirements — none of the four examples ever
//!   mention candidates, which have to come from somewhere.
//! - [`engine::RouteOutcome`] — §165, a **correction**, not
//!   originally this shape: round 13's first version reused
//!   [`failure::RouteFailureClass`] rather than inventing a second
//!   taxonomy, but §165 specifies a genuinely different, flatter
//!   8-variant enum with two cases (`Partial`, `Cancelled`) that
//!   `RouteFailureClass` has no equivalent for at all — see that
//!   type's own doc comment for the full reconciliation.
//!   [`engine::ObservedMetrics`] (§164, a type alias for
//!   [`metrics::PathMetrics`]) is `RouteResultReport`'s new field.
//!   [`engine::health_after_outcome`]'s own match arms were rewritten
//!   to match, adding real handling for §166 "Partial Outcome"
//!   (treated as gradual evidence the path still basically works, not
//!   written off) and §167 "Cancellation" (leaves the health estimate
//!   unchanged — a user decision carries no signal about path
//!   quality).
//! - [`path_switch`] — §168 "Graceful Path Switch" is pure
//!   session-layer mechanics this crate has no session to perform
//!   (same boundary as §120); §169/§170 "Make-Before-Break"/
//!   "Break-Before-Make" is the one real decision in this cluster —
//!   `path_switch_strategy_for`, with §170's own resource/security
//!   triggers treated as overriding §169's softer "reduces
//!   interruption" preference.
//! - **Everything from roughly §171 onward that isn't listed above** —
//!   multi-device route
//!   aggregation and group/broadcast routing (§171-175), storage-cost
//!   awareness (§176-178), and the remainder of this 200-section
//!   document not named above. §55 "Mesh
//!   Forwarding"'s richer candidate representation (next hop, route
//!   utility, hop budget, relay trust policy) also remains
//!   unimplemented — see [`privacy`]'s own doc comment. This is a
//!   genuinely small slice of a very large spec — see §198 "Definition
//!   of Done" in the source document for the full bar this crate does
//!   not yet clear.
//!
//! ## Relationship to the existing `siar-routing` crate
//!
//! `siar-routing` (this workspace, built earlier, against "next.md")
//! already has real path scoring, link health tracking, and a
//! scheduler covering similar conceptual ground — device routes, path
//! scoring, link health — under different type names and a different
//! design. Neither crate depends on or replaces the other. Reconciling
//! them (migrating one onto the other, keeping both for different
//! contexts, or retiring one) is a genuine product/architecture
//! decision this crate does not make unilaterally — the same posture
//! `siar-identity-multidevice` already takes toward the existing
//! `siar_crypto::device_cert` system, for the same reason.

pub mod acquisition;
pub mod adapters;
pub mod authorization;
pub mod cache;
pub mod candidate;
pub mod config;
pub mod decision;
pub mod descriptor;
pub mod diagnostics;
pub mod discovery;
pub mod dispatch;
pub mod diversity;
pub mod engine;
pub mod error;
pub mod estimate;
pub mod explain;
pub mod failure;
pub mod fairness;
pub mod metrics;
pub mod path_switch;
pub mod plan;
pub mod platform;
pub mod policy;
pub mod privacy;
pub mod probability;
pub mod quality;
pub mod requirements;
pub mod resilience;
pub mod resolve;
pub mod resource_pressure;
pub mod retry;
pub mod risk;
pub mod scope;
pub mod scoring;
pub mod security;
pub mod setup;
pub mod stability;
pub mod telemetry;
pub mod types;
pub mod ui_state;

pub use cache::RouteCache;
pub use candidate::{PathCandidate, TransportEndpoint};
pub use descriptor::{ByteCount, ContentClass, OperationDescriptor, OperationId};
pub use diversity::{are_diverse, group_by_underlay, most_diverse_fallback, UnderlayId};
pub use error::RoutingError;
pub use estimate::{
    completion_time_millis, eliminate_deadline_exceeding_candidates, exceeds_deadline,
};
pub use explain::{
    hint_from_plan, infer_reason, metric_events_for, revalidate_hint, RouteHint, RouteMetricEvent,
    RouteReason,
};
pub use failure::RouteFailureClass;
pub use metrics::{
    Bitrate, Confidence, CongestionState, EnergyCost, MeasuredValue, NetworkCost, PathMetrics,
    Ratio, SignalQuality, StabilityScore,
};
pub use plan::{plan_route, RoutePlan, RouteStrategy};
pub use policy::{HysteresisPolicy, PolicyWeights, RoutingPolicy, RoutingPolicyProfile};
pub use privacy::{
    direct_preference_bonus, eliminate_privacy_violations, eliminate_unjustified_expensive_setup,
    justifies_expensive_setup, passes_privacy_policy, PrivacyPolicy,
};
pub use quality::{quality_signal_for, PathQualitySignal};
pub use requirements::DeliveryRequirements;
pub use resilience::{
    diagnose, escalation_stage_of, hedge_policy_for, should_escalate_beyond,
    timeout_millis_for_stage, EscalationStage, HedgePolicy, RejectionReason, RouteDiagnostics,
};
pub use resolve::resolve_destination_devices;
pub use retry::RetryPolicy;
pub use scoring::{DefaultScorer, PathScorer, RouteScore, RouteScoreDelta, RoutingContext};
pub use security::{authorize_candidate, eliminate_untrusted_candidates, AuthenticatedSession};
pub use setup::{effective_setup_cost, static_setup_cost, ConnectionPoolState, SetupCost};
pub use stability::derive_stability_score;
pub use types::{
    DeliveryClass, Destination, MeteredState, PathCapabilities, PathId, Priority, RoamingState,
    RouteHealth, TransportKind,
};
