# Part 06 — DTN / Store-Carry-Forward Architecture

Source spec: `sys-arch/06-dtn-store-carry-forward-architecture.md` — 192 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~65/192 (34%) sections.** Partial — approximate, not from a full section-by-section audit like spec 04's.

## Implementing crate(s)

- `siar-dtn-bundle` — sole implementing crate. Baseline coverage (§5-6, §10-13, §17, §36 types; §7 payload; §6/§8/§13/§20-22 bundle; §15-16 store; §18-19 state; dedup ported from the retired `siar-dtn`; §23/§25/§26 forwarding) pre-dates this reorganization — see this crate's own `lib.rs` doc comment for the fullest live account of what's covered.
- Cross-references: `siar-dtn-bundle-service`, `siar-emergency`, `siar-event-registry`, `siar-testkit` all depend on this crate.

## Round-by-round history

1. **Baseline (pre-2026-09-27)**: see "Implementing crate(s)" above and this crate's own `lib.rs` doc comment for the full account — this reorganization pass didn't re-derive it from scratch.
2. **DTN Events, 2026-09-22 (see `docs/04` round 10)**: `events.rs` — `EventTypeId` 300-308, 9 events, tracking `BundleState`. This is `04-offline-event-log-architecture.md`'s own §36, not one of this spec's 192 sections.

### Round 3 (2026-09-27) — §14, §69, §91/§93-94

The next cluster after the pre-existing baseline, picked for the same reason spec 05's rounds have been: pure types and decisions, no new transport/encounter infrastructure needed.

- **§14 Bundle Envelope vs Local Record — a real, previously-named gap, now closed**: `store::WireBundle` is a type alias for `bundle::DtnBundle` (which already was the correct wire shape — nothing needed to move off it). `store::StoredBundle` gained the three local-only fields the section names: `storage_path: Option<String>`, `retry_count: u32` (bumped by `record_retry`), `peer_history: Vec<RouteToken>` (appended, not deduplicated, by `record_peer` — the same peer can legitimately receive a bundle across more than one encounter). The pre-existing `state: BundleState` field already satisfied the section's fourth item ("custody state") — confirmed and documented, not new work.
- **§69 Cancel Bundle — surfaced and fixed a real state-machine bug**: `BundleState::is_cancellable()` added (delegates to `transition`, not a second source of truth). Building it surfaced that `(Forwarded, Cancel)` was never a legal transition — meaning an in-flight, still-undelivered bundle (§19 already establishes `Forwarded` ≠ delivered) could never be cancelled at all, directly contradicting §69's own "still-undelivered" wording. Fixed in `BundleState::transition`, the same "found via building the real thing, not guessed" pattern as the earlier `mark_eligible` gap this crate's own `lib.rs` already documents. New `cancel::BundleCancel` wire message (`bundle_id`, `issued_at_millis`).
- **§91 Authority Alerts, §93 Priority Authorization, §94 Priority Downgrade**: new `priority_authorization.rs` — `AuthorityAlert` (origin signature/trusted issuer/expiry/scope, all data-only — no verification logic, which stays `siar-crypto`/`siar-identity-multidevice`'s job); `authorize_priority(claim, unauthorized_policy)`, structurally unable to grant `DtnPriority::Sos` (this crate's own name for the spec's `AuthorityCritical`) to a claim that isn't independently marked authorized, with the downgrade-or-reject fork left as an explicit caller-supplied policy per §94's own "downgrade, or reject" wording. §92 Spam Resistance was deliberately given no type of its own — its six named mechanisms belong to `siar-resource-limits` (Part 08 — see the cross-spec ranking in `ROADMAP.md`), an identity/authority concern, or are already covered by this round's own §93 work.

**Verification**: 11 new tests, 67/67 total in `siar-dtn-bundle`. `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. `cargo doc` clean. All four real dependents re-tested in the same round: `siar-dtn-bundle-service` 6/6, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-testkit` 5/5 — zero regressions, confirmed immediately.

### Round 4 (2026-09-28) — §54-61

The storage/admission-policy cluster: eviction ordering, quotas, and the admission gate a relay checks before persisting anything, plus the validation checklist upstream of that gate.

- **§54 Eviction**: new `eviction.rs` — `EvictionTier`, a real `Ord`-ranked seven-tier enum (declaration order IS eviction order — no insertion-order field exists anywhere, so "never use FIFO blindly" is structural, not a reminder) and `eviction_tier(state, priority)` mapping a bundle onto it. State wins over priority: an expired or delivered SOS bundle still evicts before a healthy low-priority one.
- **§55 Storage Quotas, §56 Critical Reserve, §57 Per-Peer Quota, §58 Admission Control**: new `admission.rs` — `StorageQuotas` (four named limits); `is_reserved_class`, reusing the pre-existing `StorageClass::CriticalEmergency`/`DeliveryReceipt` variants rather than adding a third for "authority alerts" (an authority alert IS a critical-emergency-class artifact); `PeerQuotaTier`/`PeerQuotaPolicy`, deliberately kept separate from `priority_authorization::PriorityClaim`'s authorization bool per the section's own "authorization and quota are separate"; `admit_bundle`, the seven-check gate in the spec's own listed order, structurally unable to admit on an all-false input (mirrors the `authorize_transfer` pattern spec 05's `transfer_authorization` module already established).
- **§59 Bundle Validation, §60 Oversized Bundle, §61 Malformed Metadata**: new `validation.rs` — `validate_bundle`, covering wire size (checked against the bundle's own real `postcard`-serialized length, never a size field inside it — §60's own "never trust remote size fields"), payload size (delegates to the pre-existing `PayloadReference::validate`), hop limit, expiry, and destination-token emptiness; plus a new `BundleIntegrity::verify_payload_hash` (this round's other real addition, using a newly-added `blake3` dependency — the "signature/integrity" half of §59 this crate can actually check without `siar-crypto`, since a relay never needs to decrypt a payload to confirm it's the bytes the origin hashed). §59's own two items this crate has no field for — bundle-format `version`, `header size` — are named honestly rather than faked with an always-passing check. §61 needed no code at all: parsing isolation is resolved by construction (`serde`/`postcard` has no shared mutable state across separate deserialize calls), documented rather than wrapped in a redundant type.

