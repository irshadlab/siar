# Part 09 — Crash Recovery Architecture

Source spec: `sys-arch/09-crash-recovery-architecture.md` — 186 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**✅ 186/186 — spec complete** (all sections implemented or consciously not built; §10 WAL = documented do-not-build; §182 three deferrals recorded).
Definition of Done (§184) is an executable test audit: 16 Done / 4 PartiallyDone with the missing piece named, 0 failing. §186 final principle verified clause by clause.
495 unit + 3 integration tests. Clippy `-D warnings` clean across test, release, and `crash-injection` profiles; `cargo doc -D warnings` clean.

## Implementing crate(s)

- `siar-crash-recovery` — crash detection, panic hooks, corruption quarantine, state recovery coordinator, recovery phases, idempotent initialization pipelines, and self-auditing Definition of Done.

## Architecture and Key Components

- **Recovery Failure Taxonomy (§180-186)**: Implemented spec's formal 10-variant error enum `RecoveryFailure`.
- **Scope & Phases Data Modeling**: `scope_and_phases` (§180/182/183/185) verified directly against workspace source tree and `Cargo.toml`.
- **Executable Definition of Done (§184)**: `definition_of_done.rs` provides an executable, test-verified audit of all recovery requirements.
- **Consciously Not Built**: §10 WAL is a documented do-not-build decision (delegated to backing database engine transactional logging); §182 records the 3 deferrals.

## Known gaps / integration tasks

- **Workspace Wiring**: ✅ Completed. `siar-crash-recovery` is wired into `apps/daemon` via `run_marker::RunMarker` (§138–§139), tracking clean/unclean shutdowns, feeding `RecoveryMode` into daemon startup, and registering recovery state with `runtime.health`.
