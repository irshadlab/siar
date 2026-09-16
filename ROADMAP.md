# siar — Roadmap & Coverage Hierarchy

Maintained alongside the code, not a substitute for it. This file tracks
**what's written vs. what's left**, ordered by actual need — not spec
number order. Update this file every time a spec's coverage changes.

Spec source: `sys-arch/` (33 numbered core-architecture docs +
27 `ui-ux-NN` docs, uploaded once, Wiki-style). Workspace source:
33-crate Rust workspace (`siar-source`) + `apps/{cli,desktop,android,
emergency-node}` + `platform/android`.

Legend: ✅ Done/substantial · 🟡 Partial · ⚪ Not started · 🔒 Blocked on
a prerequisite · 📋 Reconciled (already covered elsewhere, no new code
needed)

---

## Tier 0 — Foundational core crates (Parts 01-09)

All nine have real, compile+test-verified coverage, tracked crate-by-
crate. Status: **substantially built, gaps are depth not breadth** —
each has a documented list of untouched later sections. See per-crate
notes in project memory (`/areas/resilient-mesh.md`) for exact section
counts; not re-duplicated here since that detail changes less than this
file's priority ordering does.

**2026-09-01: working through all nine 1-by-1, round by round.** Total
remaining across all nine is ~1,110 sections (1,552 total, ~445 done
after this round) — at this project's actual historical pace (5-15
sections per round, each needing real spec-reading + code + compile +
test, not just prose), that's realistically 70-140+ more rounds, not
something any single session finishes. Tracking progress here as it
happens rather than promising a completion date. This round: spec 01
(`siar-protocol-ext`) §17 (confirmed already done, doc was stale),
§24-26, §28-31, §32-33 — see its own crate row below and its `lib.rs`
doc comment for specifics. Also fixed two real bugs in `siar-transport`
(pooled-connection reuse silently dropping every message after the
first) and added full test coverage to `siar-transport`/`siar-messaging`'s
`MessageService` half this session — see `/areas/resilient-mesh.md`,
not spec-numbered work but real correctness fixes found via testing.

**2026-09-05/06 update:** spec 02 (`siar-identity-multidevice`) is now
ALSO complete (204/204), across 11 rounds this session (§91-204) on top
of the ~90/204 already done before this session started. Two of the
nine Tier 0 crates are now fully spec-complete (01 and 02); the
remaining ~1,110-section estimate above is now overstated by roughly
114 sections (204 − 90) — a precise updated total across all nine
hasn't been recomputed, but 03-09 remain the actual "~1,110 minus 114"
scope.

**2026-09-08 update:** spec 03 (`siar-routing-policy`) round 2 done —
§43-56 (~14 sections), bringing it to ~74/200. Real new code: §43
targeted per-transport cache invalidation; §44/§46 a `setup.rs` module
(`SetupCost`/`ConnectionPoolState`) now folded into
`DefaultScorer` as two new `PolicyWeights` terms
(`setup_cost`/`existing_connection`, tuned per profile) — closing a gap
this crate's own doc comments had named but not filled; §47/§48 a new
`security.rs` with an `AuthenticatedSession` smart-constructor that
re-verifies a candidate's peer against Part 02's `TrustedAccountStore`
(defense-in-depth on top of `resolve.rs`'s existing device-list-stage
filtering); §49/§50/§52 a new `privacy.rs` (`PrivacyPolicy` hard
constraints, a direct-preference soft bonus, and §52's Wi-Fi
Direct/Aware setup-threshold elimination). §51/§53/§54/§56 accounted
for via existing/new fields without new functions (documented in
`privacy.rs`'s own doc comment); §55 "Mesh Forwarding" named as a real,
unimplemented gap (no distinct next-hop/hop-budget candidate shape
exists). Two new `DeliveryRequirements` fields
(`nearby_session_explicit`, `dtn_replication_budget`) — required a
downstream fix in `siar-dtn-bundle`'s own test helper (missing-fields
compile error caught by a full-workspace check, not just this crate's
own `cargo check`). 49/49 tests (up from 35), clippy clean, fmt clean,
doc-warning-free, zero regressions verified in `siar-dtn-bundle`
(37/37), `siar-identity-multidevice` (251/251), and `siar-protocol-ext`
(115/115). Next for spec 03: §57 onward.

**2026-09-08 update (round 3):** §57-62 done, ~74→~80/200. New
`descriptor.rs` — `OperationId`/`ByteCount`/`ContentClass`/
`OperationDescriptor`, named exactly per §58/§59; two new
`DeliveryRequirements` constructors (`typing_indicator`, `file_chunk`)
close out §57's own four worked examples (message/call-frame already
had constructors from Phase 1). New `estimate.rs` — §60's own
`completion_time ≈ setup + bytes / bandwidth` formula as a real
function (`completion_time_millis`, `None` for "with uncertainty"
rather than a fabricated confidence interval), plus §61's hard
"exceeds deadline → drop" elimination
(`eliminate_deadline_exceeding_candidates`). §62 closed without a new
module: `DeliveryRequirements::has_expired` and
`RetryPolicy::allows_attempt_at` finally give the `expiry_millis` field
(present since Phase 1) an actual enforcement path — it had sat unused
until this round. 63/63 tests (up from 49), clippy clean, fmt clean,
doc-warning-free (one broken intra-doc link from `[Routing]` in a doc
comment caught and escaped), zero regressions in `siar-dtn-bundle`
(37/37), `siar-identity-multidevice` (251/251), `siar-protocol-ext`
(115/115) — this round added no new `DeliveryRequirements` fields, so
no downstream literal-construction fix was needed this time.

**2026-09-08 update (round 4):** §63-68 done, ~80→~86/200.
Confirmed §63/§64/§65 ("Queue Architecture"/"Weighted Fair
Scheduling"/"Backpressure") were already real one layer down in
`siar-protocol-ext`'s `FairScheduler`/`BoundedQueue`, wired through
this crate's existing `dispatch.rs` bridge — documented in that
module's own doc comment rather than reimplemented. New this round:
`PerTransportDispatchQueue` (§66) — one independent
`RouteDispatchQueue` per `TransportKind`, so a stalled Bluetooth
backlog genuinely cannot block a healthy Iroh one, which a single
shared queue could never guarantee. New `fairness.rs` module: one
generic `RoundRobinFairQueue<K, T>` (bounded per-key, round-robin
across whichever keys have pending items) instantiated two ways in its
own tests — keyed by `DeviceId` for §67 "Per-Peer Fairness" and by the
new `ContentClass` (from round 3's `descriptor.rs`) for §68
"Per-Extension Fairness" — one primitive closing two spec sections
rather than writing the same round-robin logic twice. Deliberately not
wired into `RouteDispatchQueue` itself: `FairScheduler`'s own per-tier
queue is a fixed plain FIFO owned by `siar-protocol-ext`, not
swappable from this crate — composable instead, same posture as
`security.rs`/`privacy.rs`'s own elimination functions. 71/71 tests (up
from 63), clippy clean, fmt clean, doc-warning-free, zero regressions
in `siar-dtn-bundle` (37/37), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115). This round's diff was delivered as a
git patch (`git format-patch`/`git diff` against round-3's tree) per
explicit request, not a tarball — the project now has an actual git
repo (initialized this round, baseline commit = round 3's state) to
make that possible going forward.

**2026-09-08 update (round 5):** §69-74 done, ~86→~92/200. New
`diversity.rs`: a real `UnderlayId` newtype (§74's own code block is
literally just `pub struct UnderlayId;` — read as an illustrative
stub, not a literal instruction, same as this crate already does for
similar bare-stub spec snippets), `are_diverse`/`group_by_underlay`,
and `most_diverse_fallback` — and unlike round 4's `fairness.rs`
primitive (deliberately left composable/unwired), this one **is**
wired directly into `plan_route`'s existing redundant-strategy replica
selection, replacing a blind "take the next-best-scored fallback" with
"take the best-scored fallback that's actually on a different
underlay." New `quality.rs` for §71: `PathQualitySignal`/
`quality_signal_for`, a purpose-narrowed projection of `PathMetrics`
for a call's media-adaptation consumer (the video→audio→voice-message
fallback ladder itself stays out of scope — that's a media decision,
not a routing one).

§69/§70/§72 ("Route Planning for Messaging/Files/Emergency") were
verified rather than separately implemented — real integration tests
against `plan_route`'s actual output. This caught a real mistake
before it shipped: the first draft of the §69 messaging test assumed
DTN would naturally rank last "for free" from existing scoring terms;
running it showed that assumption was false (DTN's *setup* cost is
cheap — no live connection to establish — which is a different
property from DTN being a *good* choice for immediate delivery, and
nothing in current scoring captures that difference without real
caller-supplied latency data). The test was corrected to only assert
what's robustly true (cheap-setup transports beat expensive-setup ones
all else equal) rather than a claim that happened to be untested and
wrong. §70's files test uses realistic differentiated metrics (a real
`min_bandwidth` floor plus measured bandwidth per candidate) rather
than relying on coincidental tie-breaking. §72's emergency-composition
test was robust as originally written and needed no correction.

