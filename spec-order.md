# SIAR: Master Specification Execution Order & Dependency Matrix

> **Document Purpose**: Authoritative, dependency-ordered execution roadmap for all **60 SIAR system specifications** (33 Core Architecture Specifications + 27 UI/UX Specifications, totaling 12,893 numbered sections).  
> **Source Directory**: [`sys-arch/`](sys-arch/)  
> **Complements**: [`ROADMAP.md`](ROADMAP.md) (priority tracker) and [`SYS_ARCH_IMPLEMENTATION_STATUS_AND_REMAINING_SECTIONS.md`](SYS_ARCH_IMPLEMENTATION_STATUS_AND_REMAINING_SECTIONS.md).

---

## 1. Executive Summary & Ordering Principles

SIAR cannot be written in strictly numerical order (`01 → 33` followed by `ui-ux-01 → 27`). Doing so breaks fundamental architectural dependencies:
1. **Inverted Security Dependency**: Spec **28** (*E2EE Key Management & Privacy*) encrypts data carried by transports and storage; writing strictly `01 → 33` would delay cryptographic security until after building FFI (`19`), embedded Linux (`20`), and plugin sandboxes (`22`, `24`).
2. **Inverted UI Dependency**: Spec **`ui-ux-22`** (*Design System Tokens & Typography*) defines the styling tokens that every view depends on; writing `ui-ux-01 → 27` sequentially would build complex views like the timeline (`05`) and calls (`07`) before styling tokens even exist.
3. **Joint Vertical Slices**: Complex subsystems (e.g., QR pairing, notifications, search, backup, calls) require that the **backend core spec and frontend UI spec be implemented together** to ensure end-to-end functionality.

### The 7-Milestone Flowchart

