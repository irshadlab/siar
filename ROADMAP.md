# siar — Roadmap & Uncompleted Work Tracker

Maintained alongside the code, not a substitute for it. This file tracks
**what remains to be built, known gaps, architecture remarks, notices, and priority sequencing**.

> [!NOTE]
> **Completed Implementation Details**: All round-by-round implementation histories,
> section-by-section trackers, test statistics, and completed architectural records live
> in their respective specification files under [`docs/implementation/`](file:///home/irshad/Projects/siar/docs/implementation/README.md).
> Read `docs/implementation/` when you need to know *what's actually written and how*;
> read this file when you need to know *what's left, open gaps, and where to look next*.

Legend:
- ✅ Done / substantial (detailed record in [`docs/implementation/`](file:///home/irshad/Projects/siar/docs/implementation/README.md))
- 🟡 Partial (open work tracked below)
- ⚪/⬜ Not started
- 🔒 Blocked on a prerequisite
- 📋 Reconciled (covered elsewhere, no new code needed)
- ◇ Conceptual / guidance section, no code artifact of its own

---

## Tier 0 — Foundational Core Crates (Parts 01-09)

| # | Crate | State | Uncompleted / Open Work | Detail Doc |
|---|---|---|---|---|
| 01 | siar-protocol-ext | ✅ 108/108 | Spec complete. | [`01-protocol-extension-system-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/01-protocol-extension-system-architecture.md) |
| 02 | siar-identity-multidevice | ✅ 204/204 | Spec complete. Seam integration and mobile platform hooks remaining. | [`02-multi-device-identity-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/02-multi-device-identity-architecture.md) |
| 03 | siar-routing-policy | ✅ 200/200 | Spec complete. | [`03-transport-routing-policy-engine-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/03-transport-routing-policy-engine-architecture.md) |
| 04 | siar-event-log | ✅ Substantial | Fuzz target needs nightly; no criterion benchmarks; domain `ReplicationAuthorizer`/`PayloadSealer`; outbox migration. | [`04-offline-event-log-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/04-offline-event-log-architecture.md) |
| 05 | siar-blob-manifest | 🟡 Substantial | ~38 sections open/deferred (unsolicited push, background quota sync). | [`05-robust-file-blob-subsystem-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/05-robust-file-blob-subsystem-architecture.md) |
| 06 | siar-dtn-bundle | 🟡 ~83/192 | ~109 sections remaining (encounter protocol, routing integration, store quota reservation wiring). | [`06-dtn-store-carry-forward-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/06-dtn-store-carry-forward-architecture.md) |
| 07 | siar-capability | 🟡 ~113/164 | Remaining: §1-3 overview, 71, 77, 79-83, 87-88, 99, 115-117, and §122-164 (testing matrix, phases, DoD). | [`07-capability-negotiation-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/07-capability-negotiation-architecture.md) |
| 08 | siar-resource-limits | ✅ ~192/193 | Remaining: wiring public facade (`ResourceRuntime`) into daemon runtime coordinator; prose §1-2, 186, 190, 192-193. | [`08-resource-limits-backpressure-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/08-resource-limits-backpressure-architecture.md) |
| 09 | siar-crash-recovery | ✅ 186/186 | Remaining: wiring recovery coordinator into node runtime (`siar-runtime`). | [`09-crash-recovery-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/09-crash-recovery-architecture.md) |

### Tier 0 Open Items & Gaps

- **Part 04 (Offline Event Log)**:
  - Coverage-guided fuzz target written but not run (requires nightly toolchain).
  - Criterion performance benchmarks.
  - Per-stream sync checkpoints (§53).
  - Domain implementations of `ReplicationAuthorizer` and `PayloadSealer`.
  - `MessageService` send path still relies on `siar-storage`'s `OutboxRepository` (see `siar-messaging::event_policy`).
  - Desktop projection-only reads unverified.
- **Part 05 (Robust File/Blob Subsystem)**:
  - ~38 open or deferred sections out of 210.
  - Remote PUSH without prior request / unprompted upload authorization.
  - Background quota reconciliation against physical disk usage.
- **Part 06 (DTN / Store-Carry-Forward)**:
  - ~109 sections untouched.
  - Encounter protocol (§27-31, §189 Phase 3): peer connection discovery abstraction.
  - Routing integration (§189 Phase 5): policy bridges into bundles, but no active route planning through `siar-routing-policy`.
  - File/blob chunk carriage beyond type-level references (§40-45, §189 Phase 6).
  - Quota and critical reserve enforcement against durable stores (currently caller-supplied boolean).
- **Part 07 (Capability Negotiation)**:
  - Sections §1-3 overview, 71, 77, 79-83, 87-88, 99, 115-117 open.
  - Sections §122-164 (testing matrix, formal phases, Definition of Done).
- **Part 08 (Resource Limits & Backpressure)**:
  - Integration of `ResourceRuntime` facade into the top-level daemon coordinator.
  - Prose-only sections: §1-2, 186, 190, 192-193.
- **Part 09 (Crash Recovery)**:
  - Honest wiring gap: wiring the recovery coordinator into the node runtime (`siar-runtime`).

---

## Cross-Spec Build Order (Parts 01-33) — Dependency Analysis

Notice and dependency analysis built to determine the optimal sequence for remaining core specifications, minimizing downstream rework:

### In-Degree Ranking (Excluding Self-References)

| Part | In-degree | Status | Part | In-degree | Status |
|---|---|---|---|---|---|
| 08 Resource Limits/Backpressure | **22** | ✅ Complete facade | 14 Proximity Abstraction | 8 | ⚪ |
| 10 Fuzzing/Test Suite | **19** | ⚪ Practice across crates | 15 QR/NFC Pairing | 7 | 🟡 Substantial (linking) |
| 13 Battery-Aware Scheduling | **18** | 🟡 `siar-power` (132 tests) | 11 Relay Self-Hosted Infra | 6 | ⚪ |
| 03 Transport Routing Policy | 17 | ✅ Spec complete | 22 WASM Components | 6 | ⚪ |
| 16 Daemon/Headless Runtime | 15 | 🟡 `siar-runtime` (159 tests) | 01 Protocol Extension | 5 | ✅ Spec complete |
| 19 C-ABI/FFI | **15** | ⚪ Intentionally late | 21 3rd-Party Protocol Ext | 5 | ⚪ |
| 04 Offline Event Log | 14 | ✅ Substantial | 28 Security/E2EE | 4 | 🟡 Substantial |
| 05 File/Blob Subsystem | 14 | 🟡 Substantial (27 rounds) | 29 Realtime Calls | 2 | ⚪ |
| 07 Capability Negotiation | 14 | 🟡 ~113/164 | 25 Android HW Media | 1 | ⚪ |
| 17 Emergency Priority | 13 | 🟡 Partial crates | 26 Audio DSP | 1 | ⚪ |
| 18 Network Diagnostics | 13 | ⚪ Diagnostics in runtime | 27 Android Build Automation | 1 | ⚪ |
| 12 Multipath Networking | 11 | ⚪ | 31 Notifications | 1 | 🟡 Desktop sink |
| 23 External Interop | 11 | ⚪ | 32 Search/Indexing | 1 | ⚪ |
| 02 Multi-Device Identity | 10 | ✅ Spec complete | 30 Presence/Typing | 0 | ⚪ |
| 09 Crash Recovery | 10 | ✅ Spec complete | 33 Backup/Restore | 0 | 🟡 `siar-backup` |
| 20 Embedded Linux Node | 10 | ⚪ | | | |
| 24 Plugin/Module Ecosystem | 10 | ⚪ Deliberately last | | | |
| 06 DTN Store-Carry-Forward | 9 | 🟡 ~83/192 | | | |

### Ranking Corrections & Ordering Notices

- **Part 10 (Fuzzing/Test Suite)**: High in-degree reflects continuous testing practices applied to each crate as it is built (e.g. mutation fuzzing in Part 28/16, property tests in Part 04/08), not a blocking standalone crate.
- **Part 19 (C-ABI/FFI)**: High in-degree, but bindings must stabilize after underlying Rust crates settle. Intentionally deferred until core APIs stop shifting.
- **Part 18 (Network Diagnostics)**: Visualizes and diagnoses paths across routing, DTN, multipath, and proximity. Sits downstream of networking layer implementations.

### Recommended Build Sequence

1. **Finish Tier 0 Core**: Complete remaining sections in 06 DTN and 07 Capability, wire 08 and 09 into `siar-runtime`.
2. **12 Multipath Networking**: Directly extends Part 03 (done).
3. **17 Emergency Priority Classes**: Consolidate `siar-emergency` and `siar-emergency-service` with event logging.
4. **14 Proximity Abstraction**: Radio abstraction for BLE/Wi-Fi Direct.
5. **15 QR/NFC Bootstrap Pairing**: Complete NFC exchange and threshold pairing.
6. **28 Security/E2EE**: Complete default OpenMLS pairwise migration and abuse gate ingress.
7. **11 Relay Self-Hosted Infra** → **18 Network Diagnostics** → **23 External Interop** → **20 Embedded Linux Node**.
8. **29 Realtime Calls** (wants stable ratchets/groups) → **30 Presence** → **31 Notifications** → **32 Search** → **33 Backup/Restore** GUI wizards.
9. **Platform Media (25/26/27)**: Android hardware surface, audio DSP, and build automation.
10. **19 FFI**: C-ABI bindings once APIs settle.
11. **Ecosystem & Extensibility (21/22/23/24)**: WASM components, third-party extensions, and plugin ecosystem (deliberately lowest priority).

---

## Tier 1 — Security Backbone (Part 28) Open Work

Full completed detail lives in [`28-production-security-e2ee-key-management-privacy-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/28-production-security-e2ee-key-management-privacy-architecture.md).

### Remaining Subsystems & Open Tasks

- **Default Pairwise MLS Migration**: Pairwise forward-secret messaging exists via OpenMLS (`siar-crypto-mls::pairwise`), but currently requires explicit upgrade. Migrating the default messaging path to OpenMLS pairwise (retiring static session) requires wire session upgrade and storage schema updates.
- **Ingress Gate Production Wiring**: `siar-abuse::Gate` is wrapped in `siar-messaging::IngressGuard` and emergency-node, but default production limits remain untuned for high-volume relays.
- **Security Safe Modes & Profiles**: §79-80 security safe mode and §106-110 security profiles.
- **Cross-Implementation Test Vectors**: §93 test vectors for hybrid composite signatures and BBS passes against external implementations.
- **Envelope Fuzzing**: Fuzz targets for legacy handshake and envelope parsing on nightly.
- **Audits & Reviews**: External independent review of FROST ceremony, hybrid signatures, and workspace-wide memory zeroization.

---

## Desktop UI / Slint — Known Gaps & Uncompleted Items

Full completed detail lives in [`ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md) and [`ui-ux-tracker.md`](file:///home/irshad/Projects/siar/docs/implementation/ui-ux-tracker.md).

> [!NOTE]
> Desktop toolkit migrated from Dioxus to **Slint** (`crates/siar-ui-slint` and `apps/desktop`).
> Verified live on Wayland / Niri display with 29 interaction and 10 render tests green.

### Open Gaps

- **Unimplemented UI Tabs**:
  - Realtime Calls (`ui-ux-07`): Stub tab pending backend audio/video pipeline.
  - File Media Gallery (`ui-ux-10`): Dedicated gallery browser (currently inline chat previews only).
  - Global Search (`ui-ux-11`): Search interface pending Part 32 indexing engine.
- **MLS Group Chat Limitations**:
  - No message history reload after restart (MLS forward secrecy deletes past decryption keys; requires encrypted plaintext-at-rest store).
  - Group unread counts are not persisted across restarts (in-session only).
  - Member removal UI action is not wired.
- **Window ⇄ Daemon Controls**:
  - No in-app "Restart Now" button (desktop window cannot restart its background daemon).
  - Network "Online" indicator reflects local transport endpoint binding, not probed internet/relay reachability.
- **Build / Packaging Toolchain**:
  - Slint pinned to `=1.16.1` (Slint 1.17+ requires rustc 1.92; workspace pinned to rustc 1.91).
  - Re-run `cargo hakari generate` locally.

---

## Device Linking & Multi-Device Contacts — Known Limits & Uncompleted Work

Full completed detail lives in [`15-qr-nfc-bootstrap-pairing-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/15-qr-nfc-bootstrap-pairing-architecture.md) and [`docs/MULTI_DEVICE_CONTACTS.md`](file:///home/irshad/Projects/siar/docs/MULTI_DEVICE_CONTACTS.md).

### Known Limits

- **Primary Device Custody**: Only the device that created the account holds `root.key` and can approve new devices. Joined devices cannot approve further devices without implementing root key sharing or threshold custody.
- **Identity Adoption**: Joining replaces the account ID on disk, but the running daemon core retains its initial ID until restarted (notified via global UI banner).
- **Silent Waiting on Error**: An invalid or expired link code currently causes the joining client to wait silently without displaying an instant rejection alert.
- **Multi-Device Sync Gaps**:
  - **Group MLS**: 1:1 text and file transfers fan out to linked devices, but MLS groups do not yet add sibling devices automatically.
  - **History Sync / Catch-Up**: New devices do not receive historic conversations from sibling devices.
  - **Dynamic Contact Pushing**: Contacts added *after* initial device linking are not automatically announced to sibling devices.
  - **Revocation Delivery**: Direct tombstone delivery to the revoked device itself is not guaranteed if offline.
  - **Per-Device Delivery Status**: UI displays conversation delivery status, not per-device delivery breakdowns.

---

## Tier 2 — UI/UX (27 Specs) Status Overview

Full tracking across all 27 UI/UX specifications lives in [`ui-ux-tracker.md`](file:///home/irshad/Projects/siar/docs/implementation/ui-ux-tracker.md).

### Untouched / Open UI Specs

- `ui-ux-03` Android Compose App Shell (⚪ unstarted)
- `ui-ux-07` Calls / Realtime Media (⚪ unstarted)
- `ui-ux-10` Files / Media Gallery (🟡 inline previews only)
- `ui-ux-11` Search & Retrieval (⚪ unstarted — joint gap with Part 32)
- `ui-ux-13` Notifications UI (🟡 daemon desktop notifications only)
- `ui-ux-14` Presence & Typing (🟡 delivery ticks only; typing ephemeral unstarted)
- `ui-ux-16` Backup / Restore GUI (🟡 CLI/daemon only; GUI wizard unstarted)
- `ui-ux-17` Emergency SOS UI (⚪ unstarted)
- `ui-ux-19` Plugin / Module UI (⚪ unstarted — joint gap with Part 24)
- `ui-ux-20` Network Path Diagnostics UI (🟡 card only; visual graph unstarted)

---

## Tier 3 — Toolchain / Sandbox Environment Notes

- **Toolchain Requirements**: Rustc 1.91.1 pinned (`rust-version = "1.91"`). Direct installation via `apt-get install` (`/usr/lib/rust-1.91/bin`).
- **System Dependencies**: `libgtk-3-dev libssl-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libwebkit2gtk-4.1-dev libasound2-dev cmake libopus-dev libdav1d-dev nasm pkg-config`.
- **Cargo Lock**: Workspace `Cargo.lock` uses v4 format. Do not delete or regenerate with older cargo.
- **Disk Space Constraint**: Workspace target builds require regular cleanup (`rm -rf target && apt-get clean`).

---

## Part 13 (`siar-power`) — Uncompleted & Open Items

Full completed detail lives in [`13-battery-aware-scheduling-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/13-battery-aware-scheduling-architecture.md).

- **Mobile Producers**: Rust interfaces (`HostReport` / `PushPowerSource`) complete; Android `BatteryManager`/Doze and iOS Kotlin/Swift producers not yet written.
- **Empirical Tuning**: Hardware parameter tuning (§99) and real-device benchmark suites (§130-136) require physical device benches.
- **Desktop Bulk Consumers**: §39-49 transfer limits have no desktop consumer beyond image attachment previews.
- **UI Event Notice Wiring**: GUI does not yet consume the semantic event outbox (§114) or thermal/battery user notices (§116).

---

## Part 16 (`siar-runtime` / `apps/daemon`) — Uncompleted & Open Items

Full completed detail lives in [`16-daemon-headless-runtime-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/16-daemon-headless-runtime-architecture.md).

- **UI Sync Optimization**: Serialization currently re-encodes sections per diff; needs dirty-tracking before scaling to thousands of conversations.
- **Windows Named Pipes**: Implemented via `interprocess` but unverified on real Windows hosts.
- **Mobile Daemon Hosting**: Android foreground service hosting and iOS background execution windows (§42-46).
- **Multi-Account inside One Daemon**: Per-account directory namespaces exist; account-scoped command ledgers inside one daemon process are not built.
- **Process Isolation**: Plugin-host sandboxing (§120).
- **IPC Enhancements**: Unix FD passing (§63).
- **Remote Restore Automation**: Remote restore stages files in maintenance mode; automated process restart is not yet built.
- **Storage Node Binary**: Provisioning supports `StorageNode`, but no storage daemon binary exists yet.

---

## Part 33 (Backup / Restore) — Uncompleted Items

Full completed detail lives in [`33-backup-restore-export-import-archival-portability-architecture.md`](file:///home/irshad/Projects/siar/docs/implementation/33-backup-restore-export-import-archival-portability-architecture.md).

- **GUI Backup & Restore Wizard**: Currently exposed through `siar-desktop backup/restore` and daemon `ctl backup/restore`.
- **Automated Scheduling**: Scheduled automated background backup jobs.
- **Selective Data Export**: Exporting individual conversations or contacts.

---

## Suggested Next Priorities

1. **Tier 0 Completion**:
   - Complete remaining sections of **06 DTN Store-Carry-Forward** and **07 Capability Negotiation**.
   - Wire **08 Resource Limits** facade (`ResourceRuntime`) and **09 Crash Recovery** into `siar-runtime`.
2. **Security & Messaging Hardening**:
   - Migrate default messaging path to forward-secret **1:1 Pairwise OpenMLS**.
   - Tune and enforce `siar-abuse` ingress gates across production traffic.
3. **Core Networking Expansion**:
   - Implement **12 Multipath Networking** (extending Part 03).
   - Implement **14 Proximity Abstraction** (BLE / Wi-Fi Direct).
4. **UI Feature Completion**:
   - Realtime media calling UI & pipeline (`ui-ux-07` / Part 29).
   - Plaintext-at-rest encrypted storage for MLS group history across restarts.