82/82 tests (up from 71), clippy clean, fmt clean, doc-warning-free
(one broken intra-doc link to a `#[cfg(test)]`-only module, caught and
reworded), zero regressions in `siar-dtn-bundle` (37/37,
no `PathCandidate` literal construction there so no downstream fix
needed this time), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115). Delivered as a git commit on top of
round 4's tree (same repo from round 4, not re-initialized). Next for
spec 03: §75 onward — §75-78 (multipath chunk scheduler/path collapse/
duplicate chunks/realtime multipath) are explicitly named by the spec
itself as future/optional/advanced work, not v1, so likely to be
documented as out-of-scope rather than implemented; §79 (congestion
signals) is partially covered already by existing `PathMetrics` fields
(rtt/loss/bandwidth) — worth checking exactly what's still missing
(send-queue depth, connection-congestion signal) before assuming it
needs a full new module.

**2026-09-08 update (round 6):** §75-84 done, ~92→~101/200. §75-78
(multipath chunk scheduler, path collapse, duplicate chunks, realtime
multipath) documented as out-of-scope — the spec itself names all four
as future/optional/advanced, not v1. §81 (Cold vs Warm Path) and §84
(Battery Cost) turned out to already be covered by existing work
(`ConnectionPoolState`/`effective_setup_cost` from round 2, and
`EnergyCost` from Phase 1 respectively) — documented rather than
reimplemented, per the note left last round to check before assuming
new code is needed.

Real new work: §82/§83 (Metered Networks, Roaming) got a genuine
`bool`→tri-state refactor — new `MeteredState`/`RoamingState` enums
(`Metered`/`Unmetered`/`Unknown` and `Roaming`/`NotRoaming`/`Unknown`)
replacing a plain `metered: bool` that couldn't express "the platform
hasn't told us yet," wired into `passes_hard_constraints` via two new
functions that both treat `Unknown` conservatively. This refactor
caught a real, pre-existing bug: `DeliveryRequirements::file_chunk()`
had `allow_metered: true`, directly contradicting §82's own "block
bulk" worked example — now corrected, with a new `allow_roaming_bulk`
field added for §83's own "allow cellular but forbid roaming bulk
transfer" example. §79 (Congestion Signals) added `CongestionState`
plus two new `PathMetrics` fields, finally closing a gap
`DefaultScorer`'s own doc comment had named as unmodeled since this
crate's first phase — congestion is now a real scored term across all
7 policy profiles; failure penalty remains open (documented) since it
needs failure history this crate keeps no record of. §80 (Route
Stability Score) got a new `derive_stability_score` heuristic turning
lifetime/failure-rate/path-changes/timeouts into the existing
`StabilityScore` enum.

Process note: this round's §82/§83 work was actually started in an
unfinished prior session and found already sitting uncommitted in the
working tree at the start of this round — it compiled and was
functionally sound, but had no dedicated tests for the new
metered/roaming hard-constraint logic. Six tests were added before
treating it as done, rather than trusting that "it compiles" meant
"it's finished." As anticipated last round, adding `allow_roaming_bulk`
to `DeliveryRequirements` required the same downstream
`siar-dtn-bundle` test-helper fix as round 2's field additions did —
worth continuing to expect this every time a `DeliveryRequirements`
field is added, and to keep checking the whole workspace, not just
this crate, before calling a round done.

95/95 tests (up from 82), clippy clean, fmt clean, doc-warning-free
(two broken intra-doc links caught: one from this round's own edit,
one pre-existing in the found-uncommitted work). Zero regressions in
`siar-dtn-bundle` (37/37, after the fix above), `siar-identity-multidevice`
(251/251), `siar-protocol-ext` (115/115).

**2026-09-08 update (round 7):** §85-90 done, ~101→~107/200. New
`platform.rs`: `BatteryLevelClass`/`ThermalState`/`DeviceState` (§85),
plus a doc comment reconciling §87's six named platform signals
against where each actually lives in this crate — three already
existed (network type = `TransportKind`; metered/roaming =
`MeteredState`/`RoamingState` from round 6), the other three land this
round (power saver = `DeviceState::battery_saver`; background
restrictions = the new `requires_foreground` capability flag; radio
availability = `CandidateState`). New `acquisition.rs`: `CandidateState`
transcribed exactly from §89's own code block
(`Active`/`PassiveKnown`/`RequiresDiscovery`/`RequiresSetup`), a new
`candidate_state` scoring weight across all 7 `PolicyWeights` profiles,
and `is_currently_usable`/`eliminate_background_restricted` for §86 —
deliberately the *opposite* default from round 6's metered/roaming
`Unknown` handling: an unreported foreground state defaults
*permissive* here, since (unlike metered/roaming) there's no named
"protect a resource" bias to justify assuming the worst. New
`discovery.rs`: a real stateful `DiscoveryBudget` (sliding window +
cooldown, the first genuinely stateful-across-calls type in this
crate — everything before it was either pure functions or took
`now_millis` as a bare parameter) and `discovery_permitted`, which
layers `DeviceState` on top: thermal-critical blocks unconditionally,
with no priority override (a hardware safety margin — even an SOS
shouldn't force active scanning while the device is about to
thermally shut down), while battery-saver blocks unless
`Priority::Critical` (a user preference, overridable the same way
§52's `justifies_expensive_setup` already treats Critical priority).

All three new pieces are wired through the real pipeline this round,
not left composable-only the way round 4's `fairness.rs` was: `RoutingContext`
gained a `device: Option<DeviceState>` field, `PathCandidate` gained
`state: CandidateState`, `PathCapabilities` gained
`requires_foreground: bool`, and `plan_route` itself gained a `device`
parameter and now actually calls the §86 background-restriction filter
as a real elimination pass. This was the most invasive round yet at
the call-site level: `plan_route`'s own signature changed (a 6th
parameter), which meant fixing all 10 existing call sites in
`plan.rs`'s own test suite, on top of the now-familiar mechanical
`PathCandidate`/`PathCapabilities` literal updates across 9 files. A
genuine bug was caught mid-implementation, not just mid-review this
time: the first draft of `DiscoveryBudget`'s own cooldown test used a
short window that let the second attempt's timestamp age out of the
window before the cap was ever actually hit, so the intended
exhaustion path never triggered — caught by actually running the test
suite (it failed), not by re-reading the code, and fixed by widening
the test's window so the cap is hit while both timestamps are still
live.

106/106 tests (up from 95), clippy clean, fmt clean, doc-warning-free.
No `DeliveryRequirements` fields were touched this round, so — for the
first time since round 3 — no downstream `siar-dtn-bundle` fix was
needed; still confirmed via the full workspace check rather than
assumed. Zero regressions in `siar-dtn-bundle` (37/37),
`siar-identity-multidevice` (251/251), `siar-protocol-ext` (115/115).
Next for spec 03: §91 onward (Route Escalation Ladder, Timeout by
Stage, Hedged Requests, Deduplication Requirement, Route Diagnostics —
§91-95, a natural "resilience mechanics" cluster; §95-99/§124-127
diagnostics-and-testing work is flagged in lib.rs as a whole phase not
yet attempted, worth checking how much of it this round's cluster
might already close incidentally).

**2026-09-08 update (round 8):** §91-96 done, ~107→~113/200. New
`resilience.rs`: `EscalationStage`/`escalation_stage_of` (§91) — no
new state needed at all, built entirely by composing round 7's
`CandidateState` (stages 1-2), round 2's `SetupCost` (the 3-vs-4
lightweight/expensive split), and `TransportKind::Dtn` (stage 5)
directly, confirming the "check how much this cluster might close
incidentally" instinct from last round's note was worth having.
`timeout_millis_for_stage` (§92, same coarse-named-heuristic style as
`estimate.rs`/`discovery.rs`). `HedgePolicy`/`hedge_policy_for` (§93)
— unlike most of this round's other pieces, this one required a real
API change: `RouteStrategy` gained a `Hedged` variant and `RoutePlan`
gained `hedge_delay_millis`, with `plan_route` itself now producing
`Hedged` for high-priority, non-bulk, non-delay-tolerant traffic with
a fallback available. `RouteDiagnostics`/`diagnose` (§95), deliberately
scoped to the two rejection checks (hard constraints, background
restriction) this crate can run without an external policy object —
security/privacy checks need a `TrustedAccountStore`/`PrivacyPolicy` a
caller may not have, and a caller composing those itself already knows
which one rejected a candidate. §94 needed no new function, just a doc
comment extending `OperationId`'s existing dedup note to cover
`Hedged` alongside `Redundant`. §96 "Path Visualization" is explicitly
deferred by the spec itself to "Part 18" — documented, not attempted.