```text
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 1: TIER 0 — HEADLESS CORE ENGINE (Specs 01 ➔ 09)                       │
│ Sequential: Envelopes ➔ Identity ➔ Routing ➔ Outbox ➔ Blobs ➔ DTN ➔ Recovery    │
│ Status: Spec 01 Done (100%), Spec 02 Active (88% §170–§179 done), Specs 03–09 Next│
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 2: TIER 1 — SECURITY & KEY MANAGEMENT BACKBONE (Spec 28)              │
│ Double Ratchet, forward secrecy, MLS group crypto, key revocation envelopes      │
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 3: TIER 2 — UI DESIGN FOUNDATION & APP SHELLS                          │
│ Written Together: Tokens (ui-ux-22) + Responsive Layout (23) + App Shells (02/03)│
│ + First-Run Wizard (25) + Security Center (ui-ux-15 finish §195–§221)            │
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 4: TIER 2 — CORE CHAT & SOCIAL GRAPH UI                                │
│ Written Together: Inbox (04) + Timeline (05) + Composer (06) + Contacts (08)    │
│ + Groups (09) + File Transfers (10) + Degraded States (24) + Virtualization (26) │
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 5: TIER 2 — JOINT FEATURE SLICES (Core Backend + UI Together)          │
│ • QR/NFC Pairing (15 + ui-ux-12)      • Realtime Calls (29/25/26 + ui-ux-07)     │
│ • Presence & Receipts (30 + ui-ux-14) • Local Search & FTS (32 + ui-ux-11)       │
│ • Push Notifications (31 + ui-ux-13)  • Backup & Migration (33 + ui-ux-16)       │
│ • Emergency SOS (17 + ui-ux-17)       • Network Diagnostics (18 + ui-ux-20)      │
│ • Settings & Data Controls (ui-ux-18)                                            │
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 6: TIER 3 — TRANSPORTS, DAEMONS & PLATFORM INFRASTRUCTURE             │
│ Relays (11) + Multipath (12) + Battery (13) + Proximity BLE/Wi-Fi Direct (14)   │
│ + Headless Daemon (16) + Embedded Linux (20) + C-ABI (19) + Android Build (27)  │
└──────────────────────────────────────┬───────────────────────────────────────────┘
                                       ▼
┌──────────────────────────────────────────────────────────────────────────────────┐
│ MILESTONE 7: TIER 4 — EXTENSIBILITY, ECOSYSTEM & QUALITY GATES (Final)           │
│ Plugin Host (24 + ui-ux-19) + WASM (22) + Extensions (21) + Interoperability (23)│
│ + Accessibility (ui-ux-21) + Fuzzing Test Suite (10) + Release Quality Gates (27)│
└──────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Milestone 1: Headless Core Engine (Tier 0 — Sequential 01 → 09)

> **Execution Strategy**: Implemented **strictly sequentially**. These 9 foundational crates form the underlying communications and persistence engine. Each spec directly relies on the prior spec's types and contracts.

| Step | Spec Document | Sections | Coverage / Status | Primary Crates | Core Focus |
|:---:|:---|:---:|:---:|:---|:---|
| **1.1** | [`01-protocol-extension-system-architecture.md`](sys-arch/01-protocol-extension-system-architecture.md) | **108** | **108/108 (100%)** | [`siar-protocol-ext`](crates/siar-protocol-ext), [`siar-protocol`](crates/siar-protocol) | Envelope framing, version negotiation, capability headers |
| **1.2** | [`02-multi-device-identity-architecture.md`](sys-arch/02-multi-device-identity-architecture.md) | **204** | **204/204 (100%)** | [`siar-identity-multidevice`](crates/siar-identity-multidevice), [`siar-crypto`](crates/siar-crypto) | Ed25519 root key, device certs, revocation, algorithm agility (§190–§204 complete) |
| **1.3** | [`03-transport-routing-policy-engine-architecture.md`](sys-arch/03-transport-routing-policy-engine-architecture.md) | **200** | **~198/200 (99%)** | [`siar-routing-policy`](crates/siar-routing-policy), [`siar-connectivity`](crates/siar-connectivity) | **ACTIVE FOCUS**: Multi-Device Aggregation, Broadcast, & DTN Storage. Next: §183+ |
| **1.4** | [`04-offline-event-log-architecture.md`](sys-arch/04-offline-event-log-architecture.md) | **95** | 10/95 (11%) | [`siar-event-log`](crates/siar-event-log), [`siar-storage`](crates/siar-storage) | Crash-resilient transactional outbox, append-only log |
| **1.5** | [`05-robust-file-blob-subsystem-architecture.md`](sys-arch/05-robust-file-blob-subsystem-architecture.md) | **210** | 23/210 (11%) | [`siar-blob-manifest`](crates/siar-blob-manifest), [`siar-storage`](crates/siar-storage) | BLAKE3 chunking, deduplication, resumable blob transfers |
| **1.6** | [`06-dtn-store-carry-forward-architecture.md`](sys-arch/06-dtn-store-carry-forward-architecture.md) | **192** | 50/192 (26%) | [`siar-dtn-bundle`](crates/siar-dtn-bundle), [`siar-dtn`](crates/siar-dtn) | Delay-tolerant bundle store, epidemic forwarding, anti-entropy |
| **1.7** | [`07-capability-negotiation-architecture.md`](sys-arch/07-capability-negotiation-architecture.md) | **164** | 19/164 (12%) | [`siar-capability`](crates/siar-capability), [`siar-protocol`](crates/siar-protocol) | Dynamic link capability exchange, transport handoffs |
| **1.8** | [`08-resource-limits-backpressure-architecture.md`](sys-arch/08-resource-limits-backpressure-architecture.md) | **193** | 56/193 (29%) | [`siar-resource-limits`](crates/siar-resource-limits) | Token-bucket rate limiting, buffer caps, DoS defense |
| **1.9** | [`09-crash-recovery-architecture.md`](sys-arch/09-crash-recovery-architecture.md) | **186** | 15/186 (8%) | [`siar-crash-recovery`](crates/siar-crash-recovery), [`siar-storage`](crates/siar-storage) | WAL verification, panic isolation, corrupted state repair |

---

## 3. Milestone 2: Security & Key Management Backbone (Tier 1)

> **Execution Strategy**: Before expanding user interfaces or high-level network services, the End-to-End Encryption (E2EE) and key management invariants are closed. Transports and storage built in Milestone 1 operate in zero-knowledge mode over ciphertext payloads defined here.

| Spec Document | Sections | Coverage / Status | Primary Crates | Core Focus |
|:---|:---:|:---:|:---|:---|
| [`28-production-security-e2ee-key-management-privacy-architecture.md`](sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md) | **127** | 46/127 (36%) | [`siar-crypto`](crates/siar-crypto), [`siar-crypto-mls`](crates/siar-crypto-mls) | Double Ratchet session ratchets, MLS group key agreements, safety numbers, key storage abstractions, privacy boundaries |

---

## 4. Milestone 3: UI Design Foundation & App Shells (Written Together)

> **Execution Strategy**: Frontend specifications cannot be built in numerical sequence (`ui-ux-01` → `27`). This milestone establishes the visual foundation, cross-platform layouts, desktop/mobile app shells, and identity management UI together.

| Spec Document | Sections | Focus / Responsibility | Target Applications |
|:---|:---:|:---|:---|
| [`ui-ux-22-design-system-tokens-typography-icons-motion-architecture.md`](sys-arch/ui-ux-22-design-system-tokens-typography-icons-motion-architecture.md) | **253** | Design tokens, color palette, typography scales, spacing, icon sets, transition physics | Shared CSS / Dioxus tokens |
| [`ui-ux-01-product-foundation-cross-platform-interaction-architecture.md`](sys-arch/ui-ux-01-product-foundation-cross-platform-interaction-architecture.md) | **83** | Core interaction models, keyboard shortcuts, pointer/touch parity, accessibility foundations | Cross-platform |
| [`ui-ux-23-responsive-adaptive-desktop-tablet-foldable-phone-layout-architecture.md`](sys-arch/ui-ux-23-responsive-adaptive-desktop-tablet-foldable-phone-layout-architecture.md) | **223** | Breakpoint grid (compact, medium, expanded), adaptive sidebars, foldable hinge posture | Layout engines |
| [`ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md`](sys-arch/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md) | **227** | Desktop Dioxus application window, system tray, titlebar, multi-windowing, navigation rail | [`apps/desktop`](apps/desktop) |
| [`ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md`](sys-arch/ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md) | **217** | Android Compose / Dioxus mobile shell, edge-to-edge system bars, predictive back navigation | [`apps/android`](apps/android) |
| [`ui-ux-25-onboarding-first-run-permission-education-architecture.md`](sys-arch/ui-ux-25-onboarding-first-run-permission-education-architecture.md) | **209** | First-run setup, cryptographic account generation wizard, emergency identity backup reminder | Client apps |
| [`ui-ux-15-security-center-devices-keys-recovery-architecture.md`](sys-arch/ui-ux-15-security-center-devices-keys-recovery-architecture.md) | **221** | **184/221 (83%) Done** — Complete remaining sections §195–§221: key recovery, device revocation UX | [`crates/siar-ui-state`](crates/siar-ui-state) |

---

## 5. Milestone 4: Core Chat & Social Graph UI (Written Together)

> **Execution Strategy**: Implemented as a coherent frontend package on top of the Milestone 1 engine and Milestone 3 shells. Bridges `siar-ui-state` with the user interface.

| Spec Document | Sections | Focus / Responsibility | Target Crates / Apps |
|:---|:---:|:---|:---|
| [`ui-ux-04-conversation-list-inbox-architecture.md`](sys-arch/ui-ux-04-conversation-list-inbox-architecture.md) | **213** | Conversation inbox, unread counts, pinned chats, search filter headers, swipe actions | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-05-conversation-message-timeline-architecture.md`](sys-arch/ui-ux-05-conversation-message-timeline-architecture.md) | **254** | Bubble timeline, delivery status icons (sending, sent, delivered, read), reply threads | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-06-message-composer-attachments-voice-notes-drafts-architecture.md`](sys-arch/ui-ux-06-message-composer-attachments-voice-notes-drafts-architecture.md) | **309** | Text area, markdown formatting, voice note recording waveform, draft persistence | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-08-contacts-requests-verification-identity-architecture.md`](sys-arch/ui-ux-08-contacts-requests-verification-identity-architecture.md) | **227** | Contact directory, contact cards, Safety Number QR comparison, block/unblock lists | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-09-groups-membership-roles-architecture.md`](sys-arch/ui-ux-09-groups-membership-roles-architecture.md) | **224** | MLS group info, member list, permission roles (Admin, Member, Read-Only), join links | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-10-files-media-gallery-transfer-architecture.md`](sys-arch/ui-ux-10-files-media-gallery-transfer-architecture.md) | **161** | In-chat media viewer, transfer progress bars, auto-download controls, media grid | [`siar-ui-state`](crates/siar-ui-state) |
| [`ui-ux-24-error-loading-empty-offline-degraded-state-architecture.md`](sys-arch/ui-ux-24-error-loading-empty-offline-degraded-state-architecture.md) | **205** | Offline banners, store-and-forward queue badges, reconnect retry toasts, empty placeholders | Client apps |
| [`ui-ux-26-performance-virtualization-large-data-ui-architecture.md`](sys-arch/ui-ux-26-performance-virtualization-large-data-ui-architecture.md) | **276** | Virtual windowing for 100,000+ message timelines, image memory caching, 60fps scrolling | Client apps |

