# Master System Integration Architecture & Phased Implementation Roadmap

## SIAR: Survivable Identity & Autonomous Routing Platform

> **Comprehensive Architecture Index:** All low-level subsystem specifications are fully documented in the 33 architectural specifications located in [`sys-arch/`](file:///home/irshad/Projects/siar/sys-arch). This master document defines the global system integration, crate dependency hierarchy, application orchestration layers, cross-cutting architectural invariants, and the phased implementation roadmap.

---

## 1. System Vision & Core Invariants

SIAR is a production-grade, decentralized, local-first communication system engineered to operate seamlessly across high-speed internet, local area networks, proximity ad-hoc links (Wi-Fi Direct, Wi-Fi Aware, Bluetooth), and disconnected delay-tolerant mesh networks.

```text
┌─────────────────────────────────────────────────────────────────────────┐
│                           APPLICATION LAYER                             │
│  apps/desktop (Dioxus)  │  apps/android (Dioxus+Kotlin)  │  apps/cli    │
├─────────────────────────────────────────────────────────────────────────┤
│                     PRESENTATION & UI STATE LAYER                       │
│  crates/siar-ui-state   │  ui-gui-architecture.md  │  mobile-driven.md  │
├─────────────────────────────────────────────────────────────────────────┤
│                   MESSAGING & REALTIME MEDIA ENGINES                    │
│  siar-messaging │ siar-calls │ siar-media-core │ siar-media-av1/audio   │
├─────────────────────────────────────────────────────────────────────────┤
│                 IDENTITY & END-TO-END SECURITY LAYER                    │
│  siar-identity-multidevice │ siar-crypto │ siar-crypto-mls              │
├─────────────────────────────────────────────────────────────────────────┤
│                 STORAGE & TRANSACTIONAL OUTBOX LAYER                    │
│  siar-storage (Embedded SQL) │ siar-dtn-bundle (Store-Carry-Forward)    │
├─────────────────────────────────────────────────────────────────────────┤
│                  AUTONOMOUS MULTIPATH ROUTING CORE                      │
│  siar-routing-policy │ siar-connectivity │ siar-protocol-ext            │
├─────────────────────────────────────────────────────────────────────────┤
│                       PLUGGABLE TRANSPORTS                              │
│  siar-transport (Iroh/QUIC) │ siar-transport-ble │ -wifi-direct/-aware  │
└─────────────────────────────────────────────────────────────────────────┘
```

### Core Architectural Invariants
1. **Zero Central Dependency:** Ordinary direct communication between two reachable peers never requires a central database, server, or cloud account.
2. **Transport Independence:** Messages belong to the application protocol and are encapsulated in secure envelopes. Transports (Iroh QUIC, LAN, Wi-Fi Direct, BLE, DTN) merely carry ciphertext envelopes.
3. **Persist-Before-Send:** All messages and state changes are committed to local transactional storage before network transmission is attempted, ensuring complete crash resilience (see [`sys-arch/04`](file:///home/irshad/Projects/siar/sys-arch/04-offline-event-log-architecture.md)).
4. **Content-Addressed Blobs:** Large media and files are decoupled from chat frames, encrypted with random keys, and addressed via BLAKE3 hashes (see [`sys-arch/05`](file:///home/irshad/Projects/siar/sys-arch/05-robust-file-blob-subsystem-architecture.md)).

---

## 2. Complete 33-Part Architectural Specification Directory

| Part | Specification Document | Primary Crate / Domain | Core Focus |
| :---: | :--- | :--- | :--- |
| **01** | [`01-protocol-extension-system-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/01-protocol-extension-system-architecture.md) | `siar-protocol-ext` | Protocol negotiation, extension descriptors & versioning |
| **02** | [`02-multi-device-identity-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/02-multi-device-identity-architecture.md) | `siar-identity-multidevice` | Account root authority, device certs & revocation |
| **03** | [`03-transport-routing-policy-engine-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/03-transport-routing-policy-engine-architecture.md) | `siar-routing-policy` | Dynamic link scoring, path selection & failover |
| **04** | [`04-offline-event-log-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/04-offline-event-log-architecture.md) | `siar-storage`, `siar-messaging` | Transactional outbox, event ordering & delivery states |
| **05** | [`05-robust-file-blob-subsystem-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/05-robust-file-blob-subsystem-architecture.md) | `siar-storage` | BLAKE3 chunking, resumable transfers & deduplication |
| **06** | [`06-dtn-store-carry-forward-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/06-dtn-store-carry-forward-architecture.md) | `siar-dtn-bundle` | Delay-tolerant routing, epidemic anti-entropy & quotas |
| **07** | [`07-capability-negotiation-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/07-capability-negotiation-architecture.md) | `siar-protocol` | Runtime capability exchange & transport handoffs |
| **08** | [`08-resource-limits-backpressure-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/08-resource-limits-backpressure-architecture.md) | `siar-resource-limits`, `siar-storage` | Token-bucket rate limiting, backpressure & DoS defense |
| **09** | [`09-crash-recovery-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/09-crash-recovery-architecture.md) | `siar-storage`, `apps/desktop` | WAL recovery, panic hooks & corrupted state isolation |
| **10** | [`10-fuzzing-protocol-test-suite-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/10-fuzzing-protocol-test-suite-architecture.md) | `fuzz/`, `tests/` | Protocol fuzzers, property tests & network simulator |
| **11** | [`11-relay-self-hosted-infrastructure-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/11-relay-self-hosted-infrastructure-architecture.md) | `apps/relay` | Self-hosted Iroh relays & encrypted mailbox servers |
| **12** | [`12-multipath-networking-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/12-multipath-networking-architecture.md) | `siar-connectivity` | Concurrent link scheduling, link bonding & migration |
| **13** | [`13-battery-aware-scheduling-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/13-battery-aware-scheduling-architecture.md) | `siar-connectivity` | Power budgets, scan duty cycling & thermal throttling |
| **14** | [`14-proximity-abstraction-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/14-proximity-abstraction-architecture.md) | `siar-transport-*` | Bluetooth Classic, BLE GATT, Wi-Fi Direct & Aware |
| **15** | [`15-qr-nfc-bootstrap-pairing-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md) | `siar-identity-multidevice` | Out-of-band QR/NFC device linking & SAS verification |
| **16** | [`16-daemon-headless-runtime-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/16-daemon-headless-runtime-architecture.md) | `apps/emergency-node` | Headless service runtime, systemd integration & IPC |
| **17** | [`17-emergency-priority-classes-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/17-emergency-priority-classes-architecture.md) | `siar-emergency` | P0–P4 priority queues, emergency preemption & SOS |
| **18** | [`18-network-diagnostics-path-visualization-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/18-network-diagnostics-path-visualization-architecture.md) | `siar-ui-state` | RTT metrics, path trace graph exporter & diagnostics |
| **19** | [`19-c-abi-ffi-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/19-c-abi-ffi-architecture.md) | `crates/siar-ffi` | Stable C ABI, memory ownership & UniFFI bridge |
| **20** | [`20-embedded-linux-node-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/20-embedded-linux-node-architecture.md) | `apps/emergency-node` | OpenWrt/Raspberry Pi embedded appliance builds |
| **21** | [`21-third-party-protocol-extensions-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/21-third-party-protocol-extensions-architecture.md) | `siar-protocol-ext` | Manifest signing, dynamic extensibility & sandboxing |
| **22** | [`22-wasm-compatible-components-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/22-wasm-compatible-components-architecture.md) | `crates/siar-wasm` | WebAssembly compilation targets & browser runtime |
| **23** | [`23-external-interoperability-suite-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/23-external-interoperability-suite-architecture.md) | `tests/conformance` | Matrix/Bifrost/RFC bridge conformance testing suite |
| **24** | [`24-plugin-module-ecosystem-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/24-plugin-module-ecosystem-architecture.md) | `siar-plugin` | Wasmtime plugin host, permission grants & isolation |
| **25** | [`25-android-direct-hardware-surface-zero-copy-media-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md) | `siar-media-android` | Android MediaCodec zero-copy Surface pipelines |
| **26** | [`26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/26-rust-first-audio-dsp-resampling-aec-ns-agc-architecture.md) | `siar-media-audio` | Pure Rust audio DSP: AEC, NS, AGC, Opus codecs |
| **27** | [`27-rust-driven-android-native-build-packaging-automation.md`](file:///home/irshad/Projects/siar/sys-arch/27-rust-driven-android-native-build-packaging-automation.md) | `xtask`, `apps/android` | Automated APK/AAB builds, jniLibs staging & signing |
| **28** | [`28-production-security-e2ee-key-management-privacy-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md) | `siar-crypto`, `-mls` | Double Ratchet, MLS group crypto & secure key storage |
| **29** | [`29-realtime-calls-media-session-protocol-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/29-realtime-calls-media-session-protocol-architecture.md) | `siar-calls` | Session handshake, SRTP/QUIC media streams & jitter buffer |
| **30** | [`30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/30-presence-availability-typing-read-receipts-ephemeral-state-architecture.md) | `siar-messaging` | Ephemeral state engine, typing indicators & receipts |
| **31** | [`31-notifications-push-background-delivery-lifecycle-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/31-notifications-push-background-delivery-lifecycle-architecture.md) | `siar-connectivity` | UnifiedPush, FCM, APNs & background execution |
| **32** | [`32-search-indexing-local-knowledge-privacy-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/32-search-indexing-local-knowledge-privacy-architecture.md) | `siar-storage` | Privacy-preserving local full-text search (Tantivy/FTS) |
| **33** | [`33-backup-restore-export-import-archival-portability-architecture.md`](file:///home/irshad/Projects/siar/sys-arch/33-backup-restore-export-import-archival-portability-architecture.md) | `siar-storage` | Encrypted archival backup, passphrase KDF & streaming import |

---

## 3. Application Orchestration Architecture

SIAR compiles into four distinct application packages sharing the same underlying Rust engine:

### 1. `apps/desktop` (Linux, Windows, macOS)
- **UI Framework:** Dioxus Desktop with system WebView integration.
- **Responsibilities:** Multi-window support, system tray integration, global keyboard shortcuts, local desktop notifications, file drag-and-drop.
- **Packaging:** AppImage/deb/rpm (Linux), EXE/MSI (Windows), DMG/App (macOS) (see [`video-codec.md`](file:///home/irshad/Projects/siar/video-codec.md)).

### 2. `apps/android` (Android Mobile)
- **UI Framework:** Dioxus Mobile + Kotlin Platform Adapter layer.
- **Responsibilities:** Android lifecycle management, foreground services for calling/transfers, MediaCodec zero-copy surfaces, Bluetooth/Wi-Fi permission bridges (see [`mobile-driven-ui-ux-architecture.md`](file:///home/irshad/Projects/siar/mobile-driven-ui-ux-architecture.md) and [`sys-arch/25`](file:///home/irshad/Projects/siar/sys-arch/25-android-direct-hardware-surface-zero-copy-media-architecture.md)).

### 3. `apps/cli` (Command Line & Testing Client)
- **Responsibilities:** Headless messaging, scriptable test harness, automated peer-to-peer benchmarking, headless CI validation.

### 4. `apps/emergency-node` (Embedded & Field Relay Daemon)
- **Responsibilities:** Autonomous background daemon for headless solar/battery field nodes, DTN store-carry-forward repository, community bulletin board gateway (see [`off-grid.md`](file:///home/irshad/Projects/siar/off-grid.md) and [`sys-arch/20`](file:///home/irshad/Projects/siar/sys-arch/20-embedded-linux-node-architecture.md)).

---

## 4. Master Phased Implementation Roadmap

```text
Phase 1: Core Engine & Identity (Maturity: 85%)
├── Complete: Ed25519 root identity, device certificates, multi-device directories
└── In Progress: QR/NFC out-of-band linking flow (sys-arch/02, sys-arch/15)

Phase 2: Storage, Outbox & Mesh Transports (Maturity: 80%)
├── Complete: Transactional SQL outbox, BLAKE3 blob chunking, DTN store-carry-forward
└── In Progress: Packet-level ECMP striping across heterogeneous links (sys-arch/03, 04, 05, 06)

Phase 3: Realtime Media & Audio/Video Calls (Maturity: 65%)
├── Complete: Call state machine, Opus audio DSP, AV1/MediaCodec abstraction
└── In Progress: Hardware Surface zero-copy pipeline integration on Android (sys-arch/25, 26, 29)

Phase 4: Dioxus Desktop & Mobile UI (Maturity: 55%)
├── Complete: UI state viewmodels, command bus, design token system
└── In Progress: Virtualized timeline & native Android Kotlin bridge (ui-gui-architecture.md, mobile-driven.md)

Phase 5: Production Hardening, Packaging & Release (Maturity: 60%)
├── Complete: xtask Android build automation, cargo deny/audit supply chain gates
└── In Progress: Multi-platform signing, notarization & auto-updater (video-codec.md, sys-arch/27)
```
