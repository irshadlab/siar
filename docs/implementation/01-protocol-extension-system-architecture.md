# Part 01 — Protocol Extension System Architecture

Source spec: `sys-arch/01-protocol-extension-system-architecture.md` — 108 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**108/108 (100%) sections.**

**Spec complete.** All 108 sections closed across 6 rounds, 115/115 tests, ~2000 lines across 16 files, clippy clean under `-D warnings`. Zero regressions caused downstream (siar-resource-limits, siar-routing-policy both depend on this crate).

## Implementing crate(s)

- `siar-protocol-ext`


## Known gaps / open questions

- Round 1 found ExtensionDescriptor had no way to mark a capability required-within-an-extension (only whole-extension required/optional) — closed by adding `required_capabilities`, wired into `negotiate()`.

- Round 6's `definition_of_done.rs` self-audit honestly reports 4 unsatisfied items: 2 concrete-extension-type demos that don't exist by design, `ExtensionContext`'s placeholder fields, and no cargo-fuzz suite yet.


## Note
Detail above is transcribed from ROADMAP.md and project working notes as of 2026-09-01. This file has not yet been independently re-verified line-by-line against the current source tree — treat the section fraction as the last real count, not a live measurement.