---

## 6. Milestone 5: Joint Feature Slices (Backend Core + UI Written Together)

> **Execution Strategy**: Specialized features implemented end-to-end. Each row represents a **vertical slice**: the backend core spec and its UI counterpart are coded and verified in the same development cycle.

| Feature Area | Backend Architecture Spec | Frontend UI / UX Architecture Spec | Combined Scope & Description |
|:---|:---|:---|:---|
| **1. QR & NFC Device Linking** | [`sys-arch/15`](sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md) *(176 §)* | [`sys-arch/ui-ux-12`](sys-arch/ui-ux-12-nearby-qr-nfc-pairing-device-linking-architecture.md) *(237 §)* | Out-of-band device pairing, ephemeral SAS handshake, animated QR codes, NFC tap exchange |
| **2. Ephemeral Presence & Typing** | [`sys-arch/30`](sys-arch/30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md) *(269 §)* | [`sys-arch/ui-ux-14`](sys-arch/ui-ux-14-presence-typing-receipts-status-architecture.md) *(197 §)* | Typing indicators, ephemeral TTL status, read receipts, privacy visibility toggle |
| **3. Push & Background Lifecycle** | [`sys-arch/31`](sys-arch/31-notifications-push-background-delivery-lifecycle-architecture.md) *(308 §)* | [`sys-arch/ui-ux-13`](sys-arch/ui-ux-13-notifications-background-incoming-call-architecture.md) *(215 §)* | UnifiedPush/FCM token manager, lockscreen banners, background wakeups, call heads-up UI |
| **4. Realtime Audio/Video Calling** | [`sys-arch/29`](sys-arch/29-realtime-calls-media-session-protocol-architecture.md) *(275 §)*<br>[`sys-arch/25`](sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md) *(213 §)*<br>[`sys-arch/26`](sys-arch/26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md) *(220 §)* | [`sys-arch/ui-ux-07`](sys-arch/ui-ux-07-calls-realtime-media-architecture.md) *(235 §)* | Full media subsystem: SRTP/QUIC session protocol, Android MediaCodec zero-copy surface, pure-Rust audio DSP (AEC/AGC/Opus), and in-call controls UI |
| **5. Local Search & Retrieval** | [`sys-arch/32`](sys-arch/32-search-indexing-local-knowledge-privacy-architecture.md) *(242 §)* | [`sys-arch/ui-ux-11`](sys-arch/ui-ux-11-search-local-knowledge-retrieval-architecture.md) *(148 §)* | Privacy-preserving local full-text search (Tantivy/FTS), keyword highlighting, search filter UI |
| **6. Encrypted Backup & Migration** | [`sys-arch/33`](sys-arch/33-backup-restore-export-import-archival-portability-architecture.md) *(280 §)* | [`sys-arch/ui-ux-16`](sys-arch/ui-ux-16-backup-restore-export-migration-architecture.md) *(223 §)* | Argon2id KDF passphrase encryption, streaming tarball import/export, cloudless device transfer wizard |
| **7. Emergency SOS & Mesh Mode** | [`sys-arch/17`](sys-arch/17-emergency-priority-classes-architecture.md) *(188 §)* | [`sys-arch/ui-ux-17`](sys-arch/ui-ux-17-emergency-sos-offline-mesh-architecture.md) *(240 §)* | Priority classes (P0 SOS preemption), broadcast beaconing, high-contrast disaster mode UI |
| **8. Diagnostics & Path Tracing** | [`sys-arch/18`](sys-arch/18-network-diagnostics-path-visualization-architecture.md) *(206 §)* | [`sys-arch/ui-ux-20`](sys-arch/ui-ux-20-diagnostics-network-paths-advanced-developer-architecture.md) *(225 §)* | RTT latency tracking, hop-by-hop mesh graph visualization, developer log inspection panel |
| **9. App Settings & Privacy UI** | *(Covered by core crates)* | [`sys-arch/ui-ux-18`](sys-arch/ui-ux-18-settings-privacy-notifications-data-controls-architecture.md) *(219 §)* | Granular settings: data saver mode, auto-download rules, blocked contacts, notification priority |