**Verification**: 19 new tests, 86/86 total in `siar-dtn-bundle`. One real doc-link bug caught and fixed by `cargo doc` (a cross-crate intra-doc link to `siar-blob-manifest`'s own `transfer_authorization` module, which `rustdoc` correctly can't resolve from inside this crate — rewritten as plain prose). `cargo clippy --all-targets -- -D warnings` clean. `cargo fmt --check` clean. All four dependents re-tested in the same round: `siar-dtn-bundle-service` 6/6, `siar-emergency` 24/24, `siar-event-registry` 5/5, `siar-testkit` 5/5 — zero regressions.

## Known gaps / open questions

- **Resolved reconciliation**: The former next.md-era `siar-dtn` crate has been retired into `siar-dtn-bundle` (seen-bundle dedup ported) and local quota-bounded storage in `apps/emergency-node` (see [`MIGRATION.md`](../MIGRATION.md)).
- **No encounter protocol** (§27-31, §189 Phase 3) — no peer connection abstraction exists yet.
- **No routing integration for gateway/route selection** (§189 Phase 5) — `routing_bridge` bridges policy *into* a bundle at construction; nothing calls `siar-routing-policy` for an actual route plan.
- **No file/blob chunk carriage beyond the type-level reference** (§40-45, §189 Phase 6).
- **Quota/reserve policy exists but isn't wired to a real store yet**: `admission.rs` gives real, tested decision primitives (`StorageQuotas`, `is_reserved_class`, `PeerQuotaPolicy`, `admit_bundle`), but `store::InMemoryBundleStore` doesn't track per-peer or reserved-class byte usage anywhere — `admit_bundle`'s `quota_available` field is still a caller-supplied bool, not something this crate computes from its own store yet. §189 Phase 7's "emergency priority enforcement" is the reserve *policy* now existing; actual capacity reservation against a real store remains open.
- §32-35 (peer utility/destination likelihood/encounter history — deferred in the spec's own §188 scope list) and everything from roughly §46 onward not named in the round history above — still untouched.

## Note

Updated 2026-09-28 (round 4), transcribing §54-61. Fraction is an approximation (`~65/192`), not a full section-by-section audit — unlike spec 04, this crate hasn't yet had a dedicated tracker-building pass.
