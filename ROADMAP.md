# siar — Roadmap & Coverage Hierarchy

Maintained alongside the code, not a substitute for it. This file tracks
**what's written vs. what's left**, ordered by actual need — not spec
number order. Update this file every time a spec's coverage changes.

Spec source: `sys-arch/` (33 numbered core-architecture docs +
27 `ui-ux-NN` docs, uploaded once, Wiki-style). Workspace source:
33-crate Rust workspace (`siar-source`) + `apps/{cli,desktop,android,
emergency-node}` + `platform/android`.

Legend: ✅ Done/substantial · 🟡 Partial · ⚪/⬜ Not started · 🔒 Blocked on
a prerequisite · 📋 Reconciled (already covered elsewhere, no new code
needed) · ◇ Conceptual/guidance section, no code artifact of its own.

**2026-09-27 reorganization**: this file used to carry the full
round-by-round narrative and, for spec 04, a complete 95-row
section-by-section tracker inline. That level of detail now lives in
each spec's own `docs/docs/NN-*.md` file — this file is a **map**:
which specs/sections are done, substantial, partial, untouched, or
conceptual, and where to go for the full story. Read a `docs/docs/`
file when you need to know *why* or *how*; read this file when you
need to know *how much* and *where to look next*. `docs/docs/04-*.md`
is the fullest example of what a per-spec file looks like once it has
its own section-by-section tracker — not every spec below has one yet.

---

## Tier 0 — Foundational core crates (Parts 01-09)

All nine have real, compile+test-verified coverage, tracked crate-by-
crate below. Full round-by-round history for every row now lives in
that spec's own `docs/docs/NN-*.md` file (linked in the table) — this
table is a live pointer, not a duplicate of that detail.

| # | Crate | State | Detail |
|---|---|---|---|
| 01 | siar-protocol-ext | ✅ **108/108 — spec complete** | `docs/docs/01-protocol-extension-system-architecture.md` |
| 02 | siar-identity-multidevice | ✅ **204/204 — spec complete** (21-item Definition-of-Done self-audit: 19/21 done, 2 honestly partial) | `docs/docs/02-multi-device-identity-architecture.md` |
| 03 | siar-routing-policy | ✅ **200/200 — spec complete** (19 rounds; own Definition-of-Done self-audit included) | `docs/docs/03-transport-routing-policy-engine-architecture.md` |
| 04 | siar-event-log | 🟡 **27✅/24🟡/35⬜/9◇ of 95** — Phases 1-4 done; Phase 5 (outbox) deliberately deferred, not decided; §63 closed by new `siar-event-registry` crate | `docs/docs/04-offline-event-log-architecture.md` |
| 05 | siar-blob-manifest | 🟡 **~93/210** | `docs/docs/05-robust-file-blob-subsystem-architecture.md` |
| 06 | siar-dtn-bundle | 🟡 **~65/192** | `docs/docs/06-dtn-store-carry-forward-architecture.md` |
| 07 | siar-capability | 🟡 **~19/164** | `docs/docs/07-capability-negotiation-architecture.md` |
| 08 | siar-resource-limits | 🟡 **~56/193** | `docs/docs/08-resource-limits-backpressure-architecture.md` |
| 09 | siar-crash-recovery | 🟡 **~15/186** (earliest-stage of the nine) | `docs/docs/09-crash-recovery-architecture.md` |

**Three real, unresolved reconciliation questions**, deliberately not
silently resolved — documented in the newer crate's own `lib.rs` in
each case, and tracked in detail in `MIGRATION.md`:
- two device-cert models (`siar_crypto::device_cert` vs
  `siar-identity-multidevice`)
- two routing/scoring systems (`siar-routing` vs `siar-routing-policy`)
- two DTN bundle models (`siar-dtn` vs `siar-dtn-bundle`)

---

## Cross-spec build order (Parts 01-33) — dependency analysis

Built 2026-09-27, at the user's own request, to answer a real question:
after Tier 0, in what order should the remaining 24 core specs be
built to minimize having to come back and rework something because a
prerequisite showed up late?

**Method**: every `sys-arch/NN-*.md` file was grepped for `Part NN`
cross-references to every other spec. This is a real measurement, not
a guess — but it's a heuristic, not a verified hard-blocking graph:
most of these mentions are "this relates to / this is consumed by,"
not a compiler-enforced dependency, so treat the resulting order as a
strong prior to plan around, not a rule that can never be reordered.
**In-degree** (how many *other* specs' own text points at a given
spec) is the signal that matters most for sequencing: a spec many
others reference is one whose absence forces the most future rework if
built late, regardless of how many things *that* spec itself
references (a spec can reference dozens of "related areas" without any
of them being a real prerequisite for it).

### Full in-degree ranking (excluding self-references)