---

## 7. Milestone 6: Transports, Daemons & Hardware Infrastructure (Tier 3)

> **Execution Strategy**: Broadens connectivity beyond standard internet IP/QUIC into direct ad-hoc peer-to-peer radio protocols, headless relay infrastructure, and embedded system builds.

| Spec Document | Sections | Focus / Responsibility | Target Components |
|:---|:---:|:---|:---|
| [`11-relay-self-hosted-infrastructure-architecture.md`](sys-arch/11-relay-self-hosted-infrastructure-architecture.md) | **194** | Self-hosted encrypted relay servers, NAT hole-punching fallback | [`apps/emergency-node`](apps/emergency-node) |
| [`12-multipath-networking-architecture(1).md`](<sys-arch/12-multipath-networking-architecture(1).md>) | **178** | Concurrent packet striping across Wi-Fi and Cellular, seamless interface migration | [`siar-connectivity`](crates/siar-connectivity) |
| [`13-battery-aware-scheduling-architecture.md`](sys-arch/13-battery-aware-scheduling-architecture.md) | **145** | Battery state monitoring, scan duty cycle throttling, low-power mesh maintenance | [`siar-connectivity`](crates/siar-connectivity) |
| [`14-proximity-abstraction-architecture.md`](sys-arch/14-proximity-abstraction-architecture.md) | **131** | Proximity hardware abstraction: BLE GATT server/client, Wi-Fi Direct, Wi-Fi Aware | [`siar-transport-ble`](crates/siar-transport-ble), [`siar-transport-wifi-direct`](crates/siar-transport-wifi-direct) |
| [`16-daemon-headless-runtime-architecture.md`](sys-arch/16-daemon-headless-runtime-architecture.md) | **211** | Headless Unix daemon, systemd socket activation, local Unix domain socket IPC | [`apps/emergency-node`](apps/emergency-node) |
| [`20-embedded-linux-node-architecture.md`](sys-arch/20-embedded-linux-node-architecture.md) | **230** | Minimal footprint OpenWrt, Raspberry Pi, solar-powered field station builds | [`apps/emergency-node`](apps/emergency-node) |
| [`19-c-abi-ffi-architecture.md`](sys-arch/19-c-abi-ffi-architecture.md) | **170** | Memory-safe C-ABI exports, UniFFI bridge generation for Android Kotlin / iOS Swift | [`crates/siar-ffi`](crates) |
| [`27-rust-driven-android-native-build-packaging-automation.md`](sys-arch/27-rust-driven-android-native-build-packaging-automation.md) | **279** | Hermetic Android native compilation, automated `jniLibs` staging, cargo xtask build pipelines | [`apps/android`](apps/android) |

