# Tier 2 — UI/UX (27 specs, `ui-ux-01` through `ui-ux-27`)

No raw `sys-arch/ui-ux-NN-*.md` source documents were uploaded alongside the 33 core architecture specs, so unlike Parts 01-33 this tier had no per-spec source file initially. This tracker records implementation coverage across the 27 UI/UX specs.

## Desktop Toolkit Transition: Dioxus → Slint (2026-10-22)

The desktop shell was migrated from Dioxus to native **Slint** in `crates/siar-ui-slint` with `apps/desktop` as an unprivileged client communicating with `siar-daemon` via `siar-ui-sync`. Android remains Jetpack Compose.

## Implementation status

Substantial coverage implemented in `crates/siar-ui-slint` and `siar-ui-state`, covering the app shell (`ui-ux-02`), design system tokens (`ui-ux-22`), inbox & chat timeline (`ui-ux-04/05/06`), contacts (`ui-ux-08`), MLS groups (`ui-ux-09`), device linking (`ui-ux-12`), and Security Center (`ui-ux-15`).

| # | Spec | State | Notes |
|---|---|---|---|
| 01 | Product Foundation / Cross-Platform Interaction | 🟡 | Shared interaction rules between desktop Slint and mobile |
| 02 | Desktop App Shell / Navigation (Slint) | ✅ | Implemented in `crates/siar-ui-slint` & `apps/desktop` |
| 03 | Android (Compose) App Shell / Navigation | ⚪ | `apps/android` pre-existing Compose shell, unreconciled |
| 04 | Conversation List / Inbox | ✅ | Inbox list, filtering, unread badges, snippet previews |
| 05 | Message Timeline | ✅ | Bottom-anchored timeline, delivery ticks, image previews, paging |
| 06 | Composer / Attachments / Voice / Drafts | 🟡 | Multi-line composer, file attachments; voice notes not built |
| 07 | Calls / Realtime Media | ⚪ | Stub tab; waiting for backend realtime media pipeline |
| 08 | Contacts / Requests / Verification / Identity | ✅ | Contact directory, safety fingerprints, key change warnings |
| 09 | Groups / Membership / Roles | ✅ | MLS group sessions, one-step invites, roster discovery sync |
| 10 | Files / Media Gallery / Transfer | 🟡 | Inline attachment previews; dedicated gallery unbuilt |
| 11 | Search / Local Knowledge Retrieval | ⚪ | Joint gap with Part 32 |
| 12 | Nearby / QR / NFC Pairing / Device Linking | ✅ | Security → Devices pairing views, one-time codes, approval queue |
| 13 | Notifications / Background / Incoming Call | 🟡 | Daemon desktop notifications (`notify-rust`), background gating |
| 14 | Presence / Typing / Receipts / Status | 🟡 | Delivery receipts (ticks); typing & ephemeral status unbuilt |
| **15** | **Security Center / Devices / Keys / Recovery** | ✅ | Fingerprint reveal, diagnostics card, event banners, device lists |
| 16 | Backup / Restore / Export / Migration | 🟡 | CLI/daemon backup complete (`siar-backup`); GUI wizard unbuilt |
| 17 | Emergency SOS / Offline Mesh | ⚪ | Backend emergency crates exist, UI unbuilt |
| 18 | Settings / Privacy / Notifications / Data Controls | 🟡 | Settings view (persisted theme, density, notifications) |
| 19 | Plugin/Module Ecosystem | ⚪ | Joint gap with Part 24 |
| 20 | Diagnostics / Network Paths / Advanced Dev | 🟡 | Diagnostics card in Security view; visual graph unbuilt |
| 21 | Accessibility | 🟡 | Full keyboard navigation, focus management, high-contrast theme |
| 22 | Design System (tokens/typography/icons/motion) | ✅ | Implemented as `Theme` global in `crates/siar-ui-slint` |
| 23 | Responsive/Adaptive Layout | ✅ | Adaptive layout switching between single-pane and split views |
| 24 | Error/Loading/Empty/Offline/Degraded States | 🟡 | Network offline banners, empty list placeholders, error banners |
| 25 | Onboarding / First-Run / Permissions | 🟡 | Unprovisioned / first-run setup flow |
| 26 | Performance / Virtualization / Large-Data UI | 🟡 | Bounded 50-item timeline pages, lazy attachment loading |
| 27 | UI Testing / Screenshot / Quality Gates | ✅ | 29 interaction + 10 headless render tests, live Wayland verification |

---

## Implementing Crate(s)

- `crates/siar-ui-slint` — Slint markup, components, theme tokens, styling, interaction handlers.
- `apps/desktop` — Slint window runtime, daemon auto-spawn, IPC connection management.
- `siar-ui-sync` — domain snapshot + delta synchronization protocol connecting UI to daemon.
- `siar-ui-state` — view models and state reducers.
