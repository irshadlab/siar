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

**siar-event-log — round 1 of Phase 2, 2026-09-19**: the crate's own
`lib.rs` used to list §19's SQLite-class backend as explicitly not
attempted ("§92's own Phase 2, a separate, larger piece of work"). It
now is: new `stoolap_store.rs` — `StoolapEventStore`, a real
`EventStore` backed by `stoolap` (independent of `siar-storage`'s own
unrelated `plan.md`-era schema, on purpose — see that module's own doc
comment). Schema follows §57 conceptually, adapted to `stoolap` 0.4.0's
actual API (no blob column type — ids/payload go in as TEXT/base64,
confirmed the same way `siar-storage`'s own doc comment already
confirmed it for its schema) plus a `stream_heads` table so §12's
optimistic-concurrency check is an indexed point lookup, not a
scan/count. §22 integrity: every row carries a blake3 checksum over its
own identifying/content fields, verified on every read
(`EventStoreError::Corrupt` on mismatch — two new error variants added,
`Backend`/`Corrupt`, alongside the existing two). §11 atomic append:
`stoolap`'s own literal-SQL `BEGIN`/`COMMIT`/`ROLLBACK` (matching
`siar-storage::message_repo`'s own proven pattern) plus a process-local
`Mutex` around the whole check-then-write critical section — the same
conservative single-lock strategy `InMemoryEventStore` already uses,
chosen explicitly because `stoolap` is, per `siar-storage`'s own module
doc, "days old as a public release" with unproven concurrent-writer
isolation. Two small real gaps found and fixed in the existing Phase-1
types while wiring this: `EventId`/`CorrelationId` had no way to get
their wrapped `Uuid` back out (needed for any TEXT-column round trip at
all) — added `as_uuid`/`from_uuid`, mirroring `siar-domain`'s own
newtype convention; `StreamId` was missing the `from_bytes` half of its
existing `as_bytes`. 7 new tests (idempotency, concurrency-conflict
all-or-nothing, stream/log read ordering, a tampered-checksum-is-
detected test, and one real close-and-reopen-the-same-file durability
test — an actual `std::fs` round trip, not simulated) — 17/17 total.
Clippy clean, fmt clean, one broken intra-doc link found and fixed
(`[Uuid::to_string]` isn't a real path — `Uuid::to_string` comes from
`Display`, not an inherent method). Zero regressions in
`siar-crash-recovery` (50/50) and `siar-identity-multidevice` (256/256)
— unchanged; `siar-dtn-bundle` (43/43) — unchanged; confirmed these are
the only three crates depending on `siar-event-log`. Verified against
the actual uploaded `Cargo.lock` with rustc 1.91.1 (installed fresh
this round via the `rustc-1.91`/`cargo-1.91` apt packages Tier 3
already names — the sandbox that ran this round started with no Rust
toolchain at all, not even 1.75; nothing here was assumed to still hold
from a prior session).

Real, named, still-open gaps carried forward: no schema-version column
on `stoolap_store`'s tables (§9/§62 — the first breaking schema change
has no upcasting story yet); no outbox table (§14, Phase 5); no
projection/checkpoint machinery (§16-18, Phase 4); no snapshots (§38-39,
Phase 7); no retention/compaction (§40); no crash-injection harness
(§84 — only same-process close/reopen, not an actually-killed process);
no property/fuzz tests (§83/§85); `read_stream`/`read_log` are not yet
exercised by any real caller anywhere in this workspace (same
zero-call-sites gap `siar-identity-multidevice`'s own storage traits
already carry). Suggested next slice: Phase 3 (wire a real domain —
messaging is the obvious first consumer per §33/§78) or Phase 4/5
(projections + outbox), rather than jumping straight to Phase 7's
snapshotting before anything produces enough events to need it.

**siar-event-log / siar-messaging — round 1 of Phase 3, 2026-09-19**:
answering the standing "phase-wise vs. section-wise" question first —
this round deliberately kept doing what round 1 of Phase 2 already
did: implement by the spec's own §92 phase grouping, not by numeric
section order, then record section coverage in prose afterward for
audit purposes. Reason: a phase's sections are already the spec's own
verdict on what constitutes one coherent, shippable unit — Phase 2's
own list (§11 atomic append, §12 concurrency, §19 backend, §22
integrity, §57-58 schema/indexes) spans five non-adjacent sections that
only compile and test together as a whole; section-order would have
meant landing an `events` table with no transaction wrapping it, or a
concurrency check with no schema to check against. The cost is that
"§N done" isn't a running left-to-right counter — mitigated here (as
last round) by naming exactly which sections each phase actually
covered.

