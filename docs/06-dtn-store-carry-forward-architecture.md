# Part 06 — DTN / Store-Carry-Forward Architecture

Source spec: `sys-arch/06-dtn-store-carry-forward-architecture.md` — 192 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**50/192 (26%) sections.**

Partial.

## Implementing crate(s)

- `siar-dtn-bundle`

## Known gaps / open questions

- **Resolved reconciliation**: The former next.md-era `siar-dtn` crate has been retired into `siar-dtn-bundle` (seen-bundle dedup ported) and local quota-bounded storage in `apps/emergency-node` (see [`MIGRATION.md`](../MIGRATION.md)).


## Note
Detail above is transcribed from ROADMAP.md and project working notes as of 2026-09-01. This file has not yet been independently re-verified line-by-line against the current source tree — treat the section fraction as the last real count, not a live measurement.
