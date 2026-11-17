# Mobile-Driven UI/UX System & Architecture

## Dioxus + Rust Mobile Core + Android Kotlin Platform Bridge

> **Scope Note:** Low-level Android MediaCodec hardware surfaces, audio DSP pipelines, Android native build packaging (`xtask`), and backend cryptographic protocols are specified in:
> - [`sys-arch/25`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md) — Android Direct Hardware Surface Zero-Copy Media Architecture
> - [`sys-arch/26`](file:///home/irshad/Projects/siar/sys-arch/26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md) — Rust-First Audio DSP Architecture
> - [`sys-arch/27`](file:///home/irshad/Projects/siar/sys-arch/27-rust-driven-android-native-build-packaging-automation.md) — Rust-Driven Android Native Build & Packaging Automation
> - [`sys-arch/13`](file:///home/irshad/Projects/siar/sys-arch/13-battery-aware-scheduling-architecture.md) — Battery-Aware Scheduling Architecture
> - [`sys-arch/14`](file:///home/irshad/Projects/siar/sys-arch/14-proximity-abstraction-architecture.md) — Proximity Abstraction (BLE / Wi-Fi Direct / Wi-Fi Aware)
> - [`sys-arch/31`](file:///home/irshad/Projects/siar/sys-arch/31-notifications-push-background-delivery-lifecycle-architecture.md) — Push & Background Delivery Lifecycle
>
> This document defines the **mobile-first UI/UX architecture, Dioxus component hierarchy, touch interactions, Android Kotlin bridge boundaries, iOS strategy, adaptive layouts (foldables/tablets), and mobile performance budgets**.

---

## 1. Product Direction & Mobile UX Principles

The mobile client is designed to feel like a modern, ultra-responsive messenger under ordinary conditions, while gracefully surfacing network topology, battery state, and emergency controls when conditions degrade:

```text
Simple & Polished for Daily Messaging
                  │
                  ▼ (When connectivity degrades)
Informative on Active Transport (Internet, LAN, Wi-Fi Direct, BLE)
                  │
                  ▼ (When completely offline)
Transparent on Mesh & Store-Carry-Forward Delivery (DTN)
                  │
                  ▼ (During emergencies)
Decisive, Preemptive & Trustworthy (One-Tap SOS & Local Disaster Board)
```

### Core Mobile UX Rules
1. **Touch-First & One-Handed Friendly:** All primary interaction targets (composer, tabs, emergency triggers) are situated in the lower "natural thumb zone." Minimum touch target size is strictly **48×48 dp**.
2. **Interruption-Safe:** All draft messages, in-flight inputs, and navigation stacks survive process death, incoming phone calls, and background suspension.
3. **Honest Delivery States:** Explicitly differentiates between *Sent to Peer*, *Carried by Mesh Relays*, *Reached Gateway*, and *Delivered to Destination Device*.

---

## 2. Platform Architecture: One Product, Two Platform Families

To prevent logic duplication across Android and iOS, all UI state, navigation, viewmodels, design tokens, and business logic remain strictly in **Rust (Dioxus Mobile)**. Platform-specific languages (Kotlin on Android, Swift/Obj-C on iOS) own only thin platform integrations:

```text
                         Shared Rust Mobile Core (Dioxus)
                         - Navigation & Screen State
                         - ViewModels & Presentation Logic
                         - Timeline Virtualization & Bubbles
                         - Design Tokens & Animation Policy
                                       │
                    ┌──────────────────┴──────────────────┐
                    ▼                                     ▼
            Android Platform Adapter             iOS Platform Adapter
            (Thin Kotlin Bridge Layer)          (Rust-First / Narrow FFI)
                    │                                     │
            Android OS APIs                       Apple iOS APIs
```

### Android Kotlin Ownership Rules
- **What Kotlin Owns:** Activity lifecycle, Android runtime permissions, notification channels, foreground service lifecycles, SAF (Storage Access Framework) file picker, Photo Picker, CameraX preview binding, system Picture-in-Picture (PiP).
- **What Kotlin MUST NOT Contain:** Chat business logic, E2EE keys, message ordering, routing decisions, DTN replication rules, or SQLite table operations.

---

## 3. Shared Rust Platform Contract & Typed Event Bridge

### Shared `MobilePlatform` Trait
```rust
#[async_trait]
pub trait MobilePlatform: Send + Sync {
    async fn request_permission(&self, perm: MobilePermission) -> Result<PermissionState, PlatformError>;
    async fn pick_file(&self, req: FilePickRequest) -> Result<Option<PickedFile>, PlatformError>;
    async fn pick_media(&self, req: MediaPickRequest) -> Result<Option<PickedMedia>, PlatformError>;
    async fn scan_qr_code(&self) -> Result<QrScanResult, PlatformError>;
    async fn trigger_haptic(&self, feedback: HapticFeedbackType);
    async fn set_keep_screen_on(&self, enabled: bool) -> Result<(), PlatformError>;
}
```

### Typed `PlatformEvent` Stream
Instead of raw unstructured JNI callbacks, Kotlin emits structured, type-safe events into the Rust engine:
```rust
pub enum PlatformEvent {
    AppForegrounded,
    AppBackgrounded,
    NetworkStateChanged(NetworkSnapshot),
    PermissionChanged { permission: MobilePermission, state: PermissionState },
    ThermalStateChanged(ThermalPressureLevel),
    BatteryLevelChanged { percent: u8, is_charging: bool },
    IncomingSharedContent(SharedPayload),
    SystemBackPressed,
}
```

---

## 4. Mobile Navigation Shell & Deep Links

```text
┌──────────────────────────────────────────────────┐
│ [≡] Bob Rahman (Verified ✓)      [ 📞 ] [ 🎥 ]   │ ‹ Contextual Top Bar (56dp)
├──────────────────────────────────────────────────┤
│                                                  │
│          Active Screen / Virtualized             │
│              Conversation Timeline               │
│                                                  │
├──────────────────────────────────────────────────┤
│ [ + ] Type a message...                 [ 🎤 ]   │ ‹ Rich Mobile Composer (56dp)
├──────────────────────────────────────────────────┤
│  💬 Chats    📞 Calls    🌐 Nearby    🚨 SOS     │ ‹ Bottom Navigation Bar (64dp)
└──────────────────────────────────────────────────┘
```

### Back Navigation Determinism
1. Dismiss transient overlays, call dropdowns, or tooltips.
2. Close open bottom sheets (attachment picker, delivery detail sheet).
3. Close full-screen media/photo viewer.
4. Pop conversation timeline back to Chat List.
5. Exit application / delegate to system launcher.

---

## 5. Adaptive Layouts, Foldables & Safe Areas

### Layout Breakpoints
- **Compact (Width < 600dp):** Standard phone portrait. Single-screen navigation stack.
- **Medium (600dp ≤ Width < 840dp):** Large foldables / small tablets. Split list-detail navigation.
- **Expanded (Width ≥ 840dp):** Tablets / unfolded large foldables. Persistent 3-pane desktop-style layout.

### Foldable & Hinge Continuity
- **Hinge Exclusion:** Detects display fold posture and avoids placing buttons or text across the physical hinge crease.
- **State Continuity:** Draft text, scroll position, active call video streams, and audio playback seamlessly transition without UI reconstruction when the device is folded or unfolded.

### WindowInsets & Safe Area Insets
All views calculate dynamic padding from system insets (`status_bar_height`, `navigation_bar_height`, `ime_keyboard_height`, `display_cutout`).

---

## 6. Mobile Screens & Micro-Interactions

### A. Conversation Timeline & Delivery Detail Sheet
- Long-pressing any message opens the **Delivery Detail Bottom Sheet**:
  ```text
  ┌────────────────────────────────────────────────┐
  │ 📄 Message Delivery Details                    │
  ├────────────────────────────────────────────────┤
  │ Status: Carried by Nearby Mesh Relays (2 hops) │
  │ Message ID: 4f8a...c921                        │
  │ Created: Today, 14:22:05                       │
  │ Current Path: BLE Proximity Mesh               │
  │ Security: Signal Double Ratchet (E2EE ✓)       │
  ├────────────────────────────────────────────────┤
  │ [ Close ]                                      │
  └────────────────────────────────────────────────┘
  ```

### B. Mobile Call UI & Picture-in-Picture (PiP)
- Incoming call banner with instant Accept/Decline action buttons.
- Active call screen displays local/remote zero-copy video surfaces (see [`sys-arch/25`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md)).
- Navigating away from an active call automatically transitions into Android system Picture-in-Picture (PiP) window.

### C. Mobile Emergency SOS UX
- Single-tap high-priority emergency mode with prominent visual confirmation to prevent accidental triggers.
- Instant action triggers: `Send SOS`, `I'm Safe`, `Need Medical Help`, `Need Water/Food`.
- Configurable location privacy: Exact GPS, Fuzzed (500m radius), or None.

---

## 7. Mobile Performance Budgets & Memory Discipline

- **Frame Budget:** 60fps (16.6ms) / 120fps (8.3ms) during rapid scroll events.
- **Cold Start Time:** Initial interactive UI render within **< 400ms** on mid-range Android hardware.
- **Low-Memory Trim:** On `ComponentCallbacks2.ON_TRIM_MEMORY_RUNNING_CRITICAL`, the UI immediately flushes in-memory image caches and unloads off-screen thumbnail textures.
- **OLED Dark Theme:** True black (`#000000`) theme optimization for OLED battery conservation during emergency and off-grid operations.