Phase 3 itself ("messaging, files, identity integration"): identity
turned out to already be real, done in an earlier session before this
one (`siar-identity-multidevice::audit_log` — §35's five named events
plus three extras, 8 `EventTypeId` tags 1-8, full postcard round-trip
tests) — found, read, and left alone rather than redone. This round
added the other real gap: §33 Messaging Events, as a new
`siar-messaging::events` module in the exact same shape (`EventTypeId`
constants — 100-108, deliberately ranged away from identity's 1-8,
with an honestly-named gap that nothing enforces that convention
across crates yet; a `MessagingEvent` enum with all nine named
variants as their own `V1` schemas per §9, not `siar_storage::
StoredMessage` reused; `into_new_event`/`decode_messaging_event`
mirroring `audit_log`'s own `into_new_event`/`decode_audit_payload`).
One real design difference from identity's version, worth recording:
`audit_log::IdentityAuditPayload::origin` safely derives
`LocalDevice`/`System` from the payload alone, because every identity
operation it covers has exactly one local actor or none; messaging
can't do that — `MessageReceived` is definitionally about a remote
peer's message, so `MessagingEvent::into_new_event` takes `origin:
EventOrigin` as an explicit caller-supplied argument instead of
inferring it. 6 new tests, 15/15 total in `siar-messaging::events`
plus the crate's pre-existing 9, clippy clean, fmt clean; the one doc
warning `cargo doc` reports for this crate (`service.rs`'s own
`[siar_crypto::mailbox_token::MailboxTokenSecret]` broken link) predates
this round and is left as-is — not this change's to fix. No crate in
this workspace currently depends on `siar-messaging`, so no dependent
regression check was possible or needed. Verified against the actual
uploaded `Cargo.lock` with rustc 1.91.1.

Real, named, still-open gaps carried forward: §34 File Events (the
`siar-blob-manifest` equivalent of this round's messaging work — not
started); no real call site anywhere yet actually calls
`EventStore::append` with either identity's or messaging's
constructors — both crates only *construct* `NewEvent`s, matching each
crate's own stated "policy layer, not I/O layer" design, but that means
Phase 3's own promise ("integration") is honestly only half done until
`MessageService`/`GroupService` (which already own `Arc<Database>` /
transport) actually call `append` from a real send/receive code path;
no shared cross-crate event-type-id registry (the informal
"pick-a-block-per-domain" convention two crates now follow, undocumented
anywhere both of them can see).

**siar-blob-manifest — round 2 of Phase 3, 2026-09-19 (same session)**:
completed Phase 3's third and last named domain, §34 File Events, in
the same shape as the other two — new `events.rs`: `EventTypeId`
200-208 (the third block in the informal per-domain-range convention
noted above — messaging's own round already named this gap; this round
doesn't fix it, just doesn't collide with it either), a `FileEvent`
enum with all nine names from §34 (`TransferCreated` through
`BlobVerified`), lining up one-for-one with the crate's own
pre-existing `TransferState`/`TransferEvent` state machine
(`transfer_state.rs`) without merging into it — deciding a transition
and recording it stay separate, matching this crate's own established
"types only, no I/O" scope. One real gap found and fixed while wiring
this: nothing in the crate could identify *a transfer* — `ManifestId`
identifies content, not a directed offer of it, and the same manifest
can legitimately be re-offered as two different transfers — so a new
`TransferId` newtype was added to `ids.rs` (same shape as
`ManifestId`/`LogicalAttachmentId`). `BlobVerified` is modeled as a
verification outcome, not a state-machine state, since
`verify_complete_blob` can run either before or after a transfer's own
`Completed` transition. 6 new tests, 37/37 total in the crate; clippy
clean; fmt clean (one rustfmt diff applied); docs clean, zero warnings.
Zero regressions in the two real dependents, `siar-crash-recovery`
(50/50) and `siar-dtn-bundle` (43/43). Phase 3 is now done across all
three of its named sub-items (§33/§34/§35); the two gaps named above —
no real `EventStore::append` call site anywhere, no shared event-type
registry — now apply identically to all three domains, not just
messaging.

**siar-event-log / siar-messaging — closing the "no real caller" gap,
2026-09-19 (same session)**: new `siar-event-log::retry` —
`append_with_retry`, the optimistic-concurrency retry loop (blind
retry with the SAME `NewEvent` on a `ConcurrencyConflict`, bounded by
`max_attempts`, safe because §11's atomicity means a rejected append
leaves no partial trace to confuse §24's idempotency check on retry) —
domain-agnostic on purpose, living in `siar-event-log` itself rather
than duplicated per-domain, since `append`'s own contract (§21) is
deliberately simple and pushes retry-on-conflict to the caller. **A
real bug was caught and fixed by this module's own tests before it
shipped**: the first version reported the *corrected* version guess in
its returned `ConcurrencyConflict` error instead of the version that
was actually attempted — wrong ordering of two assignments, invisible
by inspection, caught immediately by
`exhausting_retries_returns_the_last_conflict_seen` failing with a
left/right mismatch. 21/21 tests in `siar-event-log` (4 new), clippy/
fmt/docs clean.

Wired into `siar_messaging::MessageService`: a new optional
`event_log: Option<Arc<dyn EventStore + Send + Sync>>` field plus a
`with_event_log(self, ...) -> Self` consuming builder — deliberately
NOT a new `new(...)` argument, so `apps/cli`'s own existing call site
(not visible or editable from this sandbox — `apps/` wasn't part of
the uploaded `siar-crates_tar.gz`) keeps compiling unchanged against
this crate's real `new(...)` signature. Three real call sites now
exist, the first anywhere in this workspace: `send_text` records
`MessageCreated` + `MessageQueued` (both true by the time `outbox.
enqueue` returns); `handle_incoming`'s new-message branch records
`MessageReceived` (`RemoteDevice(envelope.sender)` — the message's
own sender, not us); its `DeliveryAck` branch records
`MessageDelivered` (`RemoteDevice(envelope.sender)` again — the ACK's
sender is the original recipient confirming receipt, so the fact
originates with them even though it's *our* outbound message being
confirmed). Append failures are logged (`tracing::warn!`) and
swallowed, not propagated — `siar-storage`'s tables remain this
crate's actual system of record; making the event log authoritative
enough that its own failures should fail a send/receive call is
Phase 4/5 territory, not this round's, and is named as such in the new
code's own doc comments. New end-to-end test
(`send_text_round_trip_records_the_full_04_event_log_history`) reuses
the existing real-QUIC-over-loopback `Node` harness (every `Node` now
always carries its own `InMemoryEventStore`, wired via
`with_event_log`, confirming by construction that every *existing*
test in the file — none of which look at the event log — still passes
unmodified, i.e. the wiring really is additive) and asserts on the
actual recorded event log contents on both Alice's and Bob's side
across a real send → receive → ack round trip, not a unit-test
simulation of one. 26/26 tests in `siar-messaging` (15 unit + 11
integration, one new integration test), clippy clean, fmt clean, docs
clean (the one pre-existing unrelated broken-link warning from an
earlier session is untouched). Re-verified zero regressions in every
crate that depends on `siar-event-log` after this change:
`siar-crash-recovery` (50/50), `siar-dtn-bundle` (43/43),
`siar-identity-multidevice` (256/256), `siar-blob-manifest` (37/37).

Real, named, still-open gaps: identity's and files' own event catalogs
remain construct-only — no `append` caller for either yet, same gap
messaging just closed for itself only; `MessageEdited`/
`MessageDeleted`/`ReactionAdded`/`ReactionRemoved`/`MessageRead` have
no call sites because `MessageService` has no editing/deleting/
reacting/read-receipt-processing features to hang them off yet (the
`ReadReceipt` wire variant is explicitly a stub already, unrelated to
this round); no shared cross-crate event-type-id registry, still.
Suggested next: Phase 4 (projections/checkpoints/rebuild — the event
log finally has a real producer to project from) or Phase 5 (outbox/
effects/retry/recovery, which could plausibly subsume/replace
`siar-storage`'s own existing outbox for messaging specifically, a
real architectural question worth deciding deliberately rather than
drifting into).

**siar-event-log / siar-messaging — Phase 4, 2026-09-19 (same session,
Claude's own choice — the user explicitly deferred "do best
technically")**: new `siar-event-log::projection` — §16 `Projection`
trait (`apply`/`reset`, the latter existing specifically so §16's
"rebuildable" requirement is a real trait method, not a hope),
§17 `ProjectionCheckpoint` (verbatim field names/types),
`ProjectionCheckpointStore` trait + `InMemoryCheckpointStore` (same
"real, tested, not durable yet" status `InMemoryEventStore` itself had
before `stoolap_store` existed), and `ProjectionRunner::catch_up` — the
pull-based runner: loads a checkpoint, replays from it (or from offset
0 on first run OR on a `projection_version` mismatch, which also
triggers `reset` — §16's "versioned"/"rebuildable" made structural,
not just documented), applies each event in log order, saves an
updated checkpoint per batch. Named honestly, in the module's own doc
comment: §16 "deterministic"/"idempotent" are NOT enforced by this
module — properties of what a `Projection` implementation's own
`apply` does, unverifiable from outside; and §18 "read-your-writes"
("critical projections update in the same transaction as the event")
is NOT what `catch_up` does — it's a separate call after `append`
returns, not inside `stoolap_store`'s own transaction (doing that for
real would mean coupling `EventStore::append` to a specific registered
projection set, undoing Phases 1-3's own backend/domain-neutral
design) — what this module actually gives a caller is "call `catch_up`
synchronously right after append returns," sufficient for §18's own
single-process example but not a stronger guarantee than that. 6 new
tests (fresh catch-up, true no-op when already caught up, resumes from
checkpoint rather than replaying everything, respects a small
`batch_size` across multiple internal reads, a version bump actually
triggers full reset+replay, checkpoints for different projection ids
never cross over) — 27/27 total in `siar-event-log`, clippy/fmt/docs
clean.

`siar_messaging::conversation_summary` (new `projections.rs`) is the
first real `Projection` anywhere in this workspace, and §16's own
worked example (`conversation_summary` is literally one of that
section's seven named views) made real end to end: message_count,
last_message_id, last_activity_at, per conversation — deliberately NOT
duplicating `siar-storage`'s own message content/delivery-state tables
(those already exist and are durable; this is a from-scratch
demonstration of the new projection architecture against a genuinely
useful view, not a redundant second copy of what already works). Only
`MessageCreated`/`MessageReceived` bump `message_count` — every other
of the nine §33 events still updates `last_activity_at` without
double-counting. Filters `apply` to messaging's own `EventTypeId` range
(100-108) before decoding anything, skipping — not erroring — events
outside it: a real, not hypothetical, need, since `catch_up` calls
`read_log`, which spans every stream on whatever `EventStore` it's
given, messaging or not. `MessageService::with_event_log` now also
wires a `ConversationSummaryProjection` + its own
`InMemoryCheckpointStore` automatically (no separate opt-in — wanting
the event log at all means getting a live summary view for free); a
new `MessageService::conversation_summary(conversation_id)` is the
query side; `record_messaging_event` calls `catch_up` synchronously
right after every successful append, best-effort (logged, swallowed,
same non-authoritative scope `append` itself already has — see that
method's own doc comment for why). 4 new unit tests in `projections.rs`
plus 2 new end-to-end tests reusing the real-QUIC `Node` harness (every
`Node` now always carries a `ConversationSummaryProjection` internally
via `with_event_log`, confirming existing tests — none of which look at
it — still pass unmodified) — one repeats the full send→receive→ack
flow asserting on raw event-log contents (already existed from last
round), the new one
(`conversation_summary_reflects_sends_and_receipts_without_any_manual_
catch_up`) asserts on `conversation_summary()` directly through
ordinary `send_text`/`handle_incoming` calls only, with no manual
`catch_up` anywhere in the test — proving the automatic wiring, not
just the projection logic in isolation. 31/31 tests in `siar-messaging`
(19 unit + 12 integration), clippy clean, fmt clean, docs clean (same
one pre-existing unrelated warning, still untouched). Needed a new
`async-trait` dependency in `siar-messaging`'s own `Cargo.toml`
(already a workspace dependency, just not previously used by this
crate) for `#[async_trait] impl Projection for
ConversationSummaryProjection`. Ran into a real sandbox disk-space
exhaustion mid-round (`target/debug/incremental` had grown to 1.4 GB
across this session's many rebuilds) — cleared incremental build
artifacts, not source; nothing about the actual deliverable was
affected, but noted here since it's the kind of thing that would look
like a mysterious build failure without this line explaining it.
Re-verified zero regressions in every crate depending on
`siar-event-log` after this change: `siar-crash-recovery` (50/50),
`siar-dtn-bundle` (43/43), `siar-identity-multidevice` (256/256),
`siar-blob-manifest` (37/37).

Real, named, still-open gaps: `conversation_summary`'s own state isn't
durable (in-memory only, same as its checkpoint store) — a restart
loses it, rebuildable only by a full replay of whatever the
`EventStore` itself durably kept (which IS durable, via
`stoolap_store` — only the projection's own materialized view isn't);
no durable `ProjectionCheckpointStore` exists yet either, following the
same pattern `stoolap_store` set for `EventStore` itself; identity's
and files' own event catalogs still have no projection built against
them; the "read-your-writes" gap named in this module's own doc
comment (synchronous-call-after, not same-transaction) is real and
would need a bigger `EventStore::append` redesign to close for good,
not attempted. Suggested next: either build a durable
`StoolapCheckpointStore`/durable projection backing (closing the
"restart loses it" gap for real) or move to Phase 5 (outbox/effects/
retry/recovery), revisiting the `siar-storage`-outbox architectural
question named in the previous round's own gap list before building a
second, possibly-conflicting one.

**siar-messaging — a section-wise audit pass over spec 04 §5-15,
2026-09-19 (same session, user's own explicit request this round: read
sections 5-15 in order, implement whatever's genuinely missing, leave
whatever's already done, rather than continuing strictly phase by
phase)**: this pass is worth recording as its own methodology data
point, not just its findings. §5 (streams), §6 (local offset), §7
(event origin), §10 (append-only), §11-12 (atomic append/concurrency),
§13 (local-first ordering), and §15 (log isn't a job queue) were ALL
already satisfied — most as a side effect of Phase 1/2 work done
before "Phase" was even the organizing word for it. Left alone, per the
user's own instruction, except for adding an explicit §13 confirmation
this round (`send_text`'s existing enqueue → record-event →
network-send ordering already matched §13's own diagram; it just
wasn't named as such anywhere in the code — now it is, in a comment,
with no logic changed). That leaves exactly two sections where reading
the spec's own text line-by-line surfaced something phase-grouping had
genuinely walked past:

§8 "Correlation and Causation" — `EventEnvelope.correlation_id`/
`causation_id` existed as fields since Phase 1 but were hardcoded to
`None` at literally every call site in every domain crate; the feature
existed in the type system and nowhere else. Built a real mechanism:
`MessageCorrelation` (a new private struct in `service.rs`) — one row
per `MessageId`, tracking the shared `CorrelationId` for that message's
whole lifecycle and the `EventId` of whichever event was most recently
recorded for it, so the next one's own `causation_id` has something
real to point at. `MessagingEvent::into_new_event`'s signature changed
to take `event_id`/`correlation_id`/`causation_id` as real parameters
instead of generating an `EventId` internally and hardcoding the other
two to `None` — a deliberate breaking change to this crate's own
internal API (nothing outside this crate calls it), fixed at all
call sites in the same round (both `events.rs`'s own unit tests and
`projections.rs`'s). `send_text` now starts a correlation chain at
`MessageCreated` and continues it at `MessageQueued` (whose
`causation_id` is `MessageCreated`'s real `EventId`); `handle_incoming`
starts a fresh chain at `MessageReceived` (nothing preceded it locally)
and continues the ORIGINAL send-side chain at `MessageDelivered` by
looking `acked_message` up in the same registry. Named honestly, in
the registry's own doc comment: in-memory only, so a process restart
between `send_text` and a later `DeliveryAck` for the same message
loses the chain — `MessageDelivered` then gets `None`/`None` rather
than a wrong link, which is the correct degradation, not a bug, but is
real and worth knowing.

§9 "Versioned Event Schemas" — found a real latent bug, not just a gap:
`decode_messaging_event` (and, unfixed this round, `siar-blob-manifest`
and `siar-identity-multidevice`'s own equivalents) ran
`postcard::from_bytes` straight against the payload with zero regard
for the STORED `schema_version` sitting right next to it in the
envelope — exactly the anti-pattern §9 names by name ("do not silently
reinterpret old bytes using changed Rust structs"), since postcard's
own enum encoding is positional/discriminant-based: adding, removing,
or reordering a variant in `MessagingEvent` would have silently
decoded old rows as the wrong variant instead of failing loudly. Fixed
for messaging (files/identity have the identical bug, named as an open
gap below — not fixed this round, this pass stayed inside `siar-
messaging` as the one crate with an actual multi-event workflow to
test correlation against): `decode_messaging_event` now takes
`schema_version: u16` as a real parameter, matches on it explicitly
(`1 => decode`, anything else → `MessagingEventDecodeError::
UnsupportedVersion`), and a new `CURRENT_MESSAGING_EVENT_SCHEMA_VERSION`
constant replaces the magic `1` `into_new_event` used to write. New
test asserts a fabricated "future" schema version is rejected even
though the CURRENT struct shape could easily have decoded those exact
bytes — proving the gate actually gates, not just that decoding still
works.

Also closed, as originally planned but not reached last round: a real
§14 "Transactional Outbox" resilience test (`send_text_survives_a_
completely_broken_event_log`) — an `EventStore` that fails on literally
every call (`AlwaysFailingEventStore`), wired into a fresh
`MessageService` built from the SAME real `siar-storage` repositories
an existing `Node` already set up, proving `send_text` still persists
the message, still returns `Ok`, and only `conversation_summary`
(which lives entirely inside the failing store) comes back empty. This
is the honest, achievable half of §14 for this workspace: real
single-transaction atomicity across `siar-storage`'s outbox and
`siar-event-log`'s own store isn't attempted (they're two separate
`stoolap::Database` instances — see `stoolap_store`'s own doc comment),
only that a total failure in one doesn't take down the other.

9 new/changed tests total (2 correlation, 1 schema-rejection, 1 outbox
resilience, plus fixing 5 existing call sites for the new
`into_new_event`/`decode_messaging_event` signatures) — 34/34 in
`siar-messaging` (21 unit + 13 integration), clippy clean, fmt clean,
docs clean (same one pre-existing unrelated warning, still untouched).
Re-verified `siar-event-log` itself is unaffected (27/27, correctly,
since every change this round was internal to `siar-messaging`) and
zero regressions in the crates depending on `siar-event-log`:
`siar-crash-recovery` (50/50), `siar-dtn-bundle` (43/43),
`siar-identity-multidevice` (256/256), `siar-blob-manifest` (37/37). No
other spec (01/02/03) needed touching this round — checked, and none
of this pass's two real gaps (§8, §9) reach outside `siar-event-log`/
`siar-messaging`'s own territory.

Real, named, still-open gaps: `siar-blob-manifest::decode_file_event`
and `siar-identity-multidevice::decode_audit_payload` have the
identical §9 schema-version bug `decode_messaging_event` just had —
found, not fixed, this round (scope stayed inside messaging, the one
crate with a real workflow to prove correlation against); §14's real
architectural gap (two separate databases, no cross-store atomicity)
remains exactly as named last round, now with a test proving the
practical consequence is contained rather than just asserting it in
prose. Suggested next: apply the same §9 fix to the other two domain
crates (small, mechanical, now that the pattern is proven), or return
to Phase 5/durable-checkpoint work named at the end of the previous
round.

**siar-blob-manifest / siar-identity-multidevice — closing the §9 gap
everywhere, 2026-09-19 (same session)**: the mechanical follow-through
named at the end of the previous round. Same fix, same shape, in both
remaining domain crates: `decode_file_event` and `decode_audit_payload`
now take `schema_version: u16` as a real parameter and reject anything
they don't recognize (`FileEventDecodeError`/`AuditPayloadDecodeError`,
both `UnsupportedVersion`/`Malformed`) instead of trusting the current
Rust shape unconditionally; `into_new_event` in both now writes a real
named constant (`CURRENT_FILE_EVENT_SCHEMA_VERSION`/
`CURRENT_IDENTITY_AUDIT_SCHEMA_VERSION`) instead of a bare `1`. Neither
crate's `into_new_event` gained §8 correlation/causation wiring —
deliberately: neither has a real multi-event workflow with an actual
`EventStore::append` caller yet (per each crate's own `lib.rs`/module
doc), so there's nothing real to correlate against; building that
speculatively, ahead of a caller, was named explicitly in
`siar_blob_manifest::events`'s own new doc comment as future work once
one exists, not attempted here. One new rejection test per crate,
matching `siar_messaging::events`'s own. 38/38 in `siar-blob-manifest`
(1 new), 257/257 in `siar-identity-multidevice` (1 new), clippy clean
in both, fmt clean in both (no diff on `--check` in either — both
edits were already formatted correctly on the first pass), docs clean for blob-manifest; identity-multidevice shows
3 pre-existing unrelated warnings in `directory.rs`/`recovery.rs`/
`state_transport.rs` (none in `audit_log.rs`, none introduced this
round). Checked both crates' real dependents: `siar-crypto` (68/68)
and `siar-routing-policy` (285/285) for identity-multidevice;
`siar-crash-recovery` (50/50) and `siar-dtn-bundle` (43/43) for
blob-manifest — zero regressions anywhere. Also re-confirmed
`siar-messaging`/`siar-event-log` themselves are unaffected (21+13/21+13
and 27/27 respectively), since nothing in this round touched either.
Hit the same disk-space wall as two rounds ago mid-session (`target/`
had regrown past 15 GB across this long session's many separate crate
builds) — this time a plain `rm -rf target/debug/incremental` wasn't
enough, so a full `cargo clean` was needed, trading a slower first
rebuild for headroom; nothing about any deliverable was affected.

§9 "Versioned Event Schemas" is now consistently handled everywhere
it applies in this workspace — all three domain event catalogs
(messaging, files, identity) reject an unrecognized schema version
instead of silently reinterpreting bytes. Real, named, still-open
gaps, unchanged from before this round: §8 correlation/causation only
exists in `siar-messaging` (the one crate with a real workflow to
demonstrate it on); §14's cross-database atomicity gap; `stoolap_store`
itself still has no schema-version column on its own SQL tables (a
different, lower-level versioning question than §9's payload-level
one, named back in the very first Phase 2 round and still open).

**siar-event-log — durable checkpoint storage, 2026-09-19 (same
session, Claude's own choice of the two options offered at the end of
the previous round)**: new `stoolap_checkpoint_store.rs` —
`StoolapCheckpointStore`, the durable counterpart to
`projection::InMemoryCheckpointStore`, same `stoolap`-backed pattern
`stoolap_store::StoolapEventStore` already established for
`EventStore` itself (a `projection_checkpoints` table, `open`/
`open_in_memory` constructors, `DELETE`+`INSERT` rather than leaning on
`stoolap`'s own `UPDATE`/upsert support — same reasoning
`stoolap_store::append`'s own stream-head handling already gives).
Built ahead of any caller that actually needs it: every real
`Projection` in this workspace so far
(`siar_messaging::conversation_summary`) is itself in-memory, so
nothing currently NEEDS a durable checkpoint — the module's own doc
comment names why this was still worth building now rather than
waiting: pairing a durable materialized view with an in-memory
checkpoint would be actively wrong (a restart would resume "from where
we left off" against state that's actually still thousands of events
stale, silently skipping the gap rather than either replaying or
genuinely resuming), so checkpoint durability needs to exist BEFORE the
first durable projection is built, not after — this is that
prerequisite, not a premature optimization. One real design note
worth recording: `ProjectionId` wraps a `&'static str` (a deliberate
Phase 4 choice — real projection ids are always compile-time
constants, so no allocation needed for the in-memory case), which
means `load` can't reconstruct a `ProjectionId` from the TEXT column
it reads back without leaking memory for it — resolved by always
constructing the returned `ProjectionCheckpoint` from the CALLER's own
already-`'static` `projection_id` argument, using the column only to
verify the row exists, never to manufacture a new id. 5 new tests
(round trip, overwrite-not-duplicate — verified with a raw `COUNT(*)`
against the table, not just trusting `load`'s own unique-indexed
query, load of an unknown id, no cross-contamination between ids, and
a real close-and-reopen-the-same-file durability test matching
`stoolap_store`'s own) — 32/32 total in `siar-event-log`, clippy/fmt/
docs clean. Re-verified zero regressions in every crate touching
`siar-event-log` either directly or transitively:
`siar-crash-recovery` (50/50), `siar-dtn-bundle` (43/43),
`siar-identity-multidevice` (257/257), `siar-blob-manifest` (38/38),
`siar-messaging` (34/34, 21 unit + 13 integration).

Real, named, still-open gaps: no durable `Projection` implementation
exists yet to actually pair with this new store (it remains untested
against a real caller, only against direct unit tests of itself);
`stoolap_checkpoint_store`'s own `CREATE TABLE IF NOT EXISTS` has the
same no-schema-version-column gap `stoolap_store` already has, for the
same reason (not attempted at either layer yet). Suggested next: Phase
5 (revisit the `siar-storage`-outbox architectural question first,
per every previous round's own note) is now the largest remaining
piece of Phase 1-4-adjacent work; alternatively, a durable
`ConversationSummaryProjection` backed by a real `stoolap` table would
be the first real caller for this round's own new store, closing that
gap concretely rather than leaving it proven only in isolation.

**siar-messaging — a real durable ConversationSummaryProjection,
2026-09-19 (same session, the user's own explicit choice of the two
options offered at the end of the previous round)**: new
`stoolap_projections.rs` — `StoolapConversationSummaryProjection`, a
genuine drop-in replacement for the in-memory
`ConversationSummaryProjection`, backed by a real
`conversation_summaries` table (same `stoolap` pattern
`stoolap_store`/`stoolap_checkpoint_store` already established). This
is also `StoolapCheckpointStore`'s own first real caller — exercised
through an actual multi-event `send_text` workflow, not only its own
isolated unit tests from the previous round.

Making this a genuine drop-in required a real design change:
`MessageService`'s `conversation_summary`/`summary_checkpoints` fields
changed from concrete types (`Arc<ConversationSummaryProjection>`,
`Arc<InMemoryCheckpointStore>`) to trait objects
(`Arc<dyn ConversationSummaryQuery>`, `Arc<dyn
ProjectionCheckpointStore>`), where `ConversationSummaryQuery:
Projection` is a new supertrait relationship in `projections.rs` that
both the in-memory and durable projections implement. Passing that one
object to both `ProjectionRunner::catch_up` (which only knows the
`Projection` half) and the query side (which needs the subtrait's own
`get`) relies on Rust's trait-object upcasting coercion — stable since
1.86, confirmed working here on the first try against this workspace's
pinned 1.91 toolchain, so no manual workaround (storing the same
projection behind two separate `Arc`s, or a hand-written enum dispatch)
was needed. `conversation_summary`'s own query method is now `async`
(previously synchronous, since the in-memory backend's lookup never
did any real I/O) — a real, if small, breaking change to
`MessageService`'s own public API, fixed at every call site in the
same round. New `with_durable_conversation_summary(projection,
checkpoints)` builder, meant to chain right after `with_event_log`,
swaps both fields to their durable counterparts; calling it without
`with_event_log` first is harmless (the projection just never gets
consulted), documented as such rather than guarded against.

**A real bug caught before it shipped, not after**: the first draft of
`apply` fetched only `message_count` from the existing row before doing
a `DELETE`+`INSERT`, which would have silently blanked
`last_message_id` back to `NULL` on every event that isn't
`MessageCreated`/`MessageReceived` (delivery, read, edit, delete,
react) — a real correctness bug a manual delete-then-insert doesn't
avoid for free the way the in-memory version's `HashMap::entry()`
does. Caught while writing the code, before any test ran against it,
by re-deriving the in-memory version's own "preserve what `apply`
didn't touch" semantics explicitly; a dedicated regression test now
exists specifically for this
(`a_delivery_event_preserves_the_existing_last_message_id_and_does_not_
double_count`).

7 new tests total: 5 in `stoolap_projections.rs` itself (creation
counts and sets `last_message_id`; the delivery-preserves-state
regression test above; an out-of-range event type is silently skipped,
matching the in-memory version's own identical test; `reset` clears
everything; a real close-and-reopen-the-same-file durability test) and
2 in `tests/end_to_end.rs` (a direct unit-level check plus
`durable_conversation_summary_works_as_a_drop_in_replacement` — the
same real send-over-QUIC flow `conversation_summary_reflects_sends_
and_receipts_without_any_manual_catch_up` already covered for the
in-memory backend, now proven for the durable one through the exact
same `MessageService` public API, no special-casing). 40/40 total in
`siar-messaging` (26 unit + 14 integration), clippy clean, fmt clean,
docs clean (same one pre-existing unrelated warning, still untouched).
Re-confirmed `siar-event-log` itself is unaffected (32/32) — nothing
this round touched it; `siar-messaging` has no dependents anywhere in
this workspace, so that's the complete regression surface for this
change.

Real, named, still-open gaps: `stoolap_projections`'s own `CREATE
TABLE IF NOT EXISTS` has the same no-schema-version-column gap every
other `stoolap`-backed table in this workspace already has; the
durable projection and its durable checkpoint store are, like every
other `stoolap`-backed piece in this workspace, a separate database
from both `siar-storage`'s tables AND the event log's own store — no
new cross-store atomicity was created or claimed by adding this.
Suggested next: Phase 5 — and per every round since the gap first
surfaced, the `siar-storage`-outbox-vs-event-log-outbox architectural
question needs an actual decision (not a default) before building
anything there, since it determines whether Phase 5 extends the
existing outbox or builds a second one.

**siar-dtn-bundle / siar-emergency — §36/§37, the last two Phase 3
domain event catalogs, 2026-09-22 (same session, after the section
tracker + Phase 5 decision notes were written)**: with Phase 5 itself
deliberately deferred to notes (see the tracker/decision write-up at
the very end of this document), this round picked the next piece that
doesn't depend on that decision — closing out §92 Phase 3's two
remaining named domains, DTN and Emergency, in the exact same shape
every earlier domain module already established (`EventTypeId`
constants, a typed enum, `into_new_event`/`decode_*` with §9
schema-version checking built in from the start this time rather than
retrofitted, "construct only, never append").

`siar-dtn-bundle::events` (`EventTypeId` 300-308): nine events lining
up with [`state::BundleState`]'s own 11 states minus `Eligible`
(scheduling-internal, not durable history — §2) and minus `Rejected`
(happens before a bundle is ever durably created, so there's no
"existed, then was rejected" history distinct from never having
created one). §79's own "keep peer-encounter telemetry mostly
operational" is followed literally: `BundleForwarded` covers every
hop with zero per-hop peer/relay fields, matching
`BundleState`'s own collapse of repeated `Forward` transitions into
one state rather than a distinct one per hop. Added a real missing
piece found while wiring this: `BundleId` had no `Display` impl (its
inner `Uuid` isn't `pub`), so nothing outside `types.rs` could turn one
into a stream name — added, same accessor pattern every other new
domain ID this session already needed. This crate has explicitly NO
`siar_domain` dependency (its own `Cargo.toml` says so), so
`into_new_event` takes `origin: EventOrigin` as a pure opaque type and
never constructs a `DeviceId` itself — this module's own tests use
`EventOrigin::System`/`Imported` specifically to avoid needing one,
proving the boundary holds rather than quietly reaching around it.
9 tests, 51/51 total in the crate (43 pre-existing + 8 new — one test
covers two assertions worth counting separately), clippy/fmt/docs
clean. Real dependent checked: `siar-testkit` (5/5, unaffected).

`siar-emergency::events` (`EventTypeId` 400-405): six events —
`ReportCreated` (§80's own "SOS is persisted before transmission, even
with no network," made real), `TrustReclassified`, `ReportAcknowledged`,
`ReportResolved`, `ReportCancelled`, `ReportExpired`. Two real gaps
found and fixed while wiring this, both immediately necessary rather
than optional: `EmergencyReport` had no identifier of its own at all —
added a new `ReportId` (same UUID-newtype shape every other new domain
ID this session needed) — and `AlertTrust` had no `Serialize`/
`Deserialize` derive, needed the moment `TrustReclassified` tried to
embed one in a payload. `TrustReclassified` records trust's own
OUTCOME, never performs the signature verification that decides it —
same "decide vs record" split `siar_blob_manifest::events`'s own doc
comment already draws for `transfer_state.rs`, consistent with this
crate's own explicit no-`siar-crypto`-dependency boundary. Deliberately
did NOT add events for `DiscoveryMode` transitions (radio/power
management, not a report's own durable history — named explicitly as
an exclusion, not an oversight). 17/17 tests total in the crate (11
pre-existing + 6 new — plus the 2 new `ReportId` tests already counted
separately above make 8 new lines, 6 of them exercising the event
catalog itself), clippy/fmt/docs clean. No crate in this workspace
depends on `siar-emergency`, so no dependent regression check was
needed or possible.

§92 Phase 3 is now fully closed across all FIVE of its named domains
(§33 messaging, §34 files, §35 identity, §36 DTN, §37 emergency) — not
just the three from the original round. Real, named, still-open gaps,
same shape as every other domain past messaging: neither DTN nor
emergency has a real `EventStore::append` caller anywhere (construct-
only, same as files/identity); neither has §8 correlation/causation
wired beyond accepting the parameters (no real workflow to demonstrate
it against yet, same reasoning already given for files/identity); the
informal per-domain `EventTypeId` range convention now has FIVE blocks
to keep straight by hand (1-8, 100-108, 200-208, 300-308, 400-405)
with still nothing enforcing non-collision across any of them.

**siar-event-log — §55 Event Size Limits, 2026-09-22 (same session)**:
back in the CORE crate for the first time in several rounds (all the
recent work had been in domain crates layered on top of it) — a
self-contained piece from the tracker that, unlike Phase 5, needed no
architectural decision first. New `store::validate_payload_size` +
`DEFAULT_MAX_EVENT_PAYLOAD_BYTES` (256 KiB), enforced by BOTH real
backends (`InMemoryEventStore`/`StoolapEventStore`) before touching any
state — a batch with one oversized event anywhere in it is rejected
whole, per §11's own atomicity, not partially applied. New
`EventStoreError::PayloadTooLarge` variant. Named honestly, in the new
function's own doc comment: §55's text ("every event type must have a
maximum size") reads as inviting a PER-TYPE registry, which doesn't
exist anywhere in this workspace (there's still no central place all
five domains' `EventTypeId` ranges are even listed together — the same
gap named at the end of the previous round) — one uniform ceiling is a
real, deliberate first cut, not the section's fuller ask. 3 new tests
(oversized payload rejected before touching any state, in both
backends; a batch with a normal event AND an oversized one rejects the
whole batch, not just the bad one). Also fixed two stale doc-comment
claims found while touching `lib.rs` for this: the crate's own
top-level doc still said DTN/emergency had "no real crate home yet"
for their event catalogs, false since the previous round built both;
corrected. 35/35 tests in `siar-event-log` (3 new), clippy/fmt/docs
clean. Because this is the core crate, re-verified every real
dependent, not just the newest ones: `siar-blob-manifest` (38/38,
unaffected by this round's change), `siar-crash-
recovery` (50/50), `siar-dtn-bundle` (51/51), `siar-emergency` (17/17),
`siar-identity-multidevice` (257/257), `siar-messaging` (40/40, 26
unit + 14 integration) — zero regressions across all six.

Real, named, still-open gaps: the 256 KiB limit is uniform, not
per-`EventTypeId` — a real future refinement, not attempted; nothing
enforces it at the `NewEvent` construction site in any domain crate,
only at `append` time (a caller building an oversized `NewEvent` only
finds out when it tries to persist it, not earlier) — acceptable for
now since append is always the very next step in every real call site
built so far, but worth knowing if that ever stops being true.

**siar-event-registry — §63 Namespaced Custom Events, 2026-09-22 (same
session, the user's own explicit choice: complete a partial section
rather than start a fresh Phase)**: a brand new crate — the first new
crate added by any round in this whole session — because no EXISTING
crate could do this without creating a dependency cycle or a new,
pointless coupling between two domains: `siar-event-log` itself has no
dependency on any domain (by design), and no domain depending on
another domain just to compare `EventTypeId` constants would be a real
new coupling for no other reason. The only shape that works is a new
crate that depends on all five domain catalogs and is depended on by
none of them — `siar-event-registry` is exactly that, and nothing
else: it defines no events, decides nothing, and changes no existing
crate's own dependency graph.

`ALL_REGISTERED_EVENT_TYPES` lists all 41 real `EventTypeId` constants
across all five domains (8 identity + 9 messaging + 9 files + 9 DTN +
6 emergency), and `find_collisions` is a real, tested O(n²) pairwise
comparison — proven not just to pass on the real roster but to
actually FAIL when it should: deliberately introduced a fake collision
(pointed one messaging constant at an identity one) to watch the test
fail with a real panic and stack trace, then reverted it, rather than
trusting the passing test alone to mean the check works. A second
test checks every entry falls inside its own domain's documented
numeric block (1-8/100-108/200-208/300-308/400-405) — catching a
future misassignment even before it collides with anything. Named
honestly, in the crate's own top doc comment, in the most prominent
position it could occupy: this roster is MANUALLY maintained — nothing
automatically adds a new domain's constant here, so the guarantee this
crate provides is real but conditional on whoever adds a sixth domain,
or a new event to an existing one, also remembering to add a line
here. A registry immune to that would need `inventory`/`linkme`-style
compile-time cross-crate registration — genuinely new dependencies,
not attempted this round.

Real infrastructure note: this is the first NEW crate added this
session, so it needed real workspace-level plumbing most rounds
haven't touched — added to `members` in both the real root `Cargo.toml`
and this sandbox's own trimmed scratch copy, plus a new `[workspace.
dependencies]` entry for `siar-event-registry` itself. Also found and
fixed a real, separate gap while wiring this: `siar-emergency` had
NEVER been added to `[workspace.dependencies]` at all (only to
`members`) — nothing before this round needed to depend on it via the
workspace-alias short form, so the gap went unnoticed; added now,
benefiting any future crate that wants `siar-emergency.workspace =
true` instead of a bare path dependency. 5/5 tests, clippy clean, fmt
clean, docs clean. Zero regressions confirmed in all five domain
crates this new crate depends on (38/38, 51/51, 17/17, 257/257,
26+14/26+14) — expected, since this crate only reads their public
constants and changes nothing about them, but checked anyway rather
than assumed.

Real, named, still-open gaps: the manual-maintenance risk named above,
unavoidable in this architecture without new dependencies; this crate
has no CI/build-time hook forcing it to run on every change to any of
the five domains' event catalogs — it's a real test that exists, but
nothing currently guarantees anyone runs it before merging a change
that would need it.


| # | Crate | State |
|---|---|---|
| 01 | siar-protocol-ext | ✅ **108/108 — spec complete** (final round: §91-92 reconciled, §93-95 error codes/health/recovery, §96-99 scheduler contract/storage/metrics/capability isolation, §100-105 reconciled with notes, §106 honest 16-item Definition of Done self-audit — 4 genuine gaps named, §107-108 reconciled) |
| 02 | siar-identity-multidevice | ✅ **204/204 — spec complete** (final round, 2026-09-05: §190-204 — algorithm agility/downgrade protection utilities kept deliberately minimal per spec's own "avoid needless abstraction" caution; a root-key backup envelope that structurally cannot carry plaintext key material; backup-import validation run before any local state is touched; identity-reset/account-deletion presentations with required disclaimer fields; a guarded organization-offboarding state machine that operates only on organization-scoped device ids, never a personal AccountId; multi-tenant-safe composite keys; migration-fixture round-trip tests (honestly incomplete pending §125); and an itemized 21-item Definition-of-Done self-audit — **19/21 fully done, 2 honestly `PartiallyDone`** (no-UI-shipped confirmation prompt; property/integration tests exist but no real fuzz harness). Also fixed a genuinely broken intra-doc link left over from an earlier round, dropping this crate's doc-warning count from 4 to 3. 6 new modules (`algorithm_agility.rs`, `root_key_backup.rs`, `identity_lifecycle.rs`, `migration_fixtures.rs`, `definition_of_done.rs`) plus a `namespace.rs` extension, 20 new tests, 251/251 total, clippy clean, zero regressions. Across all 11 rounds this session: 137 new tests written, zero regressions in siar-routing-policy/siar-crypto at any point, every round compiled+tested+clippy+fmt+doc-checked for real against the actual uploaded Cargo.lock with rustc 1.91.1. Real, named, still-open gaps carried forward into future work: §125 schema versioning absent from DeviceCertificate/DeviceDirectory; §164 no cargo-fuzz harness; §191 full cross-version migration tests blocked on §125; `storage::IdentityStore`/`transaction`/all four `client_api` traits have zero real call sites anywhere in this workspace yet; `RootTrustCacheEntry`/`VerifiedContact` overlap not consolidated; §107/§91 have no real BLE/Wi-Fi/NFC transport wiring.) |
| 03 | siar-routing-policy | ✅ **200/200 — spec complete** (final round, 2026-09-15: §183-200 — §183-184 "Testing Matrix"/"Route Selection Golden Tests": all 5 of the spec's own worked examples transcribed as tests (1 caught a real bug in *this round's own test code* — a stale pre-mutation health snapshot in a stickiness test, fixed), plus 2 combos (BLE-only, Wi-Fi Direct+BLE) with no prior coverage under any framing; §185 "Property Tests" — 2 properties not yet covered under this broader framing (`allow_relay`/`allow_bluetooth` forbidden-transport, and *known* insufficient bandwidth as a genuine hard elimination, not just the existing unknown-bandwidth-isn't-penalized test); §186/§187 "Fuzzing"/"Benchmarking" — real, named gaps (no `cargo-fuzz`, no `criterion`), with 2 targeted NaN-safety tests as a partial substitute for the former; §188 "Scalability" needed zero code (operation-level routing, `RouteCache`, both already true); new `reevaluation.rs` for §189-191 (`quality_change_exceeds_threshold`/`should_reevaluate_file_route`; §191 needed zero code — already `RouteCache`/`RouteHint`); §192-197 "Architecture Reconciliation" added as new lib.rs documentation (module structure, no-UI-deps, `RoutingError`'s 6 variants mapped onto the spec's own suggested 7, no-anyhow, initial-scope/phases all honored); §198 "Definition of Done" — a full, honest self-audit (✅/⚠️/❌ per item, matching specs 01/02's own closing pattern); §199-200 closing documentation; **also found and fixed a real gap**: most of rounds 10-18's public types were never re-exported at the crate root, only rounds 1-9's — fixed with a full pass of `pub use` additions matching the established style. 18 new tests, 263/263 total, clippy/fmt/doc clean (7 broken intra-doc links fixed), zero regressions. Named, still-open gaps carried forward: no fuzz/benchmark harness, DTN delivery-probability computation (representation exists since round 16, computation needs real encounter history this crate has never had), §55 "Mesh Forwarding"'s richer candidate representation (named since round 2), `Destination::Group` resolution (named since round 18), a handful of individual adapter-reporting fields (Bluetooth proximity/paired state, Wi-Fi group/session, LAN interface name). Across all 18 rounds building this crate: round 18 covered §171-182, round 17 covered §160-170, round 16 covered §151-159, round 15 covered §140-150, round 14 covered §128-139, round 13 covered §121-127, round 12 covered §116-120, round 11 covered §108-115, round 10 covered §105-107, rounds 2-9 covered §43-104, round 1 covered §1-42 — every round compiled+tested+clippy+fmt+doc-checked for real against the actual uploaded Cargo.lock with rustc 1.91.1, zero regressions in any dependent crate at any point except one one-line fix in round 17) |
| 04 | siar-event-log | 🟡 27✅/24🟡/35⬜/9◇ of 95 (new `siar-event-registry` crate closes §63 Namespaced Custom Events 2026-09-22 — first new crate this session); §92 Phase 3 fully closed; Phase 5 deliberately deferred to notes — see full tracker + decision write-up at the very end of this document |
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
(`siar-identity-multidevice`) is complete (204/204) as of 2026-09-05.
Spec 03 (`siar-routing-policy`) is complete (200/200) as of 2026-09-15.
Spec 04 (`siar-event-log`) is now the active crate in this project's
explicit priority order — Phases 1-4 are done, the §5-15 audit pass is
closed, and `StoolapCheckpointStore` now has a real caller: a durable
`StoolapConversationSummaryProjection` in `siar-messaging`, a genuine
drop-in for the in-memory version via a new `ConversationSummaryQuery`
trait and Rust's stable trait-object upcasting. A full per-section
tracker (27 done, 24 partial, 35 not started, 9 conceptual, of 95 as
of 2026-09-22 — §63 Namespaced Custom Events closed this round via a
new `siar-event-registry` crate, this session's first entirely new
crate),
the §93 Definition-of-Done self-audit, and the Phase 5 outbox decision
are all recorded at the very end of this document now, at the user's
own explicit request — the decision itself is deliberately NOT made:
does Phase 5's outbox/effects/retry/recovery machinery extend
`siar-storage`'s existing messaging outbox, or build a second one
against the event log? That note is written to be acted on later,
when either a real development need forces the question or there's
time to weigh it properly — not decided by momentum in the meantime.
Note there is a real, documented
unresolved reconciliation question between `siar-routing`
(pre-existing, next.md-era) and `siar-routing-policy` (spec 03's own
crate) — see that crate's own `lib.rs` for the current state of that
question, now tracked in detail in `MIGRATION.md`.

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

---

## Spec 04 (`siar-event-log`) — full section-by-section tracker

Built 2026-09-22, at the user's own explicit request ("track section in
04 offline event log specs, along update it how much section in which
specs completed"), after Phases 1-4 were already done by round. This
is the granular counterpart to that crate's own `~37/95` running count
in the table above — every one of the spec's 95 numbered sections,
checked against what's actually in the crate/its domain callers, not
assumed from which Phase it nominally belongs to. Re-read every
section not already covered by an earlier round's own notes before
writing this, rather than inferring status from memory.

**Legend**: ✅ done and real (code exists, is tested, matches the
section) · 🟡 partial (something real exists but doesn't fully satisfy
the section — see the note) · ⬜ not started · ◇ conceptual/guidance
section with no code artifact of its own (informs design elsewhere;
"done" doesn't apply to it the way it does to a type or a table).

| § | Title | Status | Note |
|---|---|---|---|
| 1 | Purpose | ◇ | Informs everything; no artifact of its own. |
| 2 | Do Not Event-Source Everything | ◇ | Followed: only 3 domains (messaging/files/identity) have catalogs, not "everything." |
| 3 | Command vs Event | ◇ | Conceptual distinction; reflected in `NewEvent` vs domain command handling, not a type. |
| 4 | Core Event Envelope | ✅ | `EventEnvelope` (Phase 1). |
| 5 | Streams | ✅ | `StreamId`, per-stream version (Phase 1/2). |
| 6 | Local Global Offset | ✅ | `LocalLogOffset` (Phase 1/2). |
| 7 | Event Origin | ✅ | `EventOrigin` (Phase 1). |
| 8 | Correlation and Causation | 🟡 | Real end-to-end in `siar-messaging` (`MessageCorrelation` registry). Identity/files still always `None` — no real workflow to correlate yet. |
| 9 | Versioned Event Schemas | ✅ | `schema_version`-checked decode in all 3 domain catalogs; rejects unrecognized versions instead of blind-decoding. |
| 10 | Append-Only Semantics | ✅ | No update/delete API on stored events, by construction. |
| 11 | Atomic Append | ✅ | `stoolap` transaction + process mutex (Phase 2). |
| 12 | Optimistic Concurrency | ✅ | Version check + `ConcurrencyConflict` + `append_with_retry` (Phase 2). |
| 13 | Local-First Command Flow | ✅ | `send_text`'s existing persist→record→network-send ordering already matched this; confirmed and named in comments. |
| 14 | Transactional Outbox | 🟡 | Resilience test proves a broken event log doesn't break the real outbox. True cross-store atomicity, and the "extend vs build a second outbox" decision, both still open — **see the Phase 5 decision notes below.** |
| 15 | Event Log Is Not a Job Queue | ✅ | Outbox retry logic uses its own `due()`, never scans the event log. |
| 16 | Projection Architecture | ✅ | `Projection` trait + `ProjectionRunner` (Phase 4). |
| 17 | Projection Checkpoints | ✅ | `ProjectionCheckpoint` + in-memory AND durable (`StoolapCheckpointStore`) backends. |
| 18 | Read-Your-Writes | ✅ | Real, via synchronous catch-up after append — honestly documented as "sync-after," not "same transaction" (a real, smaller guarantee than the section's own ideal). |
| 19 | Event Store Backend | ✅ | `StoolapEventStore` (Phase 2). |
| 20 | Event Store Trait | ✅ | `EventStore` (Phase 1). |
| 21 | Batch Append | ✅ | `AppendRequest.events: Vec<NewEvent>`, all-or-nothing (Phase 1/2). |
| 22 | Integrity | ✅ | blake3 checksum per row, verified on read (Phase 2). |
| 23 | Remote Event Ingestion | ⬜ | No dependency on identity/protocol crates for validation; only `detect_gap` (§26) serves this path at all. |
| 24 | Idempotency | ✅ | Duplicate `event_id` is a no-op, tested (Phase 1/2). |
| 25 | Out-of-Order Events | 🟡 | `detect_gap` reports a gap; nothing holds or reorders — reporting only, no remediation. |
| 26 | Gap Detection | ✅ | `detect_gap` (Phase 1), tested against the spec's own worked example. |
| 27 | Logical Clocks | 🟡 | `stream_version` provides real per-stream ordering; no hybrid logical clock beyond that. |
| 28 | Offline IDs | ✅ | `EventId`/`CorrelationId` are locally-generated `Uuid` v4 — collision-resistant, offline, stable across retries (verified by `append_with_retry`'s own reuse of the same id). Not time-sortable (not v7) — the section calls this an optional locality improvement, not a correctness requirement. |
| 29 | Pure Decision Functions | ⬜ | No `decide(state, command) -> Result<Vec<DomainEvent>, DomainError>` pattern provided or enforced anywhere. |
| 30 | Effect Processing | 🟡 | `record_messaging_event`'s catch-up call is effect-adjacent but ad hoc — no formal effect-processing pattern/type exists. |
| 31 | Exactly-Once Is Not the Goal | ◇ | Reflected in §24's own idempotent-no-op design; no artifact of its own. |
| 32 | Retry Event Granularity | ◇ | Reflected in `append_with_retry`'s own per-event retry design; no artifact of its own. |
| 33 | Messaging Events | ✅ | Full: catalog, real `append` caller (3 call sites), real correlation, real durable projection. The most complete domain by far. |
| 34 | File Events | 🟡 | Catalog real and tested, §9 schema-version fix applied. New `siar-file-transfer-service` crate provides real `FileTransferService` caller wiring `TransferState` transitions to `EventStore::append` via `append_with_retry`. |
| 35 | Identity Events | 🟡 | Catalog real and tested, §9 fixed. New `siar-identity-audit-recorder` crate provides real `IdentityAuditRecorder` caller wiring `audit_log` event construction to `EventStore::append` via `append_with_retry`. |
| 36 | DTN Events | 🟡 | `siar-dtn-bundle::events` (2026-09-22): 9 events, `EventTypeId` 300-308, tested. New `siar-dtn-bundle-service` crate provides real `DtnBundleService` caller wiring `BundleState` transitions to `EventStore::append` via `append_with_retry`. |
| 37 | Emergency Events | 🟡 | `siar-emergency::events` (2026-09-22): 6 events, `EventTypeId` 400-405, tested; new `ReportId` added. New `siar-emergency-service` crate provides real `EmergencyReportService` caller wiring `ReportStatus` transitions to `EventStore::append` via `append_with_retry`. |
| 38 | Snapshotting | ⬜ | Phase 7, not started. |
| 39 | Snapshot Structure | ⬜ | Phase 7, not started. |
| 40 | Compaction | ⬜ | Not started. |
| 41 | Privacy and Deletion | ⬜ | Not started. |
| 42 | Event Encryption | 🟡 | Messaging's own `ciphertext` field is already application-encrypted upstream (by `siar-crypto`, before it ever reaches this crate). No local-database-at-rest encryption exists — the `stoolap` files this crate writes are plain, unencrypted files. |
| 43 | Blob References | ✅ | File events reference `BlobId`/`ManifestId`, never raw bytes. Messaging embeds small ciphertext directly by design (text content, not "large binary data" — attachments go through the separate blob subsystem, not through `MessagingEvent`). |
| 44 | Search as Projection | ⬜ | No FTS projection exists. |
| 45 | Replay Must Not Re-run Side Effects | 🟡 | True by construction today — the only real `Projection` (`ConversationSummaryProjection`) has zero side effects, so replay is safe. Never stress-tested against a side-effecting projection, since none exists yet. |
| 46 | Replay Modes | ⬜ | No `ReplayMode` enum; `catch_up` has one implicit mode. |
| 47 | Startup Recovery | ⬜ | No orchestrated startup sequence exists anywhere in what's been built. |
| 48 | Work Queue Reconciliation | ⬜ | Not started. |
| 49 | Replication Scope | ⬜ | No `ReplicationScope` enum. |
| 50 | Own-Device Sync | ⬜ | Not started. |
| 51 | Peer and Group Sync | ⬜ | Not started. |
| 52 | Local Storage Envelope vs Network Envelope | ⬜ | `StoredEvent` is used directly wherever a wire form would be needed — no separate `ReplicationEventV1` transform layer exists yet (moot until real replication exists). |
| 53 | Sync Cursors | ⬜ | No `SyncCursor` type. |
| 54 | Conflicts Are Domain-Specific | ◇ | Correctly not building one generic resolver — but nothing exists yet to point to as "done" either, since no real conflicts have arisen. |
| 55 | Event Size Limits | ✅ | `validate_payload_size`/`DEFAULT_MAX_EVENT_PAYLOAD_BYTES` (2026-09-22), enforced by both real backends before any write. One uniform 256 KiB ceiling, not yet the per-event-type registry the section's own text invites — see that function's own doc comment for why that's a deliberate, named first cut. |
| 56 | Durability Classes | ⬜ | No `DurabilityClass` enum; nothing distinguishes Critical/Durable/BestEffort. |
| 57 | SQL Schema | ✅ | `events`/`stream_heads` (Phase 2) plus `projection_checkpoints`/`conversation_summaries` (Phase 4). |
| 58 | Indexes | ✅ | Unique indexes on offset/event_id/(stream,version) plus each new table's own (Phase 2/4). |
| 59 | Memory Discipline | ✅ | `ProjectionRunner::catch_up` reads bounded batches (`batch_size`), discards between them — matches the section's own diagram exactly. |
| 60 | Projection Isolation | 🟡 | A broken event log doesn't stop already-succeeded appends (tested); a projection catch-up failure is logged and swallowed, not fatal. Cross-projection isolation (one broken projection not blocking another) isn't demonstrated — only one real projection exists. |
| 61 | Internal Event Notifications | 🟡 | New `siar-event-notify` crate provides decoupled wake/notify mechanism via `ChangeNotifier` and `NotifyingEventStore`, broadcasting on real non-duplicate appends. Still needs live subscribers/projections wired in `apps/*`. |
| 62 | Unknown Events | 🟡 | "Optional unknown → store/ignore safely" is real and tested (`ConversationSummaryProjection` skips foreign event-type ranges). "Required semantic unknown → block stream until upgrade" doesn't exist. |
| 63 | Namespaced Custom Events | 🟡 | New `siar-event-registry` crate (2026-09-22): a real, running, tested cross-domain collision check over all 41 `EventTypeId` constants across all five domains — proven to actually fail on a real collision, not just pass vacuously. Still manually maintained (nothing auto-adds a new domain's constant to the roster) — a real registry immune to that would need `inventory`/`linkme`-style compile-time registration, not attempted. |
| 64 | Multi-Tenant Isolation | ⬜ | No `TenantId` concept anywhere. |
| 65 | Multiple Identities | ⬜ | No isolation between personal/work identities on one device. |
| 66 | Security | 🟡 | Local corruption (§22 checksum) and duplicate/replay (§24) are covered. Malformed-imported-event, rollback, oversized-payload, and unauthorized-remote-event are all moot until §23 (remote ingestion) exists at all. |
| 67 | Event Store Errors | 🟡 | `EventStoreError` has `ConcurrencyConflict`/`Backend`/`Corrupt`/`StreamNotFound` — deliberately minimal (only what's been needed), not the spec's full suggested set (`DuplicateEvent`/`StorageFull`/`ReadOnly`/`MigrationRequired`/`Io`/`Serialization`). |
| 68 | Storage Full Behavior | ⬜ | Not specifically handled or tested. |
| 69 | Read-Only Recovery Mode | ⬜ | A `Corrupt` error just returns `Err`; no read-only fallback mode. |
| 70 | Backup | ⬜ | `read_log(from_offset, ...)` gives exactly the primitive §70 asks for, but no backup tooling is built on top of it. |
| 71 | Restore Safety | ⬜ | Not started. |
| 72 | Analytics Separation | ◇ | No analytics pipeline exists yet to separate anything from. |
| 73 | Dioxus Boundary | ⬜ | No UI crate in this workspace's uploaded scope to assess against. |
| 74 | Kotlin / iOS Boundary | ⬜ | No mobile platform code in this workspace's uploaded scope. |
| 75 | Daemon Compatibility | ⬜ | Not assessed — no daemon architecture seen in the uploaded crates. |
| 76 | Headless Compatibility | 🟡 | Satisfied by omission — nothing built so far couples to a UI — but never explicitly declared or tested as a requirement. |
| 77 | Routing Integration | ⬜ | No wiring between `siar-routing-policy` and `MessageQueued`/the event log. |
| 78 | File Integration | ✅ | `FileEvent`'s catalog is transfer-level only — no per-chunk event exists — matching this section's own "semantic transfer events, not one event per chunk" exactly, by design, from when §34 was first built. |
| 79 | DTN Integration | ⬜ | Not started. |
| 80 | Emergency Integration | ⬜ | Not started. |
| 81 | Diagnostics | ⬜ | Not started. |
| 82 | Metrics | ⬜ | Not started. |
| 83 | Property Tests | 🟡 | Several of the section's own listed invariants ARE covered — but only by targeted example tests (one specific scenario each), never by a property/fuzz framework generating arbitrary cases. |
| 84 | Crash Injection Tests | 🟡 | Close-and-reopen-the-same-file tests exist for `stoolap_store`/`stoolap_checkpoint_store`/`stoolap_projections` — a real but narrow proxy for "after commit, process restart." Not true injection at arbitrary points (before append, mid-transaction, after projection before network effect, etc.). |
| 85 | Fuzzing | ⬜ | Not started. |
| 86 | Golden Event Tests | ⬜ | No fixed-byte-encoding tests for any event schema. |
| 87 | Recovery Acceptance Test | ⬜ | The exact composed scenario (kill→restart→projection restored→outbox reconstructed→route found→same MessageId resent→recipient dedupes→MessageDelivered committed) doesn't exist as one test, though several of its pieces are covered separately by other tests. |
| 88 | Multi-Device Offline Test | ⬜ | Not started. |
| 89 | Suggested Crate Structure | 🟡 | Deliberately deviated, and said so from the first Phase 2 round onward: kept one `EventStoreError` rather than splitting into the suggested `codec.rs`/`registry.rs`/`retention.rs`/`replay.rs`/`diagnostics.rs`/`error.rs`. |
| 90 | Public API | 🟡 | `EventStore`/`ProjectionRunner` match the suggested short list; no separate `EventAppender`/`EventReader`/`SnapshotStore` types — `EventStore` covers append+read in one trait instead. |
| 91 | Initial Production Scope | 🟡 | Of the 11 "implement first" items: 9 done (SQLite store, stream versioning, global offset, unique IDs, batch append, projection checkpoints, replay, schema versioning, and — partially — critical projections). Outbox integration and basic snapshots are the two genuinely missing. |
| 92 | Implementation Phases | 🟡 | This section IS the master phase structure the rest of this document tracks by. Phases 1-4 done; 5 (outbox/effects/retry/recovery), 6 (replication), 7 (snapshots/compaction/crash/fuzz/bench) remain. |
| 93 | Definition of Done | 🟡 | Self-audited against all 21 items — see the dedicated subsection immediately below rather than one cell here; roughly 7 done, 9 partial, 5 not started. |
| 94 | Relationship to Other Parts | ◇ | Cross-references to other specs — see the Phase 5 decision notes below for the one place this actually mattered so far (Part 03/`siar-routing-policy`, and `siar-storage`'s own pre-existing outbox). |
| 95 | Final Principle | ◇ | The one-sentence guarantee ("if the app says a durable operation was accepted, that intent survives network loss and process termination") this whole spec exists to make true. Partially true today: real for messaging's own send path (proven by the crash-adjacent durability tests + the broken-event-log resilience test); not yet extended to files, identity, DTN, or emergency, none of which have a real caller. |

### §93 Definition of Done — full 21-item self-audit

Reading the section's own bullets literally, in order, rather than
summarizing:

1. accepted local commands survive process death — ✅ (real file
   close/reopen durability tests, three different `stoolap`-backed
   stores)
2. operations can be accepted without Internet — ✅ (local-first by
   construction; `append` has no network dependency)
3. IDs require no central server — ✅ (`Uuid` v4, generated locally)
4. stream versions provide deterministic ordering — ✅
5. duplicate remote events are idempotent — ✅ (tested)
6. projections rebuild deterministically — 🟡 (mechanism is real and
   tested; determinism depends on each `Projection`'s own `apply`
   being pure, which isn't generally enforced)
7. checkpoints recover after crashes — ✅ (durable checkpoint store,
   tested)
8. UI reads optimized projections — ⬜ (no UI layer in scope here to
   wire this to)
9. large data is referenced via blobs — ✅ (file events; message text
   ciphertext is small content, not "large data," by design)
10. message outbox survives restart — ✅ (`siar-storage`'s own outbox,
    pre-existing, confirmed still working)
11. file semantic state survives restart — 🟡 (`siar-file-transfer-service`
    wires `transfer_state.rs` transitions to durable `FileEvent`s via
    `EventStore::append_with_retry`; live caller in `apps/*` driving
    chunk transfer through service remaining)
12. device lifecycle remains auditable — 🟡 (`siar-identity-audit-recorder`
    wires `audit_log` constructors to durable event append via
    `EventStore::append_with_retry`; live caller in `apps/*` or
    `siar-identity-multidevice` flows remaining)
13. SOS is persisted before transmission — 🟡 (`siar-emergency-service`
    provides `EmergencyReportService` persisting report/SOS lifecycle via
    `EventStore::append_with_retry` before network transmission; live
    caller in `apps/*` remaining)
14. DTN lifecycle is durable — 🟡 (`siar-dtn-bundle-service`
    wires `BundleState` transitions to durable event append via
    `EventStore::append_with_retry`; live caller in `apps/*` remaining)
15. replication scope is explicit — ⬜ (§49 not built)
16. event schemas are versioned — ✅ (§9, all three domains)
17. replay never accidentally re-runs external effects — 🟡 (true by
    construction today, since the one real projection has no side
    effects; never stress-tested against one that does)
18. storage-full is handled safely — ⬜ (not specifically tested)
19. no external side effect occurs before durable commit — ✅ (mostly
    confirmed for messaging's own send path — persist, then record,
    then network send, in that order, per §13 — not exhaustively
    audited across every other code path)
20. crash/property/fuzz tests exist — 🟡 (crash-ADJACENT tests exist;
    no property tests; no fuzz tests)
21. the subsystem works outside the messenger — 🟡 (the CORE crate,
    `siar-event-log` itself, has zero dependency on any domain crate —
    proven structurally by identity and files each building their own
    independent catalog against the same trait — but the only domain
    with real END-TO-END usage, an actual `append` caller, is
    messaging)

Honest overall read: roughly a third of this checklist is genuinely
done, a third is real-but-partial, and a third hasn't been started —
which lines up with the ~37/95 running section count above being
closer to "40% of the letter of the spec" than "40% of a production-
ready subsystem," since the sections most concentrated in the ⬜
column (remote ingestion, replication, snapshots, most of the test-
harness sections) are disproportionately hard relative to their count.

### Phase 5 decision — deliberately deferred, not decided

Recorded here rather than acted on, per the user's own explicit
instruction this round: build the note, not the code, so this is
ready to act on "when [there's] a better time or a development which
requires writing the code."

**The question**: does §14's transactional-outbox/effects/retry
machinery (Phase 5) extend `siar-storage`'s existing messaging outbox
(`OutboxRepository`, already real, already tested, already what
`send_text`/`retry_due` use today), or build a second,
event-log-native outbox alongside it?

**Why it's a real fork, not a default**:
- *Extend the existing one*: less duplication, one source of truth for
  "what needs retrying." But couples `siar-event-log`'s own retry
  story to a table `siar-storage` owns, and `siar-storage`'s outbox
  currently only knows about messaging — files/identity/DTN/emergency
  would need either their own outbox tables (repeating the pattern
  per-domain, same as the event catalogs already do) or a shared
  generic one that doesn't exist yet either way.
- *Build a second one on the event log*: matches the event-sourcing
  pattern Phases 1-4 have followed throughout (domain-agnostic core,
  domain-specific usage) and could plausibly become the ONE outbox
  every domain uses, not just messaging. But now two systems both
  think they own "pending work" for messaging specifically
  (`siar-storage`'s outbox AND whatever Phase 5 builds), which is
  exactly the kind of split-brain two-sources-of-truth situation that
  causes real bugs later if they ever disagree about a message's
  state.

**What's already true, and doesn't disappear whichever way this
goes**: `siar-storage`'s outbox works today, is tested, and
`send_text`'s own real resilience test
(`send_text_survives_a_completely_broken_event_log`) already proves
messaging's actual delivery doesn't depend on the event log at all —
so there is no urgency pressure to resolve this from a "something is
broken" angle. This is purely a forward design choice about where
Phase 5's OWN new work should live, not a fix for anything currently
failing.

**What would make this decision easier later**: whichever domain gets
a real `append` caller next (see §34/§35's own "no real caller" gaps
above) will surface the actual shape of the problem — if files or
identity end up needing their own retry/outbox logic too, that's real
evidence for "build it once on the event log," rather than a
hypothetical argument. Revisit this note the next time Phase 5, or a
second domain's outbox need, actually comes up — not before.
