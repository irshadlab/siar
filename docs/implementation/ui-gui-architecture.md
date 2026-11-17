# Universal & Desktop Dioxus UI/GUI Architecture

> **Scope Note:** Backend networking, E2EE cryptography, SQL storage engines, audio DSP, video call protocols, and headless daemon runtimes are specified in [`sys-arch/`](file:///home/irshad/Projects/siar/sys-arch) (Parts 01–33). Mobile-specific Android/iOS UX and Kotlin bridges are specified in [`mobile-driven-ui-ux-architecture.md`](file:///home/irshad/Projects/siar/mobile-driven-ui-ux-architecture.md). This document defines the **universal UI design system, desktop application shell, presentation architecture, viewmodels, and Dioxus component hierarchy**.

---

## 1. UI Architectural Principles & Strict Separation

The Dioxus UI is strictly a presentation and interaction layer. It never owns the source of truth, never executes raw cryptographic operations, and never directly manipulates network sockets or database tables.

```text
┌────────────────────────────────────────────────────────────────────────┐
│                          Dioxus UI Components                          │
│     (AppShell, ChatList, MessageTimeline, Composer, CallOverlay)       │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │ Consumes Reactive Signals / Emits
                                   ▼
┌────────────────────────────────────────────────────────────────────────┐
│                        Presentation ViewModels                         │
│   (ConversationViewModel, MessageViewModel, NetworkStatusViewModel)    │
└──────────────────┬───────────────────────────────────▲─────────────────┘
                   │ Emits UiCommand                   │ Receives UiEvent
                   ▼                                   │
┌──────────────────────────────────────────────────────┴─────────────────┐
│                     Application Command & Event Bus                    │
└──────────────────┬───────────────────────────────────▲─────────────────┘
                   │                                   │
                   ▼                                   │
┌────────────────────────────────────────────────────────────────────────┐
│                    SIAR Core Engine (sys-arch/01-33)                   │
│   (Identity, Storage, Routing, Transports, Blobs, Crypto, Calls)       │
└────────────────────────────────────────────────────────────────────────┘
```

### Prohibited UI Anti-Patterns
- **No Direct Socket / Iroh Calls:** UI components never interact with `iroh::Endpoint` or network links.
- **No Direct Database Writes:** UI components never execute raw SQL against `siar-storage`.
- **No Private Key Access:** UI components never hold raw private keys or cryptographic seeds.
- **No Unbounded Rendering:** UI components never render entire unpaginated message histories into the DOM.

---

## 2. Desktop 3-Pane Application Shell

Desktop clients (Linux, Windows, macOS) provide a responsive 3-pane layout designed for high information density, rapid navigation, and keyboard efficiency.

```text
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│ [≡] SIAR Messenger    ● Internet (Direct QUIC)   [ 🔍 Search Ctrl+K ]         [_] [□] [✕]│
├───────────────┬──────────────────────────────────────────┬──────────────────────────────┤
│ Navigation    │ Conversations                            │ Active Conversation Timeline │
│ Rail (64px)   │ (320px)                                  │ (Remaining Viewport Width)   │
├───────────────┼──────────────────────────────────────────┼──────────────────────────────┤
│ 💬 Chats (3)  │ [ Search in chats...     ]               │ ‹ Bob Rahman (Verified ✓)    │
│ 📞 Calls      │ ──────────────────────────────────────── │ ──────────────────────────── │
│ 📁 Files      │ 📌 Alice Cooper                  12:45   │ [12:40] Bob Rahman           │
│ 🌐 Nearby     │    "Sent attachment..."         ✓✓       │ Hey Alice, are you reachable │
│ 🚨 Emergency  │                                          │ on the local mesh?           │
│ ⚙️ Settings    │ 💬 Bob Rahman                    12:40   │                              │
│               │    "Hey Alice, are you..."       [2]     │ [12:45] Alice (You)          │
│               │                                          │ Yes! Direct Wi-Fi link active│
│               │ 🚨 Emergency Mesh Broadcast      11:20   │                             ✓│
│               │    "Bridge Flooded - Sector 4"  [SOS]    │ ──────────────────────────── │
│               │                                          │ [ + ] Type a message... [ ➤ ]│
└───────────────┴──────────────────────────────────────────┴──────────────────────────────┘
```

### Desktop Window & Tray Architecture
1. **Multi-Window Support:** Separate windows for detached active video calls, network diagnostic path visualizers (see [`sys-arch/18`](file:///home/irshad/Projects/siar/sys-arch/18-network-diagnostics-path-visualization-architecture.md)), and detached attachment previews.
2. **System Tray Integration:** Minimize-to-tray capability with unread badge counter and instant background wake on incoming audio/video calls.
3. **Global Keyboard Navigation:**
   - `Ctrl+K` / `Cmd+K`: Global omnibox search (conversations, contacts, emergency alerts).
   - `Ctrl+F` / `Cmd+F`: In-conversation timeline search (see [`sys-arch/32`](file:///home/irshad/Projects/siar/sys-arch/32-search-indexing-local-knowledge-privacy-architecture.md)).
   - `Ctrl+1` through `Ctrl+5`: Instant navigation rail switching.
   - `Escape`: Dismiss modals, bottom sheets, or close active search bar.

---

## 3. UI Command & Event Contract

The UI communicates with the application core through strictly typed commands and events:

### UI Commands (Downstream)
```rust
pub enum UiCommand {
    SendMessage {
        conversation_id: ConversationId,
        content: MessageContentDraft,
    },
    SendEmergencySos {
        emergency_type: EmergencyType,
        note: Option<String>,
        share_location: bool,
    },
    StartCall {
        peer_id: AccountId,
        video: bool,
    },
    AcceptCall {
        call_id: CallId,
    },
    RejectCall {
        call_id: CallId,
    },
    PairDeviceByQr {
        qr_data: String,
    },
    SetNetworkPreference {
        metered_allowed: bool,
    },
}
```

### UI Events (Upstream)
```rust
pub enum UiEvent {
    ConversationUpdated(ConversationSummaryView),
    MessageAppended(MessageItemView),
    DeliveryStatusChanged {
        message_id: MessageId,
        status: DeliveryStatusView,
    },
    NetworkTopologyChanged(NetworkTopologySummaryView),
    IncomingCallReceived(IncomingCallPromptView),
    EmergencyAlertReceived(EmergencyAlertCardView),
    StorageQuotaWarning {
        used_bytes: u64,
        max_bytes: u64,
    },
}
```

---

## 4. Virtualized Timeline & Message Bubble Design

To guarantee smooth 60fps / 120fps scrolling on desktop and low-end hardware, the message timeline implements virtual windowing:
- **Windowed Rendering:** Only elements within the visible viewport plus an overscan buffer (20 items above/below) are mounted into the Dioxus DOM.
- **Scroll Anchor Preservation:** When history is fetched asynchronously (loading older chunks from [`sys-arch/04`](file:///home/irshad/Projects/siar/sys-arch/04-offline-event-log-architecture.md)), the scroll position is pinned to the current visible item to prevent jumping.

### Message Delivery State Badges

```text
┌───────────────────────────────────────────────┐
│ Normal Sent Message                           │
│ "Let's meet at the shelter at 3 PM."    14:22 │
│                                            ✓✓ │ (Delivered to Device)
└───────────────────────────────────────────────┘

┌───────────────────────────────────────────────┐
│ Mesh-Carried Delay-Tolerant Message           │
│ "Water supply available at North Ridge."14:25 │
│                              ⇄ Carried (2 hop)│ (In DTN Transit, sys-arch/06)
└───────────────────────────────────────────────┘

┌───────────────────────────────────────────────┐
│ High-Priority Emergency Broadcast             │
│ [ 🚨 SOS: MEDICAL ASSISTANCE NEEDED ]   14:28 │
│ Sector 7 | Battery: 15%                       │
│                           ⚡ Preempted (P0)   │ (Emergency Preemption, sys-arch/17)
└───────────────────────────────────────────────┘
```

---

## 5. Design System Tokens

All UI styling is strictly governed by semantic design tokens:

### Color Palette (Tailored HSL Tokens)
- `--color-bg-base`: `hsl(220, 20%, 10%)` (Dark mode surface foundation)
- `--color-bg-elevated`: `hsl(220, 18%, 15%)` (Card & panel surface)
- `--color-text-primary`: `hsl(220, 15%, 95%)` (High-contrast primary typography)
- `--color-text-muted`: `hsl(220, 10%, 65%)` (Secondary timestamps & metadata)
- `--color-accent`: `hsl(210, 100%, 56%)` (Primary action blue)
- `--color-success`: `hsl(145, 65%, 42%)` (Delivered / verified green)
- `--color-emergency`: `hsl(0, 85%, 55%)` (Emergency SOS crimson)
- `--color-mesh`: `hsl(280, 65%, 60%)` (Proximity mesh purple)

### Typography Scale
- **Display:** 24px / 32px line-height (Semibold)
- **Title:** 18px / 24px line-height (Medium)
- **Body Large:** 15px / 22px line-height (Regular)
- **Body Small:** 12px / 16px line-height (Regular, for timestamps and delivery badges)
- **Code / Monospace:** 13px / 18px line-height (Key fingerprints, addresses, hex hashes)

---

## 6. Accessibility & Internationalization

1. **Screen Reader Semantics:** Every interactive button, avatar, delivery badge, and input field includes ARIA labels (`aria-label`, `aria-live` for new messages, `aria-expanded` for detail panels).
2. **Keyboard Focus Discipline:** Highly visible focus indicators (`--color-focus-ring: hsl(210, 100%, 65%)`) on all keyboard-navigable elements.
3. **RTL Support:** Bidirectional text rendering with automatic layout mirroring for right-to-left languages (Arabic, Hebrew, Persian).
4. **Theme Adaptation:** Seamless switching between Dark, Light, and High-Contrast Accessibility themes.

---

## 7. UI Verification & Performance Budgets

- **Frame Budget:** Timeline rendering and typing updates must execute within **16.6ms** (60fps baseline).
- **Cold Start UI Budget:** Initial AppShell render within **< 200ms** on desktop.
- **Automated Golden Testing:** Headless Dioxus component tests verify DOM structure, event dispatching, and visual snapshot regressions without requiring active Iroh or network sockets.