---

## 8. Milestone 7: Extensibility, Ecosystem & Quality Gates (Tier 4 — Final)

> **Execution Strategy**: Third-party developer extensions, WebAssembly sandboxing, external chat bridges, and automated release quality gates. Intentionally reserved for the final phase to keep the core codebase lean and focused.

| Component | Backend / Quality Spec | Frontend UI Spec | Scope / Description |
|:---|:---|:---|:---|
| **Plugin Module Ecosystem** | [`sys-arch/24`](sys-arch/24-plugin-module-ecosystem-architecture.md) *(305 §)* | [`sys-arch/ui-ux-19`](sys-arch/ui-ux-19-plugin-module-ecosystem-architecture.md) *(266 §)* | Wasmtime sandbox host, capability-based permission prompts, plugin marketplace UI |
| **WASM Targets** | [`sys-arch/22`](sys-arch/22-wasm-compatible-components-architecture.md) *(254 §)* | — | Browser WASM build targets, WebCrypto integration, web worker concurrency |
| **Third-Party Extensions** | [`sys-arch/21`](sys-arch/21-third-party-protocol-extensions-architecture.md) *(248 §)* | — | Publisher manifest signatures, sandboxed extension execution |
| **External Interoperability** | [`sys-arch/23`](sys-arch/23-external-interoperability-suite-architecture.md) *(255 §)* | — | Conformance test suites for Matrix, Bifrost, and RFC protocol bridges |
| **Accessibility (A11y)** | — | [`sys-arch/ui-ux-21`](<sys-arch/ui-ux-21-accessibility-inclusive-interaction-architecture (1).md>) *(294 §)* | Screen reader semantic labeling, high-contrast modes, dynamic text reflow |
| **Protocol Fuzzing Suite** | [`sys-arch/10`](sys-arch/10-fuzzing-protocol-test-suite-architecture.md) *(207 §)* | — | AFL / `cargo-fuzz` harness, property-based network partition simulation |
| **UI Release Quality Gates** | — | [`sys-arch/ui-ux-27`](sys-arch/ui-ux-27-ui-testing-screenshot-interaction-release-quality-gates-architecture.md) *(274 §)* | Automated screenshot regression diffs, cross-platform interaction testing gates |