| Part | In-degree | Status | Part | In-degree | Status |
|---|---|---|---|---|---|
| 08 Resource Limits/Backpressure | **22** | 🟡 Tier 0 | 14 Proximity Abstraction | 8 | ⚪ |
| 10 Fuzzing/Test Suite | **19** | ⚪ — see note below | 15 QR/NFC Pairing | 7 | ⚪ |
| 13 Battery-Aware Scheduling | **18** | ⚪ | 11 Relay Self-Hosted Infra | 6 | ⚪ |
| 03 Transport Routing Policy | 17 | ✅ Tier 0 | 22 WASM Components | 6 | ⚪ |
| 16 Daemon/Headless Runtime | 15 | ⚪ | 01 Protocol Extension | 5 | ✅ Tier 0 |
| 19 C-ABI/FFI | **15** | ⚪ — see note below | 21 3rd-Party Protocol Ext | 5 | ⚪ |
| 04 Offline Event Log | 14 | 🟡 Tier 0 | 28 Security/E2EE | 4 | 🟡 Tier 1 |
| 05 File/Blob Subsystem | 14 | 🟡 Tier 0 | 29 Realtime Calls | 2 | ⚪ |
| 07 Capability Negotiation | 14 | 🟡 Tier 0 | 25 Android HW Media | 1 | ⚪ |
| 17 Emergency Priority | 13 | 🟡 (partial crate) | 26 Audio DSP | 1 | ⚪ |
| 18 Network Diagnostics | 13 | ⚪ — see note below | 27 Android Build Automation | 1 | ⚪ |
| 12 Multipath Networking | 11 | ⚪ | 31 Notifications | 1 | ⚪ |
| 23 External Interop | 11 | ⚪ | 32 Search/Indexing | 1 | ⚪ |
| 02 Multi-Device Identity | 10 | ✅ Tier 0 | 30 Presence/Typing | 0 | ⚪ |
| 09 Crash Recovery | 10 | 🟡 Tier 0 | 33 Backup/Restore | 0 | ⚪ |
| 20 Embedded Linux Node | 10 | ⚪ | | | |
| 24 Plugin/Module Ecosystem | 10 | ⚪ (deliberately last) | | | |
| 06 DTN Store-Carry-Forward | 9 | 🟡 Tier 0 | | | |

**Two ranking corrections — high in-degree that should NOT jump the
queue:**
- **Part 10 (Fuzzing/Test Suite)** ranks 2nd, but it isn't a phase to
  schedule — it's a *practice* applied to each crate as it's built
  (exactly what this workspace already does: `siar-event-log`'s crash
  injection/property tests, `siar-blob-manifest`'s tamper tests). Its
  high in-degree just reflects how many specs say "add fuzz/property
  tests per Part 10," not that Part 10 has to exist as its own crate
  before anything else can proceed.
- **Part 19 (C-ABI/FFI)** ranks in the top 5, but binding generation
  against an API that hasn't stabilized yet is exactly the kind of
  back-and-forth this exercise is trying to avoid — build bindings
  once the crates underneath them stop shifting, not before. Treat as
  deliberately late despite the raw score.
- **Part 18 (Network Diagnostics)** has real in-degree, but reads the
  other way around: it *visualizes and diagnoses paths across
  everything else* (routing, DTN, multipath, proximity) — it needs
  those to exist first to have anything to show, so it naturally sits
  after most of the networking-layer specs, not before them.

### Recommended order after Tier 0

Tier 0 (01-09) continues in the standing order (finish 06 → 07 → 08 →
09) — the ranking above *confirms* that plan rather than overriding
it: 07 and 08 are the single highest true "build this or everything
downstream reworks around its absence" items in the entire 33-spec set
outside what's already done.

1. **13 Battery-Aware Scheduling** — 2nd-highest real (non-corrected)
   in-degree; a cross-cutting policy layer nearly every
   networking/background spec below needs to respect (duty-cycling,
   scan frequency, discovery aggressiveness).
2. **16 Daemon/Headless Runtime** — defines how the whole system runs
   without a UI; `apps/emergency-node` and any embedded/daemon work
   depend on this existing first.
3. **12 Multipath Networking** — extends Part 03 (done) directly; nothing
   new to wait on.
4. **17 Emergency Priority Classes** — already has real partial crates
   (`siar-emergency`, `siar-emergency-service`) from the Tier 0 event-
   integration work; continuing it is lower-effort than starting cold.
5. **14 Proximity Abstraction** — a named prerequisite for #6 below.
6. **15 QR/NFC Bootstrap Pairing** — **this is the concrete reason Part
   28 isn't next**: Part 28's own tracked gap (§37-39 SAS/pairing, no
   `protocol_version` field on `DeviceLinkInvite`) is a pairing-flow
   problem, and Part 15 is where the real pairing flow this gap needs
   to reconcile against actually gets built. Doing 28's own pairing
   work before 15 exists means guessing at a shape that would likely
   need reworking once 15 lands.
7. **28 Security/E2EE** — resume here, now with Part 15 as real
   infrastructure to reconcile the SAS/pairing gap against, rather than
   an assumption.
8. **11 Relay Self-Hosted Infra**, then **18 Network Diagnostics**
   (now that most of the networking layer above actually exists to
   diagnose), then **23 External Interop**, **20 Embedded Linux Node**.
9. **29 Realtime Calls** (wants Part 28's ratchet/groups reasonably
   solid first), then **30 Presence**, **31 Notifications** (both
   reference 28 and 29), **32 Search**, **33 Backup** (references the
   most other specs of anything in the set — 28, 31, 32 among them —
   and has zero in-degree itself, the textbook "build last" signature).
10. **25/26/27** (Android hardware media, audio DSP, build automation)
    — platform-specific, slot in whenever Android work is prioritized
    independently of this ordering.
11. **19 FFI**, last among the "core" specs, once the APIs it binds
    have actually stopped changing.
12. **21/22/23/24** (third-party protocol extensions, WASM, plugin
    ecosystem) — already the standing call in "Suggested next
    priorities" below: extensibility/ecosystem work is deliberately
    lowest priority until the core product is solid, and this ranking
    doesn't change that call.

---

## Tier 1 — Security backbone (Part 28)

**~46/127 (36%) done.** Four remaining items are each their own
subsystem (the ratchet, group security/MLS reconciliation, a security
test-vector/fuzzing harness, a 10-crate workspace split) rather than a
next incremental batch — recommendation is to pause here until one of
those four is deliberately prioritized. Full detail, the closed/open
section ranges, and the four-item table: `docs/docs/28-production-security-e2ee-key-management-privacy-architecture.md`.

---

## Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

**1 of 27 specs touched (partially)** — `ui-ux-15` Security Center is
~83% of 221 sections; the other 26 are ⚪, several sharing a "joint
gap" with a still-unstarted core-arch spec (search, QR/NFC pairing,
notifications, presence, backup, plugins, diagnostics). `ui-ux-22`
Design System is the standing next-priority candidate, since most of
the rest of this tier visually depends on it existing first. Full
27-spec table and `ui-ux-15`'s own detail:
`docs/docs/ui-ux-tracker.md`.

