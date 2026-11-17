# Part 28 — Production Security / E2EE Key Management / Privacy Architecture

Source spec: `sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md` — 127 numbered sections.

## Implementation status

**~46/127 (36%) done.** Assessment carried over from ROADMAP.md: a good stopping point for incremental section-by-section work. What's built covers the sections that unblock everything downstream (message envelopes, replay protection, key storage abstraction, device revocation, trust states, domain separation, safety fingerprints, identity-change UX).

Already closed: §5-10, §14-24, §28-36, §40-44.

Partially reconciled: §37-39 (SAS/pairing) — a real, named, unfixed gap: no `protocol_version` field on `DeviceLinkInvite`, deliberately not added because it would be a breaking change to an already-shipped signed struct.

## What's left is dominated by four items, each its own subsystem rather than a next "batch"

| Item | Sections | Why it's not a batch |
|---|---|---|
| The ratchet (forward secrecy / PCR) | §11-13 | Needs a real `vodozemac` integration, not hand-rolled crypto |
| Group Security | §25-27 | Needs reconciling against `siar-crypto-mls` (831 lines, unexamined) |
| Test-vector suite + fuzzing + named attack scenarios | §93-103 | A whole security test harness |
| Workspace crate split | §112-122 | The spec's own ask: split into 10 `comm-security-*` crates — a restructuring decision, not a code batch |

## Untouched, but NOT subsystem-scale (genuinely next-batch-sized whenever picked back up)

- §46-92 — abuse resistance, plugin/FFI/embedded boundaries, crash-safe ratchets, diagnostics, failure taxonomy, threat-model docs, ADRs, disclosure process, compromise-response playbooks.
- §104-111 — performance, security profiles.

## Implementing crate(s)

- `siar-crypto` — device certificates, message envelopes, replay protection, key storage abstraction.
- `siar-identity-multidevice` — device revocation, trust states, safety fingerprints, identity-change UX (see `docs/02` for that crate's own much larger spec-02 scope; this is the slice of its work that also satisfies spec 28 sections).
- `siar-crypto-mls` — 831 lines, unexamined against Group Security's own §25-27 requirements; the reconciliation this subsystem needs hasn't been done.

## Known gaps / open questions

- Two device-cert models exist (`siar_crypto::device_cert` vs `siar-identity-multidevice`) — a real, unresolved reconciliation question, documented in the newer crate's own `lib.rs` rather than silently resolved. Not specific to this spec alone; the same question is tracked in `MIGRATION.md`.
- §37-39's missing `protocol_version` field — a deliberate, named, unfixed gap (breaking change to an already-shipped signed struct).
- The four subsystem items above (ratchet, group security, test harness, crate split) each need a dedicated, scoped effort — none should share a round with anything else.

## Recommendation (carried over from ROADMAP.md)

Pause Part 28 here. Come back to §46-92/§104-111 in ordinary batches later; treat the four subsystem items as their own dedicated project when prioritized.

## Note

Transcribed from ROADMAP.md's Tier 1 section on 2026-09-27. This is the first docs file for spec 28 — no prior stub existed. No section-by-section tracker like spec 04's has been built for this spec; the aggregate ~46/127 and the closed-section ranges above are as recorded in ROADMAP.md, not from a fresh audit.
