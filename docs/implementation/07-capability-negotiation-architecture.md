# Part 07 — Capability Negotiation Architecture

Source spec: `sys-arch/07-capability-negotiation-architecture.md` — 164 numbered sections (verified via `grep -c '^# [0-9]*\.'`).

## Implementation status

**~122/164 (74%) sections** (by section references in source; rounds N+1..N+5 completed).
144 tests passing (136 unit + 8 property). Clippy `-D warnings` and fmt clean.

## Implementing crate(s)

- `siar-capability` — capability advertisements, hierarchical negotiation, lazy negotiation, directional negotiation, trust separation, UI adapter, preference ordering, route filtering, snapshot persistence, dynamic resource limits, file/DTN/routing fallbacks, and session resume.

## Round-by-round history

### Round N+5 (§115, §116, §117)

Added +3 sections:
- **§115 File Transfer Fallbacks** (`fallback`): Degraded file transfer plans (`FileStrategy::SingleStreamResumable`, `SingleStreamOneShot`, `ParallelOneShot`, `ParallelResumable`); reports `None` if fixed chunking was not agreed.
- **§116 DTN Bundle Fallbacks** (`fallback`): Selective bundle forwarding based on class (`Small` vs `BlobChunk`); immediately transmits text/SOS bundles while deferring large blob chunks if relay is unsupported; refuses bundles exceeding negotiated ceiling sizes.
- **§117 Routing Fallbacks** (`fallback`): Constructs transport fallback chains adhering to caller preference order and determines the next viable transport after failures.

### Round N+4 (§71, §79, §82-83, §87-88)

Added +6 sections:
- **§71 Preference Ordering** (`preference`): Mode selection governed by *local preference → remote support → policy*; peer support operates strictly as a membership filter, preserving local authority.
- **§79 Route Integration** (`route_filter`): Candidate session filtering for routing operations by capability requirements (`Capable`, `Lacks(id)`, `NotNegotiated`).
- **§82–83 Event Log & Snapshot Persistence** (`snapshot`): Persisting `last_verified_peer_capabilities` as a bounded cache (`MAX_SNAPSHOT_BYTES = 64 KiB`) with integrity hashes and timestamps; gating writes via `PersistGate`.
- **§87–88 Resource Capabilities & Dynamic Limits** (`resource_limits`): Negotiated limits modeled as ceilings with dynamic reduction under storage, battery, or memory pressure.

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
- §77, 80-81 (DTN/files wiring in their crates), 99
- §122-164 (testing matrix, phases, formal definition of done audit)
