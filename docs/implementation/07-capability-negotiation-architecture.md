# Part 07 — Capability Negotiation Architecture

Source spec: `sys-arch/07-capability-negotiation-architecture.md` — 164 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~113/164 (69%) sections** (by section references in source; rounds N+1..N+3 completed).
122 tests passing (114 unit + 8 property). Clippy `-D warnings` and fmt clean.

## Implementing crate(s)

- `siar-capability` — capability advertisements, hierarchical negotiation, lazy negotiation, directional negotiation, trust separation, UI adapter, and session resume.

## Round-by-round history

### Rounds N+1 through N+3

Substantial expansion across rounds N+1 through N+3. Round N+3 added +15 sections:
- **§57-60**: UI capability adapter & feature presentation.
- **§75-76**: Lazy & hierarchical negotiation sessions.
- **§84-86**: Failure policy and graceful degradation.
- **§97-98**: Directional capabilities (`direction::negotiate_directional` ensuring send/receive asymmetry is correctly reconciled rather than naive bitwise AND).
- **§104-107**: Trust separation and session resume tokens.

### 2026-10-02 refactor and audit pass

Defects fixed in `siar-capability`:
- `BoundedBytes` derived `Deserialize`, bypassing its 256-byte bound for peer-supplied bytes; hardened to enforce bounds during deserialization.
- `HierarchicalSession::finish` honoured results for extensions never begun / unsupported, so a stray `Ok` made an unsupported extension usable and a forged fatal `Err` could terminate a healthy session (found via property tests for session invariants).
- Directional capabilities: replaced generic bitset intersection (which incorrectly kept "send" between two send-only peers) with `direction::negotiate_directional`.

## Known gaps / open sections

Remaining sections to address:
- §1-3 overview
- §71, 77, 79-83, 87-88, 99, 115-117
- §122-164 (testing matrix, phases, formal definition of done audit)