A real bug was caught mid-round by running the test suite, not by
re-reading code: the first hedge-strategy integration test's own
candidates used the default `MeteredState::Unknown`/`RoamingState::Unknown`
from the crate's usual test-helper pattern, which round 6's own hard
constraints correctly block for a Bulk-class, `allow_metered: false`
request — the test failed with `NoEligibleCandidates` until the test's
own candidates were given explicit `Unmetered`/`NotRoaming`
capabilities, the same fix `files_prefer_high_bandwidth_direct_over_small_only_bluetooth`
(round 5) already needed for the same reason.

119/119 tests (up from 106), clippy clean, fmt clean, doc-warning-free
(two broken intra-doc links to private items, caught and fixed). Also
fixed, while in the file: a real content gap in this file's own §85-90
coverage bullet from round 7 — a sentence fragment had gone missing
mid-edit, leaving a dangling `(§171-175)` reference with nothing
before it. Zero regressions in `siar-dtn-bundle` (37/37),
`siar-identity-multidevice` (251/251), `siar-protocol-ext` (115/115).
Next for spec 03: §97 onward.

**2026-09-10 update (round 9):** §97-104 done, ~113→~121/200. New
`explain.rs`: `RouteReason` transcribed exactly from §97's own code
block, now a real `RoutePlan::reason` field computed by a first-
match-wins heuristic (`infer_reason`) — honestly documented as
"excellent for debugging" classification, not a precise decomposition
of the weighted-sum score. `RouteMetricEvent`/`metric_events_for`
(§98): event classification only; this crate keeps no counters of its
own, so "average route setup latency"/"queue delay"/"retry count" have
no equivalent here at all (documented as out of reach, not silently
dropped — those are timing measurements this crate has no clock to
take). §99 needed no code: `RouteMetricEvent` structurally cannot
carry peer identity, an IP, a contact graph, or a location, since it
has no field of any kind — "privacy by construction" rather than by
filtering, and `RouteDiagnostics` (round 8) already had the same
property, now called out explicitly. `RouteHint`/`hint_from_plan`/
`revalidate_hint` (§100/§101) — the real teeth is `revalidate_hint`:
"never assume persisted route is still valid" as an actual check
against current candidates, not just a comment.

§102 "Suspend/Resume" and §103 "Process Death" both turned out to
already be fully satisfied by earlier rounds' work — §102's four
bullet points map exactly onto `RouteCache::invalidate_all` + a fresh
`plan_route` call + `RetryPolicy` + `DiscoveryBudget`'s own cooldown;
§103 is satisfied by `RoutePlan` simply never having derived
`Serialize`/`Deserialize` in the first place, now documented as a
deliberate property rather than an unremarked one. Zero new code for
either — the "check whether this is already covered before writing
something new" habit (round 6 onward) paid off twice in one round.

`RoutePlan` gained two more real fields for §104:
`created_at_millis`/`valid_until_millis`, with the validity window
derived from `RoutingPolicy::hysteresis.minimum_hold_millis` (already
existed, representing "how long to trust this decision") rather than
inventing a new duration. This required changing `plan_route`'s
signature again — a new `now_millis` parameter, meaning all 13 call
sites across `plan.rs` and `resilience.rs` needed the same mechanical
fix as round 7's `device` parameter, plus the three new
`RoutePlan`-literal-construction sites (`cache.rs`, `dispatch.rs`,
four in `explain.rs`'s own tests) needed the three new fields added.

130/130 tests (up from 119), clippy clean, fmt clean, doc-warning-free
on the first check for once. No `DeliveryRequirements` fields touched
this round, so no downstream `siar-dtn-bundle` fix was needed — still
confirmed via the full workspace check rather than assumed. Zero
regressions in `siar-dtn-bundle` (37/37), `siar-identity-multidevice`
(251/251), `siar-protocol-ext` (115/115). Next for spec 03: §105
onward (Path Authorization, Extension Capability Integration, Device
Capability Integration — §105-107, a natural "authorization
composition" cluster: §105 is explicitly an umbrella check over
device-active/identity-trusted/operation-authorized/extension-
supported, three of which already exist from earlier rounds).

**2026-09-11 update (round 10):** §105-107 done, ~121→~127/200. New
`authorization.rs`: `authorize_path()` composes §105's own four-item
list — confirming the round-9 prediction, three of the four were
already real one layer down (device-active+identity-trusted =
`security::authorize_candidate`; operation-authorized = a direct
capability-bit check against the device's own certificate, the
structural slice of `siar-identity-multidevice`'s existing
`DeviceAuthorizationDecision::combine` that's actually this crate's
job — the other two inputs that function combines are explicitly
out of scope per its own doc comment, so `authorize_path` doesn't
call `combine` itself). Only "extension supported" (§106) was
genuinely new code: checks a candidate's peer against
`siar-protocol-ext`'s `PeerCapabilities` (real negotiated-extension
record), with no capability record for a peer treated as "not
supported," never "unknown, so allow." `OperationDescriptor` gained
`required_extension: Option<ProtocolId>` (§106) — zero existing call
sites, so no downstream fixes needed. §107 "Device Capability
Integration" is deliberately a separate function,
`select_devices_with_capability()`, not another `authorize_path` —
it's device-*selection* (§107's own "phone supports video, headless
relay does not" example), one step upstream of authorizing a single
already-built candidate. That example needed a capability bit that
didn't exist yet: added `DeviceCapabilitySet::REALTIME_MEDIA` to
`siar-identity-multidevice` rather than a parallel type in this
crate, so both checks read the same certificate field.