---

## 9. Master Specification Inventory (All 60 Documents)

### Core Architecture Specs (01–33) — 6,863 Sections

| Spec | Title | Sections | Status | Milestone |
|:---:|:---|:---:|:---:|:---:|
| 01 | [Protocol Extension System](sys-arch/01-protocol-extension-system-architecture.md) | 108 | 108/108 (100%) | Milestone 1 |
| 02 | [Multi-Device Identity](sys-arch/02-multi-device-identity-architecture.md) | 204 | 204/204 (100%) | Milestone 1 |
| 03 | [Transport & Routing Policy](sys-arch/03-transport-routing-policy-engine-architecture.md) | 200 | ~198/200 (99%) | Milestone 1 (Active) |
| 04 | [Offline Event Log](sys-arch/04-offline-event-log-architecture.md) | 95 | 10/95 (11%) | Milestone 1 |
| 05 | [Robust File Blob Subsystem](sys-arch/05-robust-file-blob-subsystem-architecture.md) | 210 | 23/210 (11%) | Milestone 1 |
| 06 | [DTN Store-Carry-Forward](sys-arch/06-dtn-store-carry-forward-architecture.md) | 192 | 50/192 (26%) | Milestone 1 |
| 07 | [Capability Negotiation](sys-arch/07-capability-negotiation-architecture.md) | 164 | 19/164 (12%) | Milestone 1 |
| 08 | [Resource Limits & Backpressure](sys-arch/08-resource-limits-backpressure-architecture.md) | 193 | 56/193 (29%) | Milestone 1 |
| 09 | [Crash Recovery](sys-arch/09-crash-recovery-architecture.md) | 186 | 15/186 (8%) | Milestone 1 |
| 10 | [Fuzzing Protocol Test Suite](sys-arch/10-fuzzing-protocol-test-suite-architecture.md) | 207 | 0/207 (0%) | Milestone 7 |
| 11 | [Relay Self-Hosted Infrastructure](sys-arch/11-relay-self-hosted-infrastructure-architecture.md) | 194 | 0/194 (0%) | Milestone 6 |
| 12 | [Multipath Networking](<sys-arch/12-multipath-networking-architecture(1).md>) | 178 | 0/178 (0%) | Milestone 6 |
| 13 | [Battery-Aware Scheduling](sys-arch/13-battery-aware-scheduling-architecture.md) | 145 | 0/145 (0%) | Milestone 6 |
| 14 | [Proximity Abstraction](sys-arch/14-proximity-abstraction-architecture.md) | 131 | 0/131 (0%) | Milestone 6 |
| 15 | [QR/NFC Bootstrap Pairing](sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md) | 176 | 0/176 (0%) | Milestone 5 |
| 16 | [Daemon Headless Runtime](sys-arch/16-daemon-headless-runtime-architecture.md) | 211 | 0/211 (0%) | Milestone 6 |
| 17 | [Emergency Priority Classes](sys-arch/17-emergency-priority-classes-architecture.md) | 188 | 0/188 (0%) | Milestone 5 |
| 18 | [Network Diagnostics](sys-arch/18-network-diagnostics-path-visualization-architecture.md) | 206 | 0/206 (0%) | Milestone 5 |
| 19 | [C-ABI FFI Architecture](sys-arch/19-c-abi-ffi-architecture.md) | 170 | 0/170 (0%) | Milestone 6 |
| 20 | [Embedded Linux Node](sys-arch/20-embedded-linux-node-architecture.md) | 230 | 0/230 (0%) | Milestone 6 |
| 21 | [Third-Party Protocol Extensions](sys-arch/21-third-party-protocol-extensions-architecture.md) | 248 | 0/248 (0%) | Milestone 7 |
| 22 | [WASM-Compatible Components](sys-arch/22-wasm-compatible-components-architecture.md) | 254 | 0/254 (0%) | Milestone 7 |
| 23 | [External Interoperability Suite](sys-arch/23-external-interoperability-suite-architecture.md) | 255 | 0/255 (0%) | Milestone 7 |
| 24 | [Plugin Module Ecosystem](sys-arch/24-plugin-module-ecosystem-architecture.md) | 305 | 0/305 (0%) | Milestone 7 |
| 25 | [Android Zero-Copy Media](sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md) | 213 | 0/213 (0%) | Milestone 5 |
| 26 | [Rust-First Audio DSP](sys-arch/26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md) | 220 | 0/220 (0%) | Milestone 5 |
| 27 | [Android Native Packaging](sys-arch/27-rust-driven-android-native-build-packaging-automation.md) | 279 | 0/279 (0%) | Milestone 6 |
| 28 | [Production Security & E2EE](sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md) | 127 | 46/127 (36%) | Milestone 2 |
| 29 | [Realtime Calls Media Session](sys-arch/29-realtime-calls-media-session-protocol-architecture.md) | 275 | 0/275 (0%) | Milestone 5 |
| 30 | [Presence, Typing & Receipts](sys-arch/30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md) | 269 | 0/269 (0%) | Milestone 5 |
| 31 | [Notifications & Push Lifecycle](sys-arch/31-notifications-push-background-delivery-lifecycle-architecture.md) | 308 | 0/308 (0%) | Milestone 5 |
| 32 | [Search Indexing & Privacy](sys-arch/32-search-indexing-local-knowledge-privacy-architecture.md) | 242 | 0/242 (0%) | Milestone 5 |
| 33 | [Backup, Restore & Archival](sys-arch/33-backup-restore-export-import-archival-portability-architecture.md) | 280 | 0/280 (0%) | Milestone 5 |