---

## Tier 3 — Toolchain / sandbox environment note

Historical constraint, now resolved: this sandbox's verification
environment used to be stuck on rustc 1.75 with no path to the
workspace's pinned `rust-version = "1.91"`. **Correction (still
current)**: `rustc-1.91`/`cargo-1.91`/`rust-1.91-clippy`/`rustfmt-1.91`
install directly via `apt-get install` (`archive.ubuntu.com` is
allowlisted), landing at `/usr/lib/rust-1.91/bin`. Combined with the
right system libs (`libgtk-3-dev libssl-dev libsoup-3.0-dev
libjavascriptcoregtk-4.1-dev libwebkit2gtk-4.1-dev libasound2-dev
cmake libopus-dev libdav1d-dev`), `cargo check --workspace
--all-targets` against the real repo-root `Cargo.lock` (v4 format —
parses fine under cargo 1.91; don't delete/regenerate it) passes clean
across all 33 crates, including `apps/desktop`. Disk space is tight —
expect to `rm -rf target && apt-get clean` between big check runs. Only
genuine device/emulator/hardware-codec behavior (`apps/android`'s real
Kotlin/JNI runtime, hardware codecs) remains out of reach here.

Each new sandbox session for this project starts with **no Rust
toolchain installed at all** (not even 1.75) — install fresh every
time; never assume a prior session's toolchain state still holds.

---

## Suggested next priorities, in order

Tier 0's nine core specs are being worked through one by one, in spec
order, per standing user instruction, before moving on. Specs 01-03
are complete. Spec 04 has Phases 1-4 done, Phase 5 deliberately
deferred (see `docs/docs/04-*.md`). Spec 05 is 2 rounds into an
ordinary section-by-section pass (see `docs/docs/05-*.md`). Specs
06-09 remain untouched since their last recorded aggregate count.

**Once Tier 0 is done, the "Cross-spec build order" section above is
the primary sequencing for Parts 10-33** — it supersedes the older,
less rigorous list this section used to carry. The two tracks below
run alongside it, not instead of it:

- **UI/UX**: `ui-ux-15` continues in ordinary batches (§195-221
  remain — integration section §195-206 next, then testing/final scope
  §207-221). `ui-ux-22` Design System is worth pulling forward once
  Tier 0 ends, ahead of most core-arch work above it in the ranking —
  everything else in that tier visually depends on it, and no core-arch
  ranking captures that (`ui-ux` specs weren't part of the reference-
  graph measurement). The "joint gaps" pattern noted before (search,
  QR/NFC pairing, notifications, presence/receipts, backup, plugins,
  diagnostics — each a core-arch spec paired with its own ⚪ ui-ux
  counterpart) still holds; the cross-spec order above already
  sequences the core-arch half of each pair, so build that spec's own
  ui-ux counterpart in the same window rather than as a separate later
  effort.
- **Deliberately last, by explicit priority call**: plugin/module
  ecosystem (Part 24, ui-ux-19), WASM components (Part 22), third-party
  protocol extensions (Part 21), external interoperability (Part 23).
  Usability and reliability come first; extensibility/ecosystem work
  is lower priority until the core product is solid — the cross-spec
  ranking's own in-degree numbers agree (all four rank in the bottom
  third).
- **The four Part 28 subsystems** (ratchet, groups/MLS, test harness,
  crate split) — each needs its own dedicated, scoped effort rather
  than sharing a round with anything else, whenever Part 28 is resumed
  per the cross-spec order above.