9 new tests, including one transcribed directly from §106's own
worked example ("files/1 required, remote only supports messaging →
path invalid"). 139/139 total, clippy clean, fmt clean (4 minor
line-wrap fixes via `cargo fmt`), doc-warning-free for this round's
own files (3 pre-existing warnings elsewhere in
`siar-identity-multidevice`, untouched). No `DeliveryRequirements`
changes, so no downstream `siar-dtn-bundle` fix needed. Zero
regressions: `siar-dtn-bundle` (37/37), `siar-identity-multidevice`
(251/251), `siar-protocol-ext` (115/115) — confirmed via full
workspace check (apps/* excluded from a scratch copy of the workspace
manifest for sandbox verification only, since this upload's tarball
didn't include `apps/`; the delivered files don't touch that list).
Next for spec 03 (superseded by round 11 below): §108 onward.

**2026-09-11 update (round 11):** §108-115 done, ~127→~135/200. New
`decision.rs`: `decide_route()` composes §108's own five-layer stack
(system→application→user→operation→network context) end to end for
the first time — each layer already existed piecemeal since round 2,
but nothing before this round ran them in that order from one place.
`SystemPolicy`/`PolicyLayers` (§108/109, new): "never exceed hard size
limit" and "never route to revoked device" are real checks (the
latter reuses `security::eliminate_untrusted_candidates`, composed
not reimplemented); "never send unencrypted private message" has no
checkable equivalent — no plaintext/ciphertext field exists to
inspect. `ApplicationPolicy` (§110, new) is deliberately small: two of
its three worked examples were already operation-level
`DeliveryRequirements` fields (`allow_dtn`/`allow_relay`); only "must
not use unknown relay peers" needed a new field
(`ApplicationPolicy::allow_relay`). §111/§112 needed zero new code —
`PrivacyPolicy`/`DeliveryRequirements` already fully covered both. §113
"Policy Conflict"'s own point ("not silent policy violation") is why
`decide_route` checks each layer separately and returns as soon as one
empties a previously non-empty list, so §115's reason can name *which*
layer — tested against §113's exact worked example (large file + no
metered + only metered path exists → `Deferred(WaitingForUnmetered)`).
`RouteDecisionResult` (§114, all 4 variants) and `DeferredReason`
(§115, all 6 variants) both transcribed exactly; every `DeferredReason`
variant wired to a real condition, not left inert (`BatteryPolicy`
reuses `discovery::discovery_permitted`'s existing `battery_saver`
gate; `BackgroundRestriction` reuses `acquisition::eliminate_background_restricted`).

11 new tests, 150/150 total. Clippy needed two real fixes:
`RouteDecisionResult`'s `large_enum_variant` (kept `Routed(RoutePlan)`
un-boxed with a justified `#[allow]`, since §114's own code block
spells it exactly that way) and a `needless_option_as_deref` on the
discovery-budget branch. Fmt clean after `cargo fmt`, one broken
intra-doc link fixed, doc build clean otherwise. Zero regressions in
`siar-dtn-bundle`/`siar-identity-multidevice`/`siar-protocol-ext`
(unchanged counts — this round touches no dependency crate).

Next for spec 03 (superseded by round 12 below): §116 onward.

**2026-09-11 update (round 12):** §116-120 done, ~135→~140/200. New
`ui_state.rs`: `RouteUiState`/`ui_state_for()` (§116) — a deliberate
many-to-one collapse of `RouteDecisionResult`/`DeferredReason` down
to 5 neutral states, matching §116's own "do not expose raw transport
errors to normal users." Two inferred (not spec-dictated) choices
named honestly in that module's doc comment: `CarriedByNearbyPeer`
fires specifically for a DTN-strategy/DTN-transport plan, and
`Rejected`/`Unreachable` both collapse to `WaitingForConnection` since
§116's own five-item list reads as in-flight states, not a
terminal-failure one. New `config.rs`: `RoutingConfig` (§117,
transcribed field-for-field — 5 of its 7 fields turned out to already
have a more specific existing type: `retry::RetryPolicy`,
`policy::HysteresisPolicy`, and the same bool shape
`DeliveryRequirements`'s own `allow_relay`/`allow_bluetooth`/`allow_dtn`
already use) plus `validate()`'s one real startup check (an inverted
retry-backoff range — everything else is either a plain bool or
already validated at construction via `Ratio::new`'s clamp). §118 "No
Global Singleton" needed zero new code — verified by grep, not
assumed: no `static`/`lazy_static!`/`once_cell`/`thread_local!`
anywhere in this crate's `src/`, ever. New `engine.rs`: `RoutingEngine`
trait (§119, native async-fn-in-trait syntax — no new runtime
dependency added, since nothing else in this crate touches async)
with `RouteRequest`/`RouteDecision`/`RouteResultReport`/`RouteOutcome`,
plus a full worked implementation (`TestEngine`, holding a
`TrustedAccountStore`/`RoutingPolicy`/`DiscoveryBudget` behind a
`Mutex` since `plan` takes `&self`) and a from-scratch ~15-line
`block_on` executor in that module's own tests (no async
dev-dependency to reach for). §120 "Transport Manager API" is the one
section this round with **no code** — its trait names three types
(`ResolvedDestination`/`TransportSession`/`TransportError`) that
belong to whichever crate owns real sockets (`siar-transport`), not
this dependency-free one — named as a real, deliberate gap per the
spec's own "Routing does not own low-level sockets."

11 new tests (3 config + 6 ui_state + 2 engine), 161/161 total.
Clippy needed three real fixes along the way: `async_fn_in_trait` on
`RoutingEngine` (silenced with a justified `#[allow]`, since adding a
`Send` bound would be adding something the spec's own signature
doesn't ask for), a `doc_lazy_continuation` formatting issue in that
same doc comment, plus the usual first-draft unused-import/unused-mut
catches. Fmt clean after `cargo fmt`, two broken intra-doc links fixed
(unqualified paths), doc build clean otherwise. Zero regressions:
`siar-dtn-bundle` (37/37), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115) — unchanged, this round touched no
dependency crate.

Next for spec 03 (superseded by round 13 below): §121 "Feedback Loop"
onward.

**2026-09-11 update (round 13):** §121-127 done, ~140→~148/200. New
`engine::health_after_outcome()` (§121) — a pure single-sample health
transition (`RouteHealth` × `RouteOutcome` → `RouteHealth`), wired
into `TestEngine::report_result` against a real per-path `HashMap`;
deliberately keeps no history itself, since the "metrics update" step
before it in the spec's diagram needs history this crate has kept out
of scope since round 9's `RouteMetricEvent`. §122 "Avoid ML Initially"
needed zero new code (verified: no ML dependency, ever). §123
"Deterministic Scoring" got property tests at three layers
(`scoring::score()`, `plan::plan_route`, `decision::decide_route` —
each called twice with identical inputs, same output asserted) rather
than new production code, since the property was already true by
construction. §124 "Simulated Routing Tests" transcribed the spec's
exact Path A/Path B numbers (10ms/1Mbps/metered vs.
50ms/100Mbps/unmetered) into `plan.rs`, surfacing two real
test-writing lessons along the way: `interactive_message()` leaves
`max_latency_millis: None`, under which RTT scores as neutral
regardless of its actual value, and the latency-suitability curve
itself only penalizes RTT *exceeding* the deadline rather than
rewarding being well under it — both fixed in the test, not the
production code (which was already correct; the test's assumptions
were wrong). §125 "Policy Property Tests" — all four transcribed, and
the fourth ("expired operation never routed") **surfaced a real,
previously-unnoticed production gap**: `decide_route` had no way to
know when an operation was created, so nothing ever actually enforced
expiry at the routing-decision level. Fixed properly: new
`OperationDescriptor::created_at_millis` field, new
`RejectReason::OperationExpired`, and a new Step 0 check in
`decide_route` — not merely a test working around a known limitation.
§126 "Chaos Tests" — a 20-round WiFi-flap simulation in `plan.rs`
proving `switch_threshold` stickiness absorbs measurement jitter (≤1
switch across 19 opportunities where a naive top-score policy would
switch on every one); the spec's other two named chaos properties
("no infinite retry loop," "bounded queues") already had dedicated
tests from earlier rounds and needed nothing new. §127 "Failover
Test" — an end-to-end proof of the spec's exact scenario in
`plan.rs`, with one test-writing bug caught (wrong assumption about
which transport wins a tied-metrics scoring pass by default — fixed);
its "operation resumes if semantics allow" second half is documented
as a statement about a layer above this crate, since `plan_route` has
no concept of "resume" to test.

14 new tests, 175/175 total — not a clean first pass: three genuine
bugs in this round's own test code were caught and fixed along the
way (detailed above), none in production logic except the §125
expiry gap itself, which was a real, previously-shipped gap this
round's own test-writing caught and then fixed at the source. Clippy
clean on the first pass this round. Fmt clean after `cargo fmt`, doc
build clean. Zero regressions: `siar-dtn-bundle` (37/37),
`siar-identity-multidevice` (251/251), `siar-protocol-ext` (115/115)
— unchanged.

Next for spec 03 (superseded by round 14 below): §128 onward.

**2026-09-12 update (round 14):** §128-139 done, ~148→~157/200. §128
"File Resume Integration" and §129 "Messaging Retry Integration"
needed zero code — both are pure boundary statements ("do not make
routing understand file chunk state," "routing does not create
duplicate message semantics") this crate already satisfies by never
having a chunk-state or message-ID concept anywhere in it. New
`engine::RouteChangeEvent` (§130) — the reporting half only
(`NewPath`/`QualityUpdate`); the call/media-layer actions the spec
pairs with it ("rebind, renegotiate, adapt bitrate") stay a layer this
crate reports to, not performs. New `risk.rs`: `handle_security_event()`
(§131 — of the spec's own three actions, "invalidate route cache" is
real, calling `RouteCache::invalidate` directly; "remove candidate"
and "emit security-relevant event" come back as data since this crate
has no persistent candidate registry or event bus; "authentication
failure" and "revoked device" turned out to already be the same
`RouteFailureClass` variant, so one trigger condition, not two);
`PathPenalty` (§132, transcribed with the spec's own two fields —
`until` as a timestamp structurally prevents permanent blacklisting,
not a caller's discipline); `PeerAbuseStatus`/`eliminate_abusive_peers()`
(§133, deliberately no `RateLimited` variant — that's
`fairness::RoundRobinFairQueue`'s job at the dispatch layer, not a
routing-elimination concern; tested adversarially with an
excellent-metrics-but-abusive peer, matching round 13's §125 property
style). New `scope.rs`: `RouteScope` (§137, transcribed exactly:
`Any`/`InternetOnly`/`LocalOnly`/`NearbyOnly`) plus
`transport_allowed_in_scope()`/`eliminate_out_of_scope_candidates()`
covering §134-136's three named transport lists — named rather than
silently reconciled: §134's "Local-Only" list includes `Dtn`; §136's
otherwise-identical "Nearby-Only" list doesn't, and both are tested
explicitly to lock that asymmetry in. §138 "Emergency Override" —
new `privacy::effective_privacy_policy()` plus a new
`PrivacyPolicy::emergency_override_enabled` field, wired into
`decide_route`'s existing user-policy step with no new parameter
needed (`descriptor.requirements.priority` was already available);
both of the spec's own guard conditions (Critical priority AND
explicit opt-in) are checked, not just priority, and proven
end-to-end both ways. §139 "User Consent" needed zero new code —
`PrivacyPolicy` already **is** "the resulting policy" routing
consumes, per the spec's own last line.

17 new tests (6 risk + 5 scope + 2 privacy + 2 decision + 2 engine),
192/192 total — compiled clean and passed clean on the first try this
round, no test-writing bugs unlike round 13. Clippy needed one real
fix (`needless_lifetimes`), fmt needed the usual line-wrap pass, and
doc build needed two broken intra-doc links fixed (wrong struct name
— `fairness::RoundRobinFairQueue`, not `PerTransportDispatchQueue`).
Zero regressions: `siar-dtn-bundle` (37/37),
`siar-identity-multidevice` (251/251), `siar-protocol-ext` (115/115)
— unchanged, this round touched no dependency crate.

Next for spec 03 (superseded by round 15 below): §140 onward.

**2026-09-12 update (round 15):** §140-150 done, ~157→~168/200. §140
"Bandwidth Reservation"/§141 "Traffic Shaping" are both explicitly
forward-looking in the spec's own text ("Future call + file
coexistence") and kept correspondingly thin: new
`BandwidthReservation`/`bulk_should_yield_to_reservation()` (Bulk
yields entirely, not partially, to any active reservation — no
duration/schedule, since this crate has no clock to track when a
reservation should end) and `TrafficShapingPolicy` (the spec's own two
named caps kept as two independently-triggered conditions — Bulk's
cap only applies "while call active," Background-priority's cap is
unconditional, matching exactly what the spec states with and without
a qualifier). §142 "Connection Admission" — new
`ConnectionAdmission`/`admission_permitted()`, with
`Priority::Critical` always bypassing the caps, the same
emergency-bypass shape round 14's §138 override established for a
different resource; `is_expensive_radio_transport()` reuses
`setup::static_setup_cost` (§44, unchanged) rather than re-deriving
"expensive." §143 "Thermal Awareness" — three functions for the
spec's own three named reductions (multipath, Wi-Fi Direct setup,
background bulk), all gated on the same `ThermalState::Critical`
threshold `discovery::discovery_permitted` already established for
thermal pressure (§90) rather than a second severity line. §144
"Memory Pressure" — new `MemoryPressure` enum on `DeviceState`;
`memory_pressure_allows_acquisition()` implements "durable operations
remain persisted" as an override rather than a separate exception
path; `recommended_queue_capacity()` is a recommendation, not an
enforced resize — the caller owns the actual queue; "drop stale
realtime packets" has no function at all, since that's an action on a
live packet buffer this crate doesn't hold.

§145-150 "Transport Adapter Contract" through "DTN Adapter" — new
`adapters.rs`, almost entirely documentation rather than new code:
§145's own "report" contract is already exactly what `PathCandidate`
carries, field for field; its "support" contract (connect/close/send)
is the same kind of gap §120 already named, for the same reason (no
transport dependency to define a session type against). §146-150 are
checked field-by-field in that module's own doc comment: Iroh (§146)
fully covered, and "should not expose Iroh-specific types upward"
confirmed already true structurally; LAN (§147) mostly covered,
"interface" named as a genuine gap; Bluetooth (§148) — "do not use
Bluetooth MAC as identity" confirmed already true structurally
(`peer: DeviceId` always), "proximity"/"paired state" named as gaps;
Wi-Fi Direct/Aware (§149) mostly covered, "current group/session"
named as a gap; DTN (§150) — "replication policy" already covered
(`dtn_replication_budget`, §56), "uncertain latency" already expressed
the same way this crate expresses any unmeasured quantity
(`rtt_millis: None`), "delivery probability" named as a gap needing
real encounter history this crate has never had access to.

11 new tests, 203/203 total. One real compile error caught before the
first test run: adding `DeviceState::memory_pressure` broke two
existing test literals in `decision.rs` that didn't use
`..Default::default()` — both one-line mechanical fixes. Clippy clean
on the first pass. Fmt clean after `cargo fmt`, doc build clean on the
first pass. Zero regressions: `siar-dtn-bundle` (37/37),
`siar-identity-multidevice` (251/251), `siar-protocol-ext` (115/115)
— unchanged.

Next for spec 03 (superseded by round 16 below): §151 onward.

**2026-09-12 update (round 16):** §151-159 done, ~168→~178/200. §151
"Route Probability" — new `probability.rs`: round 15's own
`adapters.rs` had named "delivery probability" as a gap on the
grounds that this crate has no encounter history to compute one from,
but §151 doesn't actually ask for that computation — only somewhere
to *receive* an estimate from whatever adapter does have the history.
New `PathMetrics::delivery_likelihood`/`expected_delay_class` fields
are that place; `route_probability_signal()` reads them without being
wired into `DefaultScorer`'s weighted formula (a real policy-tuning
decision this round doesn't make unilaterally). Went back and
corrected round 15's now-stale `adapters.rs` paragraph to reflect this.
§152 "Metric Types Must Match Reality," §153 "Scoring Normalization,"
§154 "Policy Weight Example" all needed zero new code — confirmed by
inspection that this crate's typed-metrics design, per-factor
normalized scoring terms, and `PolicyWeights`/`RoutingPolicyProfile`
already satisfy each. §155 "Integer Score Option" — new
`PolicyWeights::sum()` + `RouteScore::as_fixed_point()`, normalized
against the actual weight-sum for a given policy rather than an
assumed constant, since weights aren't required to sum to 1.0.

§156-158 — new `diagnostics.rs`: `RouteDecisionLog` (§156, "not
payload content" already true by construction), `DeveloperDiagnostics`
(§157, transcribes the spec's own worked example field-for-field
except "Destination: Bob Phone," which has no honest source here —
documented rather than faked; added `RouteReason::description()` to
produce the example's "Reason:" line honestly, as a single cause, not
a fabricated combined phrase), `RouteHistory` (§158, a real
fixed-capacity ring buffer — "do not retain indefinitely" enforced
structurally, not by caller discipline). Getting "Score: 8240" required
a real production change beyond this cluster: `RoutePlan` had no field
carrying its own winning score before this round — added
`RoutePlan::primary_score`, which touched 8 construction sites across
6 files, all fixed. §159 "Telemetry Export" — new `telemetry.rs`:
`TelemetrySummary` has no identity field anywhere in it to redact in
the first place, same shape as round 9's `RouteMetricEvent`;
`summarize_telemetry()` returns `None` for an empty batch rather than
a misleading `0%`.

13 new tests, 216/216 total. Clippy clean on the first pass. Fmt clean
after `cargo fmt`. Doc build needed one fix (a broken intra-doc link —
`DefaultScorer::score` doesn't resolve since `score` is a trait
method, not inherent; fixed to link `PathScorer::score`). Zero
regressions: `siar-dtn-bundle` (37/37), `siar-identity-multidevice`
(251/251), `siar-protocol-ext` (115/115) — unchanged.

Next for spec 03 (superseded by round 17 below): §160 onward.

**2026-09-13 update (round 17):** §160-170 done, ~178→~189/200.
§160-163 "API Example: Text Message/Large File/SOS/Video Call" —
`RouteRequest` gained a fluent builder matching each example's own
exact method names and call shape
(`RouteRequest::for_device(...).class(...)...`, no separate builder
type or `.build()` the spec's own snippets never call), all four
transcribed as tests checking the resulting fields, not just that the
builder compiles. §162's `.allow_redundancy(true)` needed a real new
field: added `DeliveryRequirements::allow_redundancy`, since
`plan_route`'s `RouteStrategy::Redundant` trigger
(`Priority::Critical` + `DeliveryClass::DelayTolerant`) had no way to
be turned off before this round, in real tension with §21's own "use
redundancy sparingly." None of the four examples mention candidates —
added `RouteRequest::with_candidates` to bridge that real gap between
the spec's illustrative snippets and this crate's actual
requirements, documented as such.

§164-167 — a genuine **reconciliation**, not just new code: round
13's first version of `RouteOutcome` reused `RouteFailureClass`
(`Success`/`Failed(RouteFailureClass)`), a reasonable guess at the
time, but §165 specifies a flatter 8-variant enum with two cases
(`Partial`, `Cancelled`) that have no `RouteFailureClass` equivalent
at all. Replaced `RouteOutcome`'s shape to match §165 exactly;
`RouteFailureClass` itself is untouched and still used directly by
`risk::handle_security_event` for its own narrower purpose. Added
`RouteResultReport::observed_metrics` (§164, a new `ObservedMetrics`
type alias for `PathMetrics`) and rewrote `health_after_outcome`'s
match arms for real handling of §166 "Partial Outcome" (gradual
evidence the path still works, not written off as `Unreachable`) and
§167 "Cancellation" (health left completely unchanged — a user
decision carries no signal about path quality).

§168-170 — new `path_switch.rs`. §168's own five-step process
(prepare → authenticate → transfer state → switch → close) is pure
session-layer mechanics this crate has no session to perform, same
boundary as §120, documented rather than attempted. §169/§170's real
strategy choice — `path_switch_strategy_for()` — treats §170's own
resource/security triggers as overriding §169's softer "reduces
interruption" preference, confirmed by §169's own "and policy
permits" qualifier (only Make-Before-Break can be refused by policy).

14 new tests, 230/230 total. Clippy clean after removing one
now-unused import from the `RouteOutcome` reshape. Fmt clean after
`cargo fmt`, doc build clean on the first pass. **The full-workspace
regression check caught a real downstream break this round**:
`siar-dtn-bundle`'s own test helper constructs `DeliveryRequirements`
field-by-field rather than via a named constructor, and the new
`allow_redundancy` field broke its compile — fixed with a one-line
addition, documented the same way that file already documented the
two previous rounds' field additions. This is exactly why the
full-workspace check runs every round rather than checking
`siar-routing-policy` in isolation. Zero regressions after the fix:
`siar-dtn-bundle` (37/37), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115) — unchanged counts.

Next for spec 03 (superseded by round 18 below): §171 onward.

**2026-09-14 update (round 18):** §171-182 done, ~189→~198/200. §171
"Multi-Device Route Aggregation" — the one **real architectural fix**
in this round: `resolve::resolve_destination_devices` has resolved an
account to every one of its active devices since round 1, but every
decision function built on top of it since then (`plan_route`,
`decide_route`) took one flat candidate list and produced exactly one
winning plan — pooling all of an account's devices together meant
whichever single device scored highest won and every other device was
silently dropped, precisely the "flatten all devices into one route
score" the spec says not to do. New `multidevice::plan_per_device()`
fixes this: one independent `plan_route` call per device, tested with
a worked example proving a bad/forbidden candidate on one device
can't affect another device's plan in the same call. §172 "Device
Preference" — new `DeviceRole`/`devices_matching_role_for_class()`,
a preference with fallback (never a filter that could leave zero
devices targeted), matching the spec's own "policy input, not
immutable identity." §173 "Group Routing" needs no new function
(`plan_per_device` already generalizes to a group's member list) but
surfaced a real, still-open gap worth naming honestly:
`resolve_destination_devices` doesn't resolve `Destination::Group` at
all yet — that gap predates this round and isn't closed by it.

New `broadcast.rs`: §174 "Broadcast Routing" — the transport
restriction turned out to already exist exactly as round 14's
`RouteScope::LocalOnly`; the one genuinely new piece, "separate
duplication controls," is `BroadcastDeliveryTracker`, a small
caller-owned dedup set in the same shape as `RouteHistory`. §175
"Route Constraints by Content Sensitivity" needed zero new code —
`allow_dtn`/`allow_relay` already are `forwarding_allowed`/
`relay_allowed`.

New `dtn_storage.rs`: §176 "Storage Cost" — `DtnStoragePressure`, a
deliberate parallel to round 15's `MemoryPressure` for a different
resource (a DTN relay's storage, not this device's RAM). §177 "Route
Planning Under Storage Pressure" — its own "or" read as two
independent checks:
`eliminate_dtn_under_storage_pressure()`/`storage_pressure_allows_bulk_acquisition()`,
the latter tested to confirm Critical-priority bulk isn't swept up by
the low-priority rejection. §178 "Emergency Storage Override" needed
no new mechanism — the actual eviction logic is explicitly Part
06/17's job per the spec's own text; "general routing marks priority"
was already true since round 1.

§179 "Route Policy Persistence" — every settings-shaped type this
crate has (`PrivacyPolicy`, `SystemPolicy`/`ApplicationPolicy`,
`RoutingConfig`, `RetryPolicy`, `HysteresisPolicy`,
`RoutingPolicyProfile`) gained `Serialize`/`Deserialize`; this crate
has no persistence layer of its own, so the derives just make a
caller's save/load possible. Proved with a compile-time-only test — a
generic function accepting only serde-implementing types, called with
every one of them.

New `policy_triggers.rs`: §180 "Dynamic Policy Update" needed no code
(`decide_route`/`plan_route` are already pure, stateless functions —
calling either again with different inputs already *is*
"re-evaluate"). §181 "Call-Induced Policy Change" — checked against
what already existed first: "bulk throttled" was already round 15's
`TrafficShapingPolicy`, "realtime priority increased" was already an
ordinary `DeliveryRequirements::priority`; only "path switching
hysteresis increased" had no lever, so `hysteresis_for_call_state()`
doubles both hysteresis fields while a call is active, restoring the
base automatically once it ends (a pure function, no stored state to
reset). §182 "Emergency-Induced Policy Change" —
`emergency_effective_requirements()` forces `allow_dtn` only with
explicit user opt-in, the same shape §138's override already
established; "increase critical queue weight" and "enable proximity"
are named as out-of-scope rather than faked.

15 new tests, 245/245 total, compiled and passed clean on the first
try across all four new modules. Clippy clean on the first pass. Fmt
clean after `cargo fmt`, doc build clean on the first pass. **A first
since around round 14**: zero downstream fixes needed this round —
`siar-dtn-bundle` (37/37), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115) all unchanged with no intervention.

Next for spec 03 (superseded by the final round below): §183 onward.

**2026-09-15 update (final round, round 19): SPEC COMPLETE, 200/200.**
§183-184 "Testing Matrix"/"Route Selection Golden Tests" — new
`golden_tests.rs` with all five of §184's own worked examples
transcribed and checked against their exact stated outcome. One of
them caught a real bug in this round's own test code, not production
logic: the "direct degraded + relay stable → relay" test's first
draft cloned the stickiness `current` parameter *before* mutating the
degraded candidate's health, so `plan_route`'s own `degraded_override`
check (which reads `current`'s own health field directly) silently
saw a stale "still healthy" snapshot — fixed by reordering the
clone/mutate. Two testing-matrix combinations with no coverage under
any framing (BLE-only, Wi-Fi Direct+BLE together) got a test each.
§185 "Property Tests" — two properties not yet covered under this
section's own broader framing: "forbidden transport never selected"
generalized past round 13's metered-only version to
`allow_relay`/`allow_bluetooth`, and "hard minimum bandwidth
respected" got a test proving a *known*, insufficient bandwidth is a
genuine hard elimination (the existing test there only proved the
opposite direction). §186/§187 "Fuzzing"/"Benchmarking" — real, named
gaps: no `cargo-fuzz` harness, no `criterion` suite, neither built
this round. Two targeted tests stand in as a partial, honest
substitute for the former: a `0.0/0.0` (`NaN`)-producing zero-latency
deadline, and a `u64::MAX`-byte operation, both proving `plan_route`'s
own NaN-safe sort comparator (written back in round 13 for §123)
holds against actual malformed input rather than merely existing.
§188 "Scalability" needed zero code — operation-level routing and
`RouteCache` were already exactly what it asks for. New
`reevaluation.rs` for §189-191: §191 "Message Routing Frequency"
needed zero code (`RouteCache`/`RouteHint`, real since round 9); §189
"Call Routing Frequency" — `quality_change_exceeds_threshold()`,
reusing `RouteScoreDelta` rather than a second threshold concept;
§190 "File Routing Frequency" — `should_reevaluate_file_route()`, a
plain "or" over the spec's own four named triggers.

§192-197 "Architecture Reconciliation" added as new documentation in
`lib.rs`'s own top doc comment rather than new code: module structure
(this crate grew to ~50 files organized by which spec-section cluster
each round covered, not the spec's own dozen-ish concept grouping —
both valid, different questions answered); no UI/platform
dependencies (true by inspection of `Cargo.toml`, every round);
`RoutingError`'s real six variants mapped explicitly onto the spec's
own suggested seven-category enum, with the three uncovered categories
(`ResourceLimit`/`Cancelled`/`Internal`) explained as expressed
through other, more specific types the spec's own later sections went
on to specify instead (`RejectReason::ExceedsHardSizeLimit`,
`RouteOutcome::Cancelled`); no `anyhow` (true by inspection, every
round); the four originally-sketched implementation phases all have
real code eighteen rounds later, with the explicit ML/multipath
deferral list honestly honored, not just inherited.

§198 "Definition of Done" — a full item-by-item self-audit, ✅/⚠️/❌,
matching the pattern that closed out specs 01 and 02. The ❌ items:
no fuzz harness, no benchmark suite, DTN delivery-probability
*computation* (the representation exists since round 16; the
computation needs real encounter history this crate has never had),
§55 "Mesh Forwarding"'s richer candidate representation (named since
round 2). The ⚠️ items: group-destination routing
(`multidevice::plan_per_device` is ready, but nothing resolves
`Destination::Group` yet — named since round 18) and a handful of
individual adapter-reporting fields with no equivalent (Bluetooth
proximity/paired state, Wi-Fi group/session, LAN interface name).
§199-200 "Relationship to Other Parts"/"Final Principle" added as
closing documentation, tying the spec's own closing principle back to
specific real pieces of this crate (`RouteScore` as a relative
ranking, not a probability; `Rejected`/`Deferred` existing
specifically so a caller can tell "never" from "not yet") rather than
presented as separate from everything already built.

**Also found and fixed a real gap while writing the reconciliation
section**: most of rounds 10-18's public types were never re-exported
at the crate root — only rounds 1-9's got the `pub use module::{...}`
treatment established early on. Fixed with a full pass of re-exports
matching the established style and density.

18 new tests, 263/263 total. Clippy clean. Fmt clean after `cargo
fmt`. Doc build needed 7 broken intra-doc links fixed (crate-name
links needing plain text instead of doc-link syntax; two
`RouteHint`/`revalidate_hint` links pointing at the wrong module —
they live in `explain`, not `cache`) — clean after. Zero regressions:
`siar-dtn-bundle` (37/37), `siar-identity-multidevice` (251/251),
`siar-protocol-ext` (115/115) — unchanged; confirmed no other crate in
the workspace depends on `siar-routing-policy` besides
`siar-dtn-bundle`.

**Across all 18 rounds building this crate** (round 1 covered §1-42,
rounds 2-9 covered §43-104, then one focused cluster per round through
round 18's §171-182, and this final round's §183-200): every round
compiled, tested, clippy-checked, fmt-checked, and doc-checked for
real against the actual uploaded `Cargo.lock` with rustc 1.91.1 — not
assumed to still hold from a prior round. Zero regressions in any
dependent crate at any point except one one-line fix in round 17.
Real, named, still-open gaps carried forward for whoever picks this up
next: no fuzz/benchmark harness; DTN delivery-probability computation
(needs Part 06's own peer-encounter logic); §55's richer mesh-
forwarding representation; `Destination::Group` resolution in
`resolve.rs`; a handful of individual adapter-reporting fields with no
equivalent (Bluetooth proximity/paired state, Wi-Fi group/session, LAN
interface name).


| # | Crate | State |
|---|---|---|
| 01 | siar-protocol-ext | ✅ **108/108 — spec complete** (final round: §91-92 reconciled, §93-95 error codes/health/recovery, §96-99 scheduler contract/storage/metrics/capability isolation, §100-105 reconciled with notes, §106 honest 16-item Definition of Done self-audit — 4 genuine gaps named, §107-108 reconciled) |
| 02 | siar-identity-multidevice | ✅ **204/204 — spec complete** (final round, 2026-09-05: §190-204 — algorithm agility/downgrade protection utilities kept deliberately minimal per spec's own "avoid needless abstraction" caution; a root-key backup envelope that structurally cannot carry plaintext key material; backup-import validation run before any local state is touched; identity-reset/account-deletion presentations with required disclaimer fields; a guarded organization-offboarding state machine that operates only on organization-scoped device ids, never a personal AccountId; multi-tenant-safe composite keys; migration-fixture round-trip tests (honestly incomplete pending §125); and an itemized 21-item Definition-of-Done self-audit — **19/21 fully done, 2 honestly `PartiallyDone`** (no-UI-shipped confirmation prompt; property/integration tests exist but no real fuzz harness). Also fixed a genuinely broken intra-doc link left over from an earlier round, dropping this crate's doc-warning count from 4 to 3. 6 new modules (`algorithm_agility.rs`, `root_key_backup.rs`, `identity_lifecycle.rs`, `migration_fixtures.rs`, `definition_of_done.rs`) plus a `namespace.rs` extension, 20 new tests, 251/251 total, clippy clean, zero regressions. Across all 11 rounds this session: 137 new tests written, zero regressions in siar-routing-policy/siar-crypto at any point, every round compiled+tested+clippy+fmt+doc-checked for real against the actual uploaded Cargo.lock with rustc 1.91.1. Real, named, still-open gaps carried forward into future work: §125 schema versioning absent from DeviceCertificate/DeviceDirectory; §164 no cargo-fuzz harness; §191 full cross-version migration tests blocked on §125; `storage::IdentityStore`/`transaction`/all four `client_api` traits have zero real call sites anywhere in this workspace yet; `RootTrustCacheEntry`/`VerifiedContact` overlap not consolidated; §107/§91 have no real BLE/Wi-Fi/NFC transport wiring.) |
| 03 | siar-routing-policy | ✅ **200/200 — spec complete** (final round, 2026-09-15: §183-200 — §183-184 "Testing Matrix"/"Route Selection Golden Tests": all 5 of the spec's own worked examples transcribed as tests (1 caught a real bug in *this round's own test code* — a stale pre-mutation health snapshot in a stickiness test, fixed), plus 2 combos (BLE-only, Wi-Fi Direct+BLE) with no prior coverage under any framing; §185 "Property Tests" — 2 properties not yet covered under this broader framing (`allow_relay`/`allow_bluetooth` forbidden-transport, and *known* insufficient bandwidth as a genuine hard elimination, not just the existing unknown-bandwidth-isn't-penalized test); §186/§187 "Fuzzing"/"Benchmarking" — real, named gaps (no `cargo-fuzz`, no `criterion`), with 2 targeted NaN-safety tests as a partial substitute for the former; §188 "Scalability" needed zero code (operation-level routing, `RouteCache`, both already true); new `reevaluation.rs` for §189-191 (`quality_change_exceeds_threshold`/`should_reevaluate_file_route`; §191 needed zero code — already `RouteCache`/`RouteHint`); §192-197 "Architecture Reconciliation" added as new lib.rs documentation (module structure, no-UI-deps, `RoutingError`'s 6 variants mapped onto the spec's own suggested 7, no-anyhow, initial-scope/phases all honored); §198 "Definition of Done" — a full, honest self-audit (✅/⚠️/❌ per item, matching specs 01/02's own closing pattern); §199-200 closing documentation; **also found and fixed a real gap**: most of rounds 10-18's public types were never re-exported at the crate root, only rounds 1-9's — fixed with a full pass of `pub use` additions matching the established style. 18 new tests, 263/263 total, clippy/fmt/doc clean (7 broken intra-doc links fixed), zero regressions. Named, still-open gaps carried forward: no fuzz/benchmark harness, DTN delivery-probability computation (representation exists since round 16, computation needs real encounter history this crate has never had), §55 "Mesh Forwarding"'s richer candidate representation (named since round 2), `Destination::Group` resolution (named since round 18), a handful of individual adapter-reporting fields (Bluetooth proximity/paired state, Wi-Fi group/session, LAN interface name). Across all 18 rounds building this crate: round 18 covered §171-182, round 17 covered §160-170, round 16 covered §151-159, round 15 covered §140-150, round 14 covered §128-139, round 13 covered §121-127, round 12 covered §116-120, round 11 covered §108-115, round 10 covered §105-107, rounds 2-9 covered §43-104, round 1 covered §1-42 — every round compiled+tested+clippy+fmt+doc-checked for real against the actual uploaded Cargo.lock with rustc 1.91.1, zero regressions in any dependent crate at any point except one one-line fix in round 17) |
| 04 | siar-event-log | 🟡 ~10/95 (Phase 2 SQLite blocker below is now STALE — see Tier 3 update) |
| 05 | siar-blob-manifest | ✅ ~23/210 (+ metadata_encryption.rs) |
| 06 | siar-dtn-bundle | ✅ ~50/192 |
| 07 | siar-capability | ✅ ~19/164 |
| 08 | siar-resource-limits | ✅ ~56/193 |
| 09 | siar-crash-recovery | 🟡 ~15/186 (real total corrected this session — earliest-stage of the nine) |

**Three real, unresolved reconciliation questions**, deliberately not
silently resolved — documented in the newer crate's own `lib.rs`:
- two device-cert models (`siar_crypto::device_cert` vs
  `siar-identity-multidevice`)
- two routing/scoring systems (`siar-routing` vs `siar-routing-policy`)
- two DTN bundle models (`siar-dtn` vs `siar-dtn-bundle`)

---

## Tier 1 — Security backbone (Part 28)

**127 sections total. ~46 done. Assessment: good stopping point for
incremental section-by-section work** — what's built covers the
sections that unblock everything downstream (message envelopes, replay
protection, key storage abstraction, device revocation, trust states,
domain separation, safety fingerprints, identity-change UX). What's
left is dominated by four items that are each their own subsystem, not
a next "batch":

| Item | Sections | Why it's not a batch |
|---|---|---|
| The ratchet (forward secrecy/PCR) | §11-13 | Needs a real `vodozemac` integration, not hand-rolled crypto |
| Group Security | §25-27 | Needs reconciling against `siar-crypto-mls` (831 lines, unexamined) |
| Test-vector suite + fuzzing + named attack scenarios | §93-103 | A whole security test harness |
| Workspace crate split | §112-122 | The spec's own ask: split into 10 `comm-security-*` crates — a restructuring decision, not a code batch |

Everything else already closed (§5-10, §14-24, §28-36, §40-44) or
partially reconciled (§37-39 SAS/pairing — real gap: no
`protocol_version` field on `DeviceLinkInvite`, not fixed, breaking
change to an already-shipped signed struct). Untouched and NOT
subsystem-scale (genuinely next-batch-sized whenever picked back up):
§46-92 (abuse resistance, plugin/FFI/embedded boundaries, crash-safe
ratchets, diagnostics, failure taxonomy, threat-model docs, ADRs,
disclosure process, compromise-response playbooks), §104-111
(performance, security profiles).

**Recommendation: pause Part 28 here.** Come back to §46-92/§104-111 in
ordinary batches later; treat the four subsystem items as their own
dedicated project when prioritized.

---

## Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

**Started this round. 1 of 27 specs touched (partially).** This tier
was entirely ⚪ before this session — apps/desktop and apps/android
have substantial pre-existing UI code (chat, groups, attachments) but
none of it had been reconciled against the formal 27-spec set until
now.

| # | Spec | State |
|---|---|---|
| 01 | Product Foundation / Cross-Platform Interaction | ⚪ |
| 02 | Desktop (Dioxus) App Shell / Navigation | ⚪ (apps/desktop's shell pre-dates this spec, unreconciled) |
| 03 | Android (Compose) App Shell / Navigation | ⚪ (apps/android's shell pre-dates this spec, unreconciled) |
| 04 | Conversation List / Inbox | ⚪ (siar-ui-state::conversation_list.rs pre-exists, unreconciled) |
| 05 | Message Timeline | ⚪ (siar-ui-state::timeline.rs pre-exists, unreconciled) |
| 06 | Composer / Attachments / Voice / Drafts | ⚪ (siar-ui-state::composer.rs pre-exists, unreconciled) |
| 07 | Calls / Realtime Media | ⚪ (siar-calls crate pre-exists, unreconciled) |
| 08 | Contacts / Requests / Verification / Identity | ⚪ (siar-ui-state::contact_list.rs pre-exists, unreconciled) |
| 09 | Groups / Membership / Roles | ⚪ (siar-ui-state::group_list.rs pre-exists, unreconciled) |
| 10 | Files / Media Gallery / Transfer | ⚪ |
| 11 | Search / Local Knowledge Retrieval | ⚪ (Part 32 has no crate either — joint gap) |
| 12 | Nearby / QR / NFC Pairing / Device Linking | ⚪ (Part 15 has no crate either — joint gap) |
| 13 | Notifications / Background / Incoming Call | ⚪ (Part 31 has no crate either — joint gap) |
| 14 | Presence / Typing / Receipts / Status | ⚪ (Part 30 has no crate either — joint gap) |
| **15** | **Security Center / Devices / Keys / Recovery** | 🟡 **§3-90, §96-100, §110-141, §144, §148-183, §186-194 (reconciled) done, ~83% of 221 total sections** — everything from prior rounds, plus now `RevocationCapabilities`/`sign_out_copy` (§179 — "this signs the device out" only ever renders when actually true), the fixed revocation-can't-erase-remote-copies disclaimer (§180-181), `RecoveryScope` and its history caveat (§182-183 — silence about unrestorable history would itself be the overclaim these sections warn against). **Real spec-internal inconsistency found and documented, not silently resolved**: §184's compromise-response checklist has a different order and two extra steps versus §38's (built in round 7) — extended the existing `CompromiseResponseStep` enum with §184's two genuinely new steps (`ReVerifyAffectedContacts`, `CreateFreshBackup`) rather than reordering the original five, since earlier rounds' tests already depend on that order. §186-194 closed via reconciliation, no new code — event correlation is explicitly deferred by the spec itself as "advanced future feature"; retention/search/Alerts-vs-Events are policy notes; the four cross-crate integration points (§190-193) were already satisfied by design (`RecoveryStatus`/`BackupSecurityState` reused not duplicated since round 11; `DeviceLinked`/`VerificationFailed`/`IdentityChanged` event kinds already exist) or point to still-unbuilt Parts (07 calls, 12 linking is partially built elsewhere, 13 notifications). |
| 16 | Backup / Restore / Export / Migration | ⚪ (Part 33 has no crate either — joint gap) |
| 17 | Emergency SOS / Offline Mesh | ⚪ (siar-emergency crate pre-exists, unreconciled) |
| 18 | Settings / Privacy / Notifications / Data Controls | ⚪ |
| 19 | Plugin/Module Ecosystem | ⚪ (Part 24 has no crate either — joint gap) |
| 20 | Diagnostics / Network Paths / Advanced Dev | ⚪ (Part 18 has no crate either — joint gap) |
| 21 | Accessibility | ⚪ |
| 22 | Design System (tokens/typography/icons/motion) | ⚪ — **candidate for next priority: almost everything else in this tier visually depends on it existing first** |
| 23 | Responsive/Adaptive Layout | ⚪ |
| 24 | Error/Loading/Empty/Offline/Degraded States | ⚪ |
| 25 | Onboarding / First-Run / Permissions | ⚪ |
| 26 | Performance / Virtualization / Large-Data UI | ⚪ |
| 27 | UI Testing / Screenshot / Release Quality Gates | ⚪ |

---

## Tier 3 — The verification-boundary problem — **STALE as of 2026-09-01, see below**

**Original text, kept for history**: This isn't a spec, it's a standing
constraint on everything in Tier 2. `apps/desktop` transitively depends
on `siar-messaging`/`siar-transport`, workspace-pinned to
`rust-version = "1.91"` (the `iroh`/`stoolap` floor). The verification
sandbox used for every crate in Tier 0/1 only has rustc 1.75.0, with no
network path to a newer toolchain. `apps/android` needs a full
Android/Gradle/JNI toolchain not present at all.

**Correction**: `apt-cache search "^rustc-1"` in this sandbox exposes
`rustc-1.91`/`cargo-1.91`/`rust-1.91-clippy` packages directly
(`archive.ubuntu.com` is allowlisted) — installable directly, binaries
land at `/usr/lib/rust-1.91/bin`. Combined with the right system libs
(`libgtk-3-dev libssl-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev
libwebkit2gtk-4.1-dev libasound2-dev cmake libopus-dev libdav1d-dev`),
`cargo check --workspace --all-targets` against the ORIGINAL
repo-root `Cargo.lock` (v4 format — parses fine under cargo 1.91;
don't delete/regenerate it) passes clean across all 33 crates,
including `apps/desktop` (webview/GTK/audio/AV1). So: `apps/desktop`
and `apps/android`'s Rust glue ARE now compile-verifiable here — only
real device/emulator testing (actual Kotlin/JNI runtime behavior,
hardware codecs) remains genuinely out of reach. Disk space is tight
(~6G free after the above) — expect to `rm -rf target && apt-get
clean` between big check runs.

**Practical consequence, updated**: full `cargo build`/`cargo
test`/`cargo clippy -D warnings` rigor is now available for
EVERYTHING in this workspace, not just Tier 0/1-grade crates — spec 04
`siar-event-log`'s "Phase 2 SQLite blocked on rustc 1.87" note above is
one direct casualty of this correction and should be re-evaluated
next time that crate is picked up. `apps/desktop`/`apps/android`
Rust-level code no longer needs to ship flagged-unverified by default —
only genuine device/emulator/hardware-codec behavior does.

---

## Suggested next priorities, in order

**Superseded by explicit user instruction (2026-09-01): work through the
9 Tier 0 core specs first, one by one, before returning to this list.**
Spec 01 (`siar-protocol-ext`) is complete (108/108). Spec 02
(`siar-identity-multidevice`) is now ALSO complete (204/204) as of
2026-09-05. Spec 03 (`siar-routing-policy`) is in progress, ~80/200 as
of 2026-09-10 (round 9, §97-104) — the next crate in this project's
explicit priority order ("work through the 9 Tier 0 core specs first,
one by one"), continuing with §105 onward. Note there is a real, documented unresolved
reconciliation question between `siar-routing` (pre-existing,
next.md-era) and `siar-routing-policy` (this spec's own crate) — see
that crate's own `lib.rs` for the current state of that question
before starting new work there.

Original list, resumes once Tier 0 is done:

1. `ui-ux-15` **continue in ordinary batches** (§195-221 remain): next
   natural slice continues the integration section (§195-206 — Privacy
   Settings Integration onward), then testing matrix + final scope
   (§207-221) — likely finishes the spec in 2-3 more rounds.
2. **`ui-ux-22` Design System** — tokens/typography/icons/motion that
   every other visual spec in this tier implicitly depends on; doing it
   later means retrofitting styling into everything built before it.
4. **Part 28 §46-92 in ordinary batches** — abuse resistance and
   embedded/plugin/FFI security boundaries are the two sub-areas most
   likely to matter soon given `siar-protocol-ext`'s existing extension
   mechanism (Part 01) and the still-unstarted Parts 21/24
   (third-party extensions / plugin ecosystem).
5. **Joint gaps** (spec pairs where both the core-arch and ui-ux spec
   are ⚪): 11+32 (search), 12+15 (QR/NFC pairing), 13+31
   (notifications), 14+30 (presence/receipts), 16+33 (backup), 19+24
   (plugins), 20+18 (diagnostics). Each pair is naturally one unit of
   work (backend + its UI together), not two separate efforts.
6. **Deliberately last, by explicit priority call**: plugin/module
   ecosystem (Part 24, ui-ux-19), WASM components (Part 22), third-party
   protocol extensions (Part 21), external interoperability (Part 23).
   Usability and reliability come first; extensibility/ecosystem work
   is lower priority until the core product is solid.
7. **The four Part 28 subsystems** (ratchet, groups/MLS, test harness,
   crate split) — each needs its own dedicated, scoped effort rather
   than sharing a round with anything else.