---

### UI/UX Specs (ui-ux-01–27) — 6,030 Sections

| Spec | Title | Sections | Status | Milestone |
|:---:|:---|:---:|:---:|:---:|
| 01 | [Product Foundation Interaction](sys-arch/ui-ux-01-product-foundation-cross-platform-interaction-architecture.md) | 83 | 0/83 (0%) | Milestone 3 |
| 02 | [Desktop Dioxus App Shell](sys-arch/ui-ux-02-desktop-dioxus-app-shell-navigation-window-architecture.md) | 227 | 0/227 (0%) | Milestone 3 |
| 03 | [Android Compose App Shell](sys-arch/ui-ux-03-android-jetpack-compose-app-shell-navigation-lifecycle-architecture.md) | 217 | 0/217 (0%) | Milestone 3 |
| 04 | [Conversation List & Inbox](sys-arch/ui-ux-04-conversation-list-inbox-architecture.md) | 213 | 0/213 (0%) | Milestone 4 |
| 05 | [Message Timeline](sys-arch/ui-ux-05-conversation-message-timeline-architecture.md) | 254 | 0/254 (0%) | Milestone 4 |
| 06 | [Message Composer & Drafts](sys-arch/ui-ux-06-message-composer-attachments-voice-notes-drafts-architecture.md) | 309 | 0/309 (0%) | Milestone 4 |
| 07 | [Realtime Calls UI](sys-arch/ui-ux-07-calls-realtime-media-architecture.md) | 235 | 0/235 (0%) | Milestone 5 |
| 08 | [Contacts & Verification](sys-arch/ui-ux-08-contacts-requests-verification-identity-architecture.md) | 227 | 0/227 (0%) | Milestone 4 |
| 09 | [Groups & Membership Roles](sys-arch/ui-ux-09-groups-membership-roles-architecture.md) | 224 | 0/224 (0%) | Milestone 4 |
| 10 | [Files & Media Gallery](sys-arch/ui-ux-10-files-media-gallery-transfer-architecture.md) | 161 | 0/161 (0%) | Milestone 4 |
| 11 | [Search & Knowledge Retrieval](sys-arch/ui-ux-11-search-local-knowledge-retrieval-architecture.md) | 148 | 0/148 (0%) | Milestone 5 |
| 12 | [Nearby QR/NFC Pairing](sys-arch/ui-ux-12-nearby-qr-nfc-pairing-device-linking-architecture.md) | 237 | 0/237 (0%) | Milestone 5 |
| 13 | [Background Notifications UI](sys-arch/ui-ux-13-notifications-background-incoming-call-architecture.md) | 215 | 0/215 (0%) | Milestone 5 |
| 14 | [Presence & Typing Indicators](sys-arch/ui-ux-14-presence-typing-receipts-status-architecture.md) | 197 | 0/197 (0%) | Milestone 5 |
| 15 | [Security Center UI](sys-arch/ui-ux-15-security-center-devices-keys-recovery-architecture.md) | 221 | 184/221 (83%) | Milestone 3 |
| 16 | [Backup, Restore & Migration UI](sys-arch/ui-ux-16-backup-restore-export-migration-architecture.md) | 223 | 0/223 (0%) | Milestone 5 |
| 17 | [Emergency SOS Mode UI](sys-arch/ui-ux-17-emergency-sos-offline-mesh-architecture.md) | 240 | 0/240 (0%) | Milestone 5 |
| 18 | [Settings & Privacy Controls](sys-arch/ui-ux-18-settings-privacy-notifications-data-controls-architecture.md) | 219 | 0/219 (0%) | Milestone 5 |
| 19 | [Plugin Ecosystem UI](sys-arch/ui-ux-19-plugin-module-ecosystem-architecture.md) | 266 | 0/266 (0%) | Milestone 7 |
| 20 | [Diagnostics & Developer UI](sys-arch/ui-ux-20-diagnostics-network-paths-advanced-developer-architecture.md) | 225 | 0/225 (0%) | Milestone 5 |
| 21 | [Accessibility & Inclusion](<sys-arch/ui-ux-21-accessibility-inclusive-interaction-architecture (1).md>) | 294 | 0/294 (0%) | Milestone 7 |
| 22 | [Design System Tokens & Theme](sys-arch/ui-ux-22-design-system-tokens-typography-icons-motion-architecture.md) | 253 | 0/253 (0%) | Milestone 3 |
| 23 | [Responsive Adaptive Layout](sys-arch/ui-ux-23-responsive-adaptive-desktop-tablet-foldable-phone-layout-architecture.md) | 223 | 0/223 (0%) | Milestone 3 |
| 24 | [Error, Degraded & Empty States](sys-arch/ui-ux-24-error-loading-empty-offline-degraded-state-architecture.md) | 205 | 0/205 (0%) | Milestone 4 |
| 25 | [Onboarding & Key Wizard](sys-arch/ui-ux-25-onboarding-first-run-permission-education-architecture.md) | 209 | 0/209 (0%) | Milestone 3 |
| 26 | [Virtualized Large Data UI](sys-arch/ui-ux-26-performance-virtualization-large-data-ui-architecture.md) | 276 | 0/276 (0%) | Milestone 4 |
| 27 | [UI Testing & Quality Gates](sys-arch/ui-ux-27-ui-testing-screenshot-interaction-release-quality-gates-architecture.md) | 274 | 0/274 (0%) | Milestone 7 |

