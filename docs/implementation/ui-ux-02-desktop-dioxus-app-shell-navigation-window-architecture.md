# UI/UX Part 02 — Desktop App Shell, Navigation & Window Architecture (Slint)

Source spec: `sys-arch/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md` — 227 numbered sections.

## Toolkit Migration: Dioxus → Slint (2026-10-22)

The desktop shell was migrated from Dioxus (WebView) to native **Slint** in `crates/siar-ui-slint`.
`apps/desktop` is a thin binary connecting the Slint UI shell to `siar-daemon` over local IPC sockets via `siar-ui-sync`. The `ui-ux-NN` specs define the visual design, state machines, tokens, and interactions; Slint provides native GPU-accelerated rendering and lower resource usage.

## Implementation status

**🟡 Substantially complete across app shell, messaging, contacts, groups, security center, and device linking.**
All 29 interaction tests and 10 headless render tests pass cleanly. Live verified on physical hardware running Linux / Wayland (Niri compositor).

## Implementing crate(s) & applications

- `crates/siar-ui-slint` — Slint UI markup, component library, theme tokens, and interaction logic.
- `apps/desktop` — desktop shell launcher, window management, auto-spawning daemon helper, and IPC connection bridge.
- `siar-ui-sync` — domain snapshot + delta synchronization connecting the Slint UI to `siar-daemon`.
- `siar-ui-state` — view models and UI state reducers.

---

## Completed Architecture & Features

### 1. App Shell & Navigation (`ui-ux-02`)
- **Navigation Rail**: Left rail with quick navigation to Chats, Contacts, Groups, Security Center, and Settings.
- **Adaptive Split Layout**: Responsive multi-pane layout adapting between compact single-pane and expanded master-detail views.
- **Status Bar**: Live network indicators ("Online" / "Offline"), connection tickets, and synchronization indicators.
- **Accessibility & Focus**: Full keyboard navigation (Tab stops, arrow navigation for segmented controls, auto-focus on Paste).

### 2. Design System Tokens (`ui-ux-22`)
- Semantic tokens declared in `Theme` global: primary, surface, accent, warning, danger, text colors, spacing, radius, and elevation.
- Supported modes: Light, Dark, and High-Contrast themes.

### 3. Chats & Messaging UI (`ui-ux-04`, `ui-ux-05`, `ui-ux-06`)
- **Conversation List**: Filterable inbox list with unread badges and snippet previews.
- **Timeline**: Bottom-anchored message stream, delivery tick marks (sending, carried, delivered), and image attachment previews.
- **History Paging**: Survives process restarts via `MessageService::open_stored_payload` and strictly increasing per-device sequence numbers (`next_sequence`). Paged history with scroll anchoring.
- **Read State**: Unread badges survive restarts via persisted incoming sequence marks (`read-marks.txt`).
- **Composer**: Text entry with multi-line support (Enter / Shift+Enter) and attachment pickers.

### 4. Contacts & Safety Verification (`ui-ux-08`, `ui-ux-15`)
- Filterable contact list with verification indicators.
- Safety numbers computed from `siar-identity-multidevice`'s `SafetyFingerprint`.
- Warning banners on contact key changes (`KeyChanged` / `IdentityChanged` events).
- Top banner with "Review" and "Dismiss" local actions.
- Own identity fingerprint display (short/full with reveal-on-demand) and diagnostics card.

### 5. Groups & Invites (`ui-ux-09`)
- Persistent MLS group sessions (SQLite storage with `JsonCodec`).
- One-step group invite acceptance: `WelcomeBundle` carries MLS welcome and group roster; invite cards enable single-tap acceptance.
- Group discovery and roster synchronization: `CommitBundle` and `DirectoryEntry` distributed to members; peer addresses persisted in stoolap `peer_directory`.

### 6. Device Linking UI (`ui-ux-12`, `ui-ux-15`)
- Dedicated views in Security → Devices:
  - "Add a device" showing one-time 80-bit code and transport ticket.
  - "Devices waiting for approval" with key fingerprint confirmation.
  - "Join an existing account" input fields.
  - Global warning banner reminding user to restart after joining an account.

### 7. Process Separation & Daemon Sync (`siar-ui-sync`)
- Slint window runs as an unprivileged client of `siar-daemon`.
- If daemon is not running, the desktop app automatically starts `siar-daemon` in the background.
- State is synchronized through domain wire sections; window local state remains strictly in UI process memory.

---

## Known Gaps / Open Items

- **Realtime Media Calls (`ui-ux-07`)**: Calls UI tab is a stub pending backend audio/video pipeline.
- **File Transfer Browser (`ui-ux-10`)**: Standalone media gallery view (currently inline image previews only).
- **Search UI (`ui-ux-11`)**: Global full-text search interface.
- **In-App Restart**: Ability for desktop window to trigger graceful restart of background daemon.
