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
each spec's own `docs/NN-*.md` file — this file is a **map**:
which specs/sections are done, substantial, partial, untouched, or
conceptual, and where to go for the full story. Read a `docs/`
file when you need to know *why* or *how*; read this file when you
need to know *how much* and *where to look next*. `docs/04-*.md`
is the fullest example of what a per-spec file looks like once it has
its own section-by-section tracker — not every spec below has one yet.

---

## Tier 0 — Foundational core crates (Parts 01-09)

All nine have real, compile+test-verified coverage, tracked crate-by-
crate below. Full round-by-round history for every row now lives in
that spec's own `docs/NN-*.md` file (linked in the table) — this
table is a live pointer, not a duplicate of that detail.

| # | Crate | State | Detail |
|---|---|---|---|
| 01 | siar-protocol-ext | ✅ **108/108 — spec complete** | `docs/01-protocol-extension-system-architecture.md` |
| 02 | siar-identity-multidevice | ✅ **204/204 — spec complete** (21-item Definition-of-Done self-audit: 19/21 done, 2 honestly partial) | `docs/02-multi-device-identity-architecture.md` |
| 03 | siar-routing-policy | ✅ **200/200 — spec complete** (19 rounds; own Definition-of-Done self-audit included) | `docs/03-transport-routing-policy-engine-architecture.md` |
| 04 | siar-event-log | 🟡 **27✅/24🟡/35⬜/9◇ of 95** — Phases 1-4 done; Phase 5 (outbox) deliberately deferred, not decided; §63 closed by new `siar-event-registry` crate | `docs/04-offline-event-log-architecture.md` |
| 05 | siar-blob-manifest | 🟡 **~32/210** | `docs/05-robust-file-blob-subsystem-architecture.md` |
| 06 | siar-dtn-bundle | 🟡 **~50/192** | `docs/06-dtn-store-carry-forward-architecture.md` |
| 07 | siar-capability | 🟡 **~19/164** | `docs/07-capability-negotiation-architecture.md` |
| 08 | siar-resource-limits | 🟡 **~56/193** | `docs/08-resource-limits-backpressure-architecture.md` |
| 09 | siar-crash-recovery | 🟡 **~15/186** (earliest-stage of the nine) | `docs/09-crash-recovery-architecture.md` |

**Three real, unresolved reconciliation questions**, deliberately not
silently resolved — documented in the newer crate's own `lib.rs` in
each case, and tracked in detail in `MIGRATION.md`:
- two device-cert models (`siar_crypto::device_cert` vs
  `siar-identity-multidevice`)
- two routing/scoring systems (`siar-routing` vs `siar-routing-policy`)
- two DTN bundle models (`siar-dtn` vs `siar-dtn-bundle`)

---

## Tier 1 — Security backbone (Part 28)

**~46/127 (36%) done.** Four remaining items are each their own
subsystem (the ratchet, group security/MLS reconciliation, a security
test-vector/fuzzing harness, a 10-crate workspace split) rather than a
next incremental batch — recommendation is to pause here until one of
those four is deliberately prioritized. Full detail, the closed/open
section ranges, and the four-item table: `docs/28-production-security-e2ee-key-management-privacy-architecture.md`.

---

## Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

**1 of 27 specs touched (partially)** — `ui-ux-15` Security Center is
~83% of 221 sections; the other 26 are ⚪, several sharing a "joint
gap" with a still-unstarted core-arch spec (search, QR/NFC pairing,
notifications, presence, backup, plugins, diagnostics). `ui-ux-22`
Design System is the standing next-priority candidate, since most of
the rest of this tier visually depends on it existing first. Full
27-spec table and `ui-ux-15`'s own detail:
`docs/ui-ux-tracker.md`.

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
order, per standing user instruction, before returning to the list
below. Specs 01-03 are complete. Spec 04 has Phases 1-4 done, Phase 5
deliberately deferred (see `docs/04-*.md`). Spec 05 is the current
active crate, mid-way through an ordinary section-by-section pass (see
`docs/05-*.md` for the most recent round). Specs 06-09 remain
untouched since their last recorded aggregate count.

Original list, resumes once Tier 0 is done:

1. `ui-ux-15` **continue in ordinary batches** (§195-221 remain): next
   natural slice continues the integration section (§195-206 — Privacy
   Settings Integration onward), then testing matrix + final scope
   (§207-221) — likely finishes the spec in 2-3 more rounds.
2. **`ui-ux-22` Design System** — tokens/typography/icons/motion that
   every other visual spec in this tier implicitly depends on; doing it
   later means retrofitting styling into everything built before it.
3. **Part 28 §46-92 in ordinary batches** — abuse resistance and
   embedded/plugin/FFI security boundaries are the two sub-areas most
   likely to matter soon given `siar-protocol-ext`'s existing extension
   mechanism (Part 01) and the still-unstarted Parts 21/24
   (third-party extensions / plugin ecosystem).
4. **Joint gaps** (spec pairs where both the core-arch and ui-ux spec
   are ⚪): 11+32 (search), 12+15 (QR/NFC pairing), 13+31
   (notifications), 14+30 (presence/receipts), 16+33 (backup), 19+24
   (plugins), 20+18 (diagnostics). Each pair is naturally one unit of
   work (backend + its UI together), not two separate efforts.
5. **Deliberately last, by explicit priority call**: plugin/module
   ecosystem (Part 24, ui-ux-19), WASM components (Part 22), third-party
   protocol extensions (Part 21), external interoperability (Part 23).
   Usability and reliability come first; extensibility/ecosystem work
   is lower priority until the core product is solid.
6. **The four Part 28 subsystems** (ratchet, groups/MLS, test harness,
   crate split) — each needs its own dedicated, scoped effort rather
   than sharing a round with anything else.