---

## 10. Current Position & Immediate Execution Steps

```text
Current Milestone: MILESTONE 1 (Tier 0: Headless Core Engine)
Current Document : Spec 03 (03-transport-routing-policy-engine-architecture.md)
Target Crate     : crates/siar-routing-policy
Current Status   : ~198 / 200 Sections (99% Reconciled, 245/245 Tests Passing)
```

1. **Completed Batches**:
   * Spec 01: 108/108 (100%) — complete
   * Spec 02: 204/204 (100%) — complete
   * Spec 03 Round 1: §1–§42 (30%)
   * Spec 03 Round 2: §43–§56 (37%)
   * Spec 03 Round 3: §57–§62 (40%)
   * Spec 03 Round 4: §63–§68 (43%)
   * Spec 03 Round 5: §69–§74 (46%)
   * Spec 03 Round 6: §75–§84 (51%)
   * Spec 03 Round 7: §85–§90 (54%)
   * Spec 03 Round 8: §91–§96 (57%)
   * Spec 03 Round 9: §97–§104 (61%)
   * Spec 03 Round 10: §105–§107 (64%)
   * Spec 03 Round 11: §108–§115 (68%)
   * Spec 03 Round 12: §116–§120 (70%)
   * Spec 03 Round 13: §121–§127 (74%)
   * Spec 03 Round 14: §128–§139 (78%)
   * Spec 03 Round 15: §140–§150 (84%)
   * Spec 03 Round 16: §151–§159 (89%)
   * Spec 03 Round 17: §160–§170 (95%)
   * Spec 03 Round 18: §171–§182 (99%)
2. **Next Batch (Round 19 / Final Stretch)**: Implement sections **§183 onward** of Spec 03:
   * Testing Matrix, Golden & Property Tests, Architecture Reconciliation & Definition of Done (§183–§200)
