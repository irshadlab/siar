# 12 — Cross-Platform Client Architecture

> **Corresponding Specifications:** [`sys-arch/ui-ux-01-product-foundation-cross-platform-interaction-architecture.md`](../sys-arch/ui-ux-01-product-foundation-cross-platform-interaction-architecture.md), [`sys-arch/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md`](../sys-arch/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md), [`sys-arch/ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md`](../sys-arch/ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md)  
> **Key Modules:** [`crates/siar-ui-state`](../crates/siar-ui-state), [`apps/desktop`](../apps/desktop), [`apps/android`](../apps/android)

---

## 1. Unified Core & Native Shell Strategy

SIAR uses a **Single Core, Native Presentation** architectural model:

```
+-------------------------------------------------------------------------------+
|             Desktop UI (Dioxus 0.7)            Android UI (Jetpack Compose)   |
|            Rust-Native Cross-Platform                Kotlin Material 3        |
+------------------------------------+------------------------------------------+
                                     | (JNI Bridge / Direct In-Process Memory)
+------------------------------------v------------------------------------------+
|                  SIAR Reactive UI State Engine (siar-ui-state)                 |
|       - Unidirectional Data Flow (UDF)      - Optimistic UI Mutators          |
|       - Caching & View State Slices         - Subscription Event Bus          |
+------------------------------------+------------------------------------------+
                                     |
+------------------------------------v------------------------------------------+
|                       Underlying Rust Workspace Services                      |
| (siar-storage, siar-messaging, siar-routing-policy, siar-crypto, siar-calls)  |
+-------------------------------------------------------------------------------+
```

### Core Invariants
1. **Zero Business Logic in UI**: All encryption, routing, validation, database operations, and state machine transitions execute strictly inside the Rust workspace.
2. **Deterministic UI State**: The UI is a pure projection of the reactive state models exposed by [`siar-ui-state`](../crates/siar-ui-state).
3. **Instant Responsiveness**: Optimistic updates render message bubbles and UI interactions in $< 16\text{ms}$ (60 fps), updating delivery ticks asynchronously as network events arrive.

---

## 2. Desktop Shell: Dioxus 0.7 Architecture

The desktop application ([`apps/desktop`](../apps/desktop)) is written entirely in modern Rust using **Dioxus 0.7**:

```rust
#[component]
pub fn AppShell() -> Element {
    let active_tab = use_signal(|| NavigationTab::Inbox);
    let system_health = use_signal(|| MeshHealthStatus::Healthy);

    rsx! {
        div { class: "app-container dark-theme",
            SidebarNavigation { active_tab }
            MainContentSplitView { active_tab }
            StatusBarMeshTelemetry { system_health }
        }
    }
}
```

### Desktop Capabilities
- **Multi-Pane Responsive Layout**: Collapsible sidebar, virtualized conversation stream, detail inspection sidebar.
- **System Tray & Hotkeys**: Native window minimizing to notification tray with global emergency shortcut keys.
- **Hardware Acceleration**: GPU-accelerated rendering with low idle RAM footprint ($< 60\text{ MB}$).

---

## 3. Android Shell: Jetpack Compose & JNI Glue

The Android application ([`apps/android`](../apps/android)) combines modern Jetpack Compose Kotlin UI with native Rust performance:

```
[Kotlin Jetpack Compose ViewModels]
                 |
                 v
[SiarNativeBridge (JNI Interface)]
                 |
                 v
[crates/siar-ui-state & Rust Core Engine]
```

### Android Lifecycle & Service Management
- **`MeshForegroundService`**: Maintains persistent low-power BLE advertising/scanning and Wi-Fi Direct listening when the screen is locked.
- **WakeLock Management**: Partitions wakeups into short, bounded micro-windows ($< 250\text{ms}$) to preserve battery longevity while maintaining mesh routing presence.
- **Material You Dynamic Theming**: Adaptive color palette conforming to system dark mode and user accessibility settings.
