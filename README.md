<p align="center">
  <img src="assets/branding/logo.png" alt="SIAR Logo" width="600"/>
</p>

<h1 align="center">SIAR — Survivable Identity & Autonomous Routing</h1>

<p align="center">
  <a href="https://irshadali5.github.io/siar-site/"><img src="https://img.shields.io/badge/Official%20Site-siar--site-00f2fe?style=flat-square" alt="Official Website"/></a>
  <a href="wiki/Home.md"><img src="https://img.shields.io/badge/Wiki-26%20Chapters-8b5cf6?style=flat-square" alt="Technical Wiki"/></a>
  <a href="https://irshadali5.github.io/siar-site/guide.html"><img src="https://img.shields.io/badge/User%20Manual-Guide%20%26%20Ops-34d399?style=flat-square" alt="User Manual"/></a>
  <a href="https://irshadali5.github.io/siar-site/sys-arch/"><img src="https://img.shields.io/badge/Architecture-mdBook%20Portal-a78bfa?style=flat-square" alt="Architecture Specs"/></a>
  <a href="https://irshadali5.github.io/siar-site/docs.html"><img src="https://img.shields.io/badge/Developer-C--ABI%20%26%20APIs-38bdf8?style=flat-square" alt="Developer Hub"/></a>
  <a href="https://irshadali5.github.io/siar-site/packages.html"><img src="https://img.shields.io/badge/Downloads-Packages-fbbf24?style=flat-square" alt="Package Center"/></a>
  <a href="#license--dual-tier-model"><img src="https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0%20%7C%20AGPLv3-blue.svg?style=flat-square" alt="License"/></a>
</p>

> **Official Showcase, Package Center & Live Architecture Documentation:**
> - 🌐 **[Official Product Presentation](https://irshadali5.github.io/siar-site/)**
> - 📚 **[Comprehensive Technical Wiki (26 Chapters)](wiki/Home.md)** — System Stack, Multi-Device Trust, Routing, DTN, Security Center, Calls, Testing & Operations
> - 📘 **[End-to-End User & Operations Manual](https://irshadali5.github.io/siar-site/guide.html)** — Zero-Knowledge Setup, QR/NFC Pairing, Vault Export/Import & Anti-Forensics
> - 📑 **[System Architecture & Protocol Specs (mdBook)](https://irshadali5.github.io/siar-site/sys-arch/)** — 33+ numbered core specs + 27 UI/UX specs
> - 📦 **[Universal Package Center](https://irshadali5.github.io/siar-site/packages.html)** — .deb, .rpm, .apk, .dmg, brew, winget
> - 🔬 **[Interactive WASM Mesh Simulator Lab](https://irshadali5.github.io/siar-site/simulator.html)**
> - 🛡️ **[SLSA Level 3+ Supply Chain Cryptographic Verifier](https://irshadali5.github.io/siar-site/verify.html)**

---

## Table of Contents

- [What SIAR Is](#what-siar-is)
- [Core Design Rules](#core-design-rules)
- [Architecture: Five Layers](#architecture-five-layers)
- [Transport & Routing Policy Engine](#transport--routing-policy-engine)
- [Multi-Device Identity & Trust](#multi-device-identity--trust)
- [End-to-End Security Model](#end-to-end-security-model)
- [DTN: Store-Carry-Forward](#dtn-store-carry-forward)
- [Realtime Calls & Media](#realtime-calls--media)
- [Headless Daemon & Embedded Nodes](#headless-daemon--embedded-nodes)
- [Protocol Extensions & WASM](#protocol-extensions--wasm)
- [Anonymity Transport Plane](#anonymity-transport-plane)
- [Workspace Crate Map (33 Crates)](#workspace-crate-map-33-crates)
- [Deployment Modes](#deployment-modes)
- [SIAR vs Traditional Messengers](#siar-vs-traditional-messengers)
- [Implementation Status & Roadmap](#implementation-status--roadmap)
- [Prerequisites & Environment Setup](#prerequisites--environment-setup)
- [Build & Compilation Tutorial](#build--compilation-tutorial)
- [Node Configuration & User Guide](#node-configuration--user-guide)
- [Testing & Fuzzing](#testing--fuzzing)
- [License & Dual-Tier Model](#license--dual-tier-model)

---

## What SIAR Is

**SIAR** ( **Survivable Identity & Autonomous Routing** ) is a **reusable, zero-infrastructure, multi-transport, offline-first P2P communication platform**.

Traditional messengers (WhatsApp, Signal, Telegram) assume always-on infrastructure: centralized server clusters, public DNS, PKI, and continuous Internet access. When natural disasters strike, telecommunications infrastructure collapses, or state-level censorship severs external backhauls, these apps completely fail.

SIAR is engineered from first principles to invert these assumptions:

1. **Zero-Infrastructure Invariant** — the system must operate seamlessly when zero external servers, DNS nodes, or Internet gateways are reachable.
2. **Cryptographic Sovereignty** — identities are root Ed25519 signing pairs owned exclusively by local devices — no phone numbers, email addresses, or centralized registries.
3. **Opportunistic Dissemination** — messages and files are delay-tolerant bundles that travel over any available medium (BLE, Wi-Fi Direct, Wi-Fi Aware, Bluetooth Classic, LAN, Internet QUIC) through store-carry-forward physical mules.
4. **Unified Cross-Platform Core** — a high-performance, memory-safe pure-Rust workspace powers CLI daemons, solar-powered mesh repeaters, Android apps, and desktop interfaces.
5. **Reusable Platform** — the underlying architecture is not messenger-specific. The same crates back messaging, files, emergency SOS, group comms, ERP payloads, and future custom applications.

The governing rule from [`sys-arch/`](sys-arch/):

> **The application expresses intent and constraints. The platform selects paths, manages retries, falls back to DTN, and re-establishes sessions — without the application understanding transport mechanics.**

---

## Core Design Rules

These rules appear directly in the sys-arch specifications and govern every crate in the workspace:

| Rule | Specification Source |
| :--- | :--- |
| Feature code must never implement transport selection | `sys-arch/03` §2 |
| The daemon owns durable state; UI is a client of the runtime | `sys-arch/16` §1 |
| Transport security protects the path; application E2EE protects the conversation regardless of path | `sys-arch/28` §1 |
| A person is an account, not a device — routing must address accounts, not device keys | `sys-arch/02` §2 |
| DTN is not Bluetooth forwarding logic or emergency-only forwarding — it is a transport-neutral persistence layer | `sys-arch/06` §2 |
| Protocol extensions are a versioned capability architecture, not a dynamic plugin system for arbitrary untrusted code | `sys-arch/01` §2 |
| SIAR must treat anonymity as an explicit routing/security property, not a side effect of encryption | `sys-arch/34` §1 |
| The call is a logical secure session; network paths, codecs, devices, and surfaces are replaceable implementation resources inside that session | `sys-arch/29` §1 |

---

## Architecture: Five Layers

SIAR is structured into five distinct, decoupled architectural layers:

```
+---------------------------------------------------------------------------------------+
|                                5. Applications & UI/UX                                |
|  - apps/android (Jetpack Compose + JNI)   - apps/desktop (Dioxus 0.7 Desktop GUI)     |
|  - apps/emergency-node (Solar Repeater)   - apps/cli (Dev Diagnostics & CLI Node)     |
+---------------------------------------------------------------------------------------+
|                                4. High-Level Services                                 |
|  - siar-messaging (Message & Group Engine)- siar-calls (Realtime AV1 / Opus Sessions) |
|  - siar-identity-multidevice (Certs, SAS) - siar-ui-state (Security Center & Flows)   |
+---------------------------------------------------------------------------------------+
|                            3. Routing, Policy & DTN Engine                            |
|  - siar-routing-policy (Multi-Metric)     - siar-dtn-bundle (Spray-and-Wait Forward)  |
|  - siar-connectivity (Link State Probes)  - siar-emergency (Priority Classes P0–P3)   |
|  - siar-routing (PathTable & Latency)     - siar-dtn (Store-Carry-Forward Buffer)     |
+---------------------------------------------------------------------------------------+
|                             2. Storage, Crypto & Reliability                          |
|  - siar-crypto (Ed25519/X25519 AEAD)      - siar-crypto-mls (RFC 9420 MLS E2EE)       |
|  - siar-storage (Stoolap Embedded SQL)    - siar-blob-manifest (BLAKE3 Merkle DAG)    |
|  - siar-resource-limits (Backpressure)    - siar-crash-recovery (WAL & Checkpoints)   |
|  - siar-event-log (Monotonic Sequence Log)                                            |
+---------------------------------------------------------------------------------------+
|                              1. Transport & Wire Protocols                            |
|  - siar-protocol (Postcard Wire Codec)    - siar-protocol-ext (CapNeg & Scheduler)    |
|  - siar-capability (2-Phase Confirmation) - siar-transport (Pooled Connection Socket) |
|  - siar-transport-ble / -ble-android      - siar-transport-bluetooth-classic (RFCOMM) |
|  - siar-transport-wifi-direct (P2P)       - siar-transport-wifi-aware (NAN Proximity) |
+---------------------------------------------------------------------------------------+
```

---

## Transport & Routing Policy Engine

*Specification: [`sys-arch/03`](sys-arch/03-transport-routing-policy-engine-architecture.md) · Crate: [`siar-routing-policy`](crates/siar-routing-policy)*

The platform supports all of the following physical paths simultaneously:

```
Iroh QUIC (direct / relay)   Wi-Fi Direct (P2P)
Local LAN (multicast)        Wi-Fi Aware (NAN)
Bluetooth Classic (RFCOMM)   Bluetooth LE (GATT)
DTN / store-carry-forward    future transports
```

Rather than letting application code pick a transport, each operation publishes **`DeliveryRequirements`** (priority class, deadline, bandwidth floor, metered/roaming policy, relay allowance, DTN opt-in). The routing policy engine:

1. Resolves candidate paths for the destination account across all of its devices.
2. Scores each candidate with a **multi-metric weighted formula** (RTT, bandwidth, setup cost, stability, energy cost, congestion, candidate state).
3. Applies hard constraint elimination (metered, roaming, background restriction, scope, revoked device, deadline).
4. Applies layered policy checks (system → application → user → operation → network context).
5. Returns a `RoutePlan` (`Direct` / `Redundant` / `Hedged` / `Dtn`) with per-device routing for multi-device accounts.

Policy profiles include: `RealTime`, `Interactive`, `Bulk`, `Background`, `Emergency`, `LowPower`, `OffGrid`. The engine also enforces stickiness hysteresis to prevent oscillation under measurement jitter.

SIAR automatically shifts between three operational modes:

| Mode | Available Networks | Delivery Mechanism |
| :--- | :--- | :--- |
| **Connected Online** | Internet / LAN / Wi-Fi | Iroh QUIC, direct end-to-end streams |
| **Local Tactical Mesh** | Wi-Fi Direct, Wi-Fi Aware, LAN | Hop-by-hop mesh forwarding (sub-10 ms) |
| **Air-Gapped / Off-Grid** | BLE GATT, Bluetooth Classic | Physical mule DTN with Spray-and-Wait |

---

## Multi-Device Identity & Trust

*Specification: [`sys-arch/02`](sys-arch/02-multi-device-identity-architecture.md) · Crate: [`siar-identity-multidevice`](crates/siar-identity-multidevice)*

A person is not a device. SIAR distinguishes five identity layers:

```
Account Identity  →  Device Identity  →  Transport Identity  →  Session Identity  →  Application Profile
```

Each device generates an independent Ed25519 key pair and obtains a **`DeviceCertificate`** signed by the account's root authority. The trust model supports:

- **Multiple active devices** (phone, laptop, headless node, secondary phone)
- **Independent per-device keys** with device-specific transport addresses
- **Monotonic revocation** — a revoked device cannot forge retroactive authority
- **SAS (Short Authentication String) out-of-band verification** for secure linking (QR code, NFC, or verbal code)
- **Per-device capability sets** — a headless relay does not advertise `REALTIME_MEDIA`
- **Account recovery** with encrypted key backups and import validation before any local state is touched
- **Algorithm agility** — signature and KEM schemes are replaceable without breaking the identity model
- **Organization/enterprise namespaces** with multi-tenant composite keys

The identity layer is designed to remain **reusable outside the messenger** — the same model backs messaging, files, emergency SOS, ERP, and any future application built on this platform.

---

## End-to-End Security Model

*Specification: [`sys-arch/28`](sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md) · Crates: [`siar-crypto`](crates/siar-crypto), [`siar-crypto-mls`](crates/siar-crypto-mls)*

```
Application E2EE (MLS RFC 9420)
      ↓
Conversation / Group Security (OpenMLS tree ratchet)
      ↓
Authenticated Device Sessions (Ed25519 + X25519)
      ↓
Iroh / QUIC Transport Security
      ↓
Internet / LAN / BLE / Relay / DTN
```

**Threat model**: any relay server, DTN carrier, public network, nearby Bluetooth peer, server database, third-party plugin, external FFI caller, stolen device, or old backup can be hostile or compromised. The design trusts only: the current unlocked local device, explicitly authorized devices, verified contacts, and explicit organization/authority keys.

Required security properties:
- **End-to-end confidentiality** — ciphertext is unreadable by relays, carriers, or infrastructure operators
- **Forward secrecy** — compromise of today's keys does not expose past messages (MLS epoch ratchet)
- **Post-compromise security** — after a device is removed, new group state remains confidential from it
- **Offline / DTN-safe encryption** — messages encrypted for delivery remain sealed while carried by DTN mules
- **Metadata minimization** — routing decisions carry no content; `RouteMetricEvent` has no peer-identity or IP fields by construction

---

## DTN: Store-Carry-Forward

*Specification: [`sys-arch/06`](sys-arch/06-dtn-store-carry-forward-architecture.md) · Crates: [`siar-dtn`](crates/siar-dtn), [`siar-dtn-bundle`](crates/siar-dtn-bundle)*

DTN (Delay-Tolerant Networking) enables communication without a continuous end-to-end path:

```
Alice (no Internet, no Wi-Fi, no direct Bluetooth to destination)
  ↓
Bob (physical mule — encounters Alice via BLE)
  ↓
Carol (physical mule — encounters Bob via Wi-Fi Direct)
  ↓
Gateway (encounters Carol, has Internet)
  ↓
Destination
```

The DTN subsystem persists payloads locally, carries them across time, and forwards them when useful connectivity appears. It is transport-neutral and supports:

- Text messages, delivery receipts, emergency SOS, authority alerts
- Small file metadata and selected file chunks
- Custom application events for ERP and third-party integrations

**Spray-and-Wait** replication (configurable `dtn_replication_budget`) limits network flooding while maximising delivery probability. Bundle custody receipts confirm when a mule has accepted responsibility for forwarding. The routing policy engine integrates DTN as a first-class `RouteStrategy::Dtn` outcome — fallback to DTN is a policy decision, not a last-resort hack.

---

## Realtime Calls & Media

*Specification: [`sys-arch/29`](sys-arch/29-realtime-calls-media-session-protocol-architecture.md) · Crates: [`siar-calls`](crates/siar-calls), [`siar-media-android`](crates/siar-media-android), [`siar-media-audio`](crates/siar-media-audio), [`siar-media-av1`](crates/siar-media-av1)*

A call is a **logical secure session**; network paths, codecs, devices, and display surfaces are replaceable resources inside that session:

```
Call Controller
    ├── Signaling   (ringing / accept / reject / busy / timeout)
    ├── Security    (session authentication bound to E2EE identity)
    └── Policy      (codec selection, bitrate, hold/resume, path handoff)
         ├── siar-media-android  (Android MediaCodec zero-copy hardware surfaces)
         ├── siar-media-av1      (desktop dav1d AV1 software decoder)
         └── siar-media-audio    (Opus + AEC / NS / AGC DSP pipeline)
```

The call architecture supports:
- **P2P signaling** without a signaling server — session control messages travel over the same Iroh/mesh transport as regular messages
- **Multipath resilience** — the session survives a path change; the routing engine's `RouteChangeEvent::NewPath` triggers codec renegotiation, not call termination
- **Hardware zero-copy surfaces** on Android — `MediaCodec` decodes directly into `SurfaceTexture` without intermediate copy
- **Pure-Rust DSP on desktop** — Opus encode/decode, acoustic echo cancellation (AEC), noise suppression (NS), automatic gain control (AGC)
- **Crash-safe call history** — call records are written to durable storage before the session is confirmed torn down

---

## Headless Daemon & Embedded Nodes

*Specification: [`sys-arch/16`](sys-arch/16-daemon-headless-runtime-architecture.md) · App: [`apps/emergency-node`](apps/emergency-node)*

The platform does not require the user interface to remain alive for core networking to function:

```
Dioxus Desktop UI  ──Secure Local IPC──▶  comm-daemon / runtime
                                              ├── Identity
                                              ├── Messaging
                                              ├── Files & DTN
                                              ├── Routing & Multipath
                                              ├── Power policy
                                              └── Recovery
                                                      │
                                             Iroh / LAN / BLE / Wi-Fi
```

**`apps/emergency-node`** is a standalone headless daemon deployable on:
- Raspberry Pi, OpenWrt routers, embedded Linux single-board computers
- Solar-powered field nodes and dedicated mobile repeater boosters
- Linux servers for self-hosted relay infrastructure

In headless mode, the daemon autonomously stores, carries, and re-transmits encrypted bundles across disconnected network partitions — functioning as a **mesh repeater** during complete Internet blackouts and disaster scenarios, without any UI requirement.

Self-hosted relay infrastructure ([`sys-arch/11`](sys-arch/11-relay-self-hosted-infrastructure-architecture.md)) allows organizations to deploy private relay fleets, regional relay clusters, and DTN gateway bridges without lock-in to any centralized service.

---

## Protocol Extensions & WASM

*Specifications: [`sys-arch/01`](sys-arch/01-protocol-extension-system-architecture.md), [`sys-arch/22`](sys-arch/22-wasm-compatible-components-architecture.md) · Crates: [`siar-protocol-ext`](crates/siar-protocol-ext), [`siar-capability`](crates/siar-capability)*

Instead of a single monolithic protocol enum, SIAR uses a **versioned capability architecture**:

```
Communication Session
    │
    ├── Core Control Protocol (always present)
    │
    └── Extension Negotiation (per-peer capability set)
          ├── messaging/1
          ├── files/1
          ├── dtn/1
          ├── emergency/1
          ├── calls/1
          └── custom-app/1  (third-party or ERP extensions)
```

Two-phase capability confirmation (`NegotiationHash` + `HandshakeNonce`) ensures peers agree on extension support before sending extension frames. `FairScheduler` and `BoundedQueue` in `siar-protocol-ext` enforce weighted-fair scheduling and backpressure across all extensions — a slow file transfer cannot starve an emergency SOS.

WASM-compatible components ([`sys-arch/22`](sys-arch/22-wasm-compatible-components-architecture.md)) enable portable execution of selected logic (custom protocol handlers, routing filters, ERP integrations) across desktop, server, embedded Linux, and browser-compatible environments — while the native Rust core retains ownership of networking, hardware codecs, secure key stores, and daemon lifecycle.

---

## Anonymity Transport Plane

*Specification: [`sys-arch/34`](sys-arch/34-mixnet-loopix-sphinx-nym-high-anonymity-transport-architecture.md)*

SIAR's existing transports optimize for reachability, latency, bandwidth, reliability, and offline operation. They do not, by themselves, provide strong resistance against network metadata analysis — a network observer can still infer communication relationships from timing and volume, even when message contents are E2EE-protected.

Part 34 introduces an independent **High-Anonymity Transport Plane** based on mix-network principles (Loopix/Sphinx/Nym):

- **Sphinx packet encapsulation** — layered onion routing with per-hop key agreement
- **Randomized delays and cover traffic** — timing analysis resistance
- **Anonymous mailboxes** — offline reception and unlinkable reply capability
- **Strict downgrade prevention** — explicit anonymity policy, not a best-effort side effect

The anonymity plane is an additive routing class integrated with SIAR's existing policy engine — it does not replace or weaken direct, mesh, DTN, or realtime transport.

---

## Workspace Crate Map (33 Crates)

SIAR is a modular Rust cargo workspace comprising **33 domain crates**, **4 application binaries**, Android JNI runtime bridges, and fuzz testing targets:

```text
siar/
├── apps/
│   ├── android/                          # Android Native App & JNI Bridges
│   │   ├── app/                          # Jetpack Compose UI (Chat, Groups, Settings, Radar)
│   │   ├── messaging-jni/                # Rust JNI cdylib surface (siar-android-messaging)
│   │   ├── rust-jni-glue/                # Shared JNI glue, memory safety, and JVM callbacks
│   │   └── build-native.sh               # Multi-ABI cargo-ndk build automation script
│   ├── cli/                              # Interactive Terminal Messenger & Diagnostics Node
│   ├── desktop/                          # Desktop GUI Application (Dioxus 0.7 Desktop UI)
│   └── emergency-node/                   # Headless Emergency DTN Relay & Booster Daemon
├── crates/
│   ├── [Core Domain, Identity & Cryptography]
│   │   ├── siar-domain/                  # Core entities: AccountId, DeviceId, Ticket, SafetyFingerprint
│   │   ├── siar-crypto/                  # Ed25519, X25519, ChaCha20-Poly1305, zeroize primitives
│   │   ├── siar-crypto-mls/              # IETF MLS (RFC 9420) 1:1 and group E2EE engine
│   │   └── siar-identity-multidevice/    # Multi-device authority, device certs, trust store, SAS pairing
│   ├── [Protocols & Extension Engine]
│   │   ├── siar-protocol/                # Wire envelopes, Postcard binary codec, frame types
│   │   ├── siar-protocol-ext/            # Extensible protocol engine: FairScheduler, BoundedQueue, health
│   │   └── siar-capability/              # Two-phase capability negotiation & codec matrices
│   ├── [Mesh Routing, Policy & Connectivity]
│   │   ├── siar-routing/                 # PathTable, link health scoring, latency metrics, classification
│   │   ├── siar-routing-policy/          # Multi-metric candidate scoring, hysteresis, decide_route
│   │   └── siar-connectivity/            # Cross-transport state engine & dynamic link probes
│   ├── [DTN, Emergency Priority & Scheduling]
│   │   ├── siar-dtn/                     # Opportunistic DTN store-carry-forward buffer & anti-entropy
│   │   ├── siar-dtn-bundle/              # Bundle framing & Spray-and-Wait forwarding strategies
│   │   └── siar-emergency/               # Priority class queuing (P0–P3) & battery override
│   ├── [Storage, Blobs & Reliability]
│   │   ├── siar-storage/                 # Pure-Rust Stoolap embedded SQL (Messages, Contacts, Outbox)
│   │   ├── siar-event-log/               # Append-only offline event log & causal gap detection
│   │   ├── siar-blob-manifest/           # BLAKE3 Merkle DAG blob chunking & AEAD encryption
│   │   ├── siar-resource-limits/         # Backpressure engine, token buckets & queue drop policies
│   │   └── siar-crash-recovery/          # WAL recovery, transactional checkpoints & corrupt state isolation
│   ├── [Messaging Orchestration & UI State]
│   │   ├── siar-messaging/               # MessageService, GroupService, Ticket manager, multi-node tests
│   │   └── siar-ui-state/                # Framework-agnostic UI state machines & Security Center
│   ├── [Realtime Media & Hardware Codecs]
│   │   ├── siar-media-core/              # Media traits, raw video/audio buffers, sample clocks
│   │   ├── siar-media-audio/             # Desktop Opus codec + AEC/NS/AGC DSP pipeline
│   │   ├── siar-media-av1/               # Desktop dav1d AV1 video decoder with lookahead decoding
│   │   ├── siar-media-android/           # Android MediaCodec hardware surface zero-copy pipeline
│   │   ├── siar-media-image/             # Image processing, format transcoding & responsive thumbnails
│   │   └── siar-calls/                   # Realtime P2P media call session protocols & signaling
│   ├── [Multi-Transport Physical Sockets]
│   │   ├── siar-transport/               # Transport manager, pooled socket multiplexer & lifecycle
│   │   ├── siar-transport-ble/           # Linux/cross-platform Bluetooth Low Energy transport
│   │   ├── siar-transport-ble-android/   # Android native Bluetooth Low Energy transport driver
│   │   ├── siar-transport-bluetooth-classic/ # High-throughput RFCOMM Bluetooth Classic transport
│   │   ├── siar-transport-wifi-direct/   # High-bandwidth Wi-Fi Direct P2P ad-hoc transport
│   │   └── siar-transport-wifi-aware/    # Wi-Fi Aware (NAN — Neighbor Awareness Networking) transport
│   └── [Simulation & Test Harness]
│       └── siar-testkit/                 # In-memory virtual radio mesh simulator & link impairments
├── platform/
│   └── android/                          # Android native platform bindings & permission harnesses
└── fuzz/                                 # Cargo Fuzz targets (frame & blob decoders)
```

---

## Deployment Modes

SIAR deploys in three distinct forms:

### 1. Standalone User Applications

| App | Platform | UI Framework |
| :--- | :--- | :--- |
| `apps/android` | Android (minSdk 26) | Jetpack Compose + JNI bridge to Rust core |
| `apps/desktop` | Linux, Windows | Dioxus 0.7 Desktop (WebView2/GTK) |
| `apps/cli` | Any POSIX | Interactive terminal — identity, tickets, DTN |

### 2. Headless Off-Grid Repeater Daemon

`apps/emergency-node` runs headless on any Linux device:
- Raspberry Pi, OpenWrt routers, solar-powered field nodes
- Autonomous store-carry-forward DTN mesh repeater
- No GUI required; daemon owns all networking and storage state

### 3. Embedded / Self-Hosted Relay Infrastructure

The headless runtime (`sys-arch/16`) and relay architecture (`sys-arch/11`) support:
- Organization-private relay fleets and regional clusters
- DTN gateway bridges between Internet and off-grid mesh segments
- Multi-cloud / bare-metal deployment with no product-level relay lock-in

---

## SIAR vs Traditional Messengers

| Feature | SIAR | Signal / WhatsApp / Telegram |
| :--- | :--- | :--- |
| **Server Requirement** | **None** — fully autonomous P2P / mesh | Mandatory central cloud servers |
| **Offline / Disaster Operation** | **Native** — BLE, Wi-Fi Direct/Aware, DTN | Unusable without Internet |
| **Addressing & Identity** | Self-sovereign Peer Tickets / Ed25519 root keys | Phone numbers / cloud user IDs |
| **Transport Layer** | Multi-transport adaptive path routing (8+ physical paths) | HTTPS / WebSockets / TCP |
| **Group Security** | OpenMLS (RFC 9420) forward secrecy + post-compromise | Custom Signal Protocol / server-managed |
| **Hardware Codec Acceleration** | Native Android `MediaCodec` + pure-Rust DSP | WebRTC / platform C-libs |
| **Anonymity** | Mixnet / Sphinx transport plane (Part 34) | None / VPN-dependent |
| **Self-Hosted Relay** | Full support — no lock-in | Not supported |
| **Extensibility** | Versioned protocol extensions + WASM sandbox | Closed monolithic protocol |

---

## Implementation Status & Roadmap

SIAR maintains an honest, compile-and-test-verified implementation tracking matrix per [`ROADMAP.md`](ROADMAP.md).
All coverage figures are from the `sys-arch/` specification corpus (33 numbered core-architecture docs + 27 `ui-ux-NN` UI specs).

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────┐
│                              SIAR IMPLEMENTATION MATURITY OVERVIEW                               │
│                              (as of 2026-09-17 · rustc 1.91 verified)                           │
├───────────────────────────────┬──────────────────────────────┬──────────────────────────────────┤
│ Architectural Layer           │ Spec Coverage                │ Verified State                   │
├───────────────────────────────┼──────────────────────────────┼──────────────────────────────────┤
│ Tier 0 — Foundational Core    │                              │                                  │
│                               │                              │                                  │
│  01 · siar-protocol-ext       │ ✅ 108/108 — SPEC COMPLETE   │ FairScheduler, BoundedQueue,     │
│                               │   115 tests, 0 warnings      │ extension health & DoD self-audit│
│  02 · siar-identity-multidevice│ ✅ 204/204 — SPEC COMPLETE  │ Root authority, DeviceCert,      │
│                               │   251 tests, 0 warnings      │ TrustStore, algorithm agility,   │
│                               │                              │ SAS pairing, identity lifecycle  │
│  03 · siar-routing-policy     │ ✅ 200/200 — SPEC COMPLETE   │ Multi-metric scoring, RoutePlan, │
│                               │   263 tests, 0 warnings      │ hysteresis, decide_route,        │
│                               │                              │ multidevice routing, 19 rounds   │
│  04 · siar-event-log          │ 🟡 ~10/95  (~11%)            │ Monotonic sequence log           │
│  05 · siar-blob-manifest      │ 🟡 ~23/210 (~11%)            │ BLAKE3 Merkle DAG, AEAD encrypt  │
│  06 · siar-dtn-bundle         │ 🟡 ~50/192 (~26%)            │ Spray-and-Wait, bundle framing   │
│  07 · siar-capability         │ 🟡 ~19/164 (~12%)            │ 2-phase capability negotiation   │
│  08 · siar-resource-limits    │ 🟡 ~56/193 (~29%)            │ Token-bucket backpressure        │
│  09 · siar-crash-recovery     │ 🟡 ~15/186  (~8%)            │ WAL recovery, checkpoint isolator│
├───────────────────────────────┼──────────────────────────────┼──────────────────────────────────┤
│ Tier 1 — Security Backbone    │                              │                                  │
│  28 · siar-crypto / mls       │ 🟡 ~46/127 (~36%)            │ Ed25519/X25519, MLS E2EE,        │
│                               │                              │ replay protection, revocation    │
├───────────────────────────────┼──────────────────────────────┼──────────────────────────────────┤
│ Tier 2 — UI/UX (27 specs)     │                              │                                  │
│  ui-ux-15 · Security Center   │ 🟡 ~183/221 (~83%)           │ RecoveryScope, RevocationCaps,   │
│                               │                              │ CompromiseResponse, desktop UI   │
│  ui-ux-01..14, 16..27         │ ⚪ Not yet reconciled        │ Pre-existing UI code in          │
│                               │                              │ apps/desktop & apps/android      │
├───────────────────────────────┼──────────────────────────────┼──────────────────────────────────┤
│ Multi-Transport Layer         │ ✅ Production Hardened        │ Pooled stream multiplexer,       │
│                               │                              │ multi-node end-to-end test suite │
└───────────────────────────────┴──────────────────────────────┴──────────────────────────────────┘
```

> **Tier 0 progress** — **3 of 9 foundational core specs are spec-complete** (01, 02, 03).
> Specs 04–09 are in progress; each has a real crate with real code — gaps are depth, not breadth.
> See [`ROADMAP.md`](ROADMAP.md) for section-by-section coverage notes and the next-priority ordering.

**Quality discipline applied to every spec round:**
- Compile + test + `cargo clippy -D warnings` + `cargo fmt` + doc-warning-free — verified clean before a round is marked done
- Zero regressions in any dependent crate at any point across all 19 rounds of spec work
- Real bugs caught by tests (not review): pooled-connection reuse silently dropping messages after the first; expired operation never enforced at routing-decision level; DTN `file_chunk()` constructor contradicting its own spec example; stickiness test using a stale pre-mutation health snapshot

**Three open reconciliation questions** (documented in the relevant crates' own `lib.rs`):
- Two device-cert models (`siar_crypto::device_cert` vs `siar-identity-multidevice`)
- Two routing/scoring systems (`siar-routing` vs `siar-routing-policy`)
- Two DTN bundle models (`siar-dtn` vs `siar-dtn-bundle`)

---

## Prerequisites & Environment Setup

### 1. Nix Development Environment (Recommended)

SIAR provides a universal, cross-distribution Nix installer ([`install-nix.sh`](install-nix.sh)) that automates Nix setup across **any Linux distribution** (Arch, Ubuntu, Debian, Fedora, RHEL, openSUSE, Alpine, Void, etc.) and macOS:

```bash
# Automated cross-distribution Nix installer
./install-nix.sh

# Or remotely via curl:
curl --proto '=https' --tlsv1.2 -sSfL \
  https://raw.githubusercontent.com/irshadali5/siar/develop/scripts/install-nix.sh | bash
```

> See the full [Nix Installation & Configuration Guide](docs/nix-installation-guide.md) for distro-specific options, diagnostics (`--doctor`), and uninstallation (`--uninstall`).

Once installed, the hermetic Nix Flake ([`flake.nix`](flake.nix), [`shell.nix`](shell.nix)) provides Rust 1.91, GTK3, WebKit2GTK, ALSA, OpenSSL, CMake, libxdo, and Darwin SDK frameworks automatically:

```bash
# Enter the fully provisioned hermetic development shell
nix develop

# Build workspace binaries directly with Nix
nix build .#siar-cli
nix build .#siar-desktop
nix build .#siar-emergency-node

# Run Nix checks and flake validation
nix flake check
```

If you use `direnv`:
```bash
direnv allow
```

### 2. Manual Rust & Platform Setup

1. **Rust** — 1.91.0 or newer:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup target add x86_64-unknown-linux-gnu aarch64-linux-android \
     armv7-linux-androideabi i686-linux-android x86_64-linux-android
   ```

2. **Android Development** (for `apps/android`):
   - Android SDK API Level 34+, NDK `r25b` or newer
   - `cargo install cargo-ndk`
   - Set `ANDROID_HOME` and `ANDROID_NDK_HOME` environment variables

3. **System dependencies (Linux GUI)**:
   ```bash
   sudo apt-get install -y build-essential pkg-config libssl-dev \
     libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libasound2-dev \
     libjavascriptcoregtk-4.1-dev cmake libopus-dev libdav1d-dev
   ```

---

## Build & Compilation Tutorial

### 1. Building the Rust Workspace

```bash
# Check compilation across all workspace crates
cargo check --workspace

# Build all binaries in debug mode
cargo build --workspace

# Build optimized release binaries
cargo build --workspace --release
```

Compiled binaries land in `target/release/`:
- `target/release/siar-cli`
- `target/release/siar-desktop`
- `target/release/siar-emergency-node`

### 2. Cross-Compiling Android Native (.so) Libraries

```bash
cd apps/android
./build-native.sh
```

This invokes `cargo ndk` for all Android-relevant workspace crates and places `.so` files in:
`apps/android/app/src/main/jniLibs/<abi>/`

### 3. Building the Android Application

```bash
cd apps/android
./gradlew assembleDebug      # Build debug APK
./gradlew installDebug       # Install on connected device/emulator
```

---

## Node Configuration & User Guide

### 1. Command-Line Interface (`siar-cli`)

`siar-cli` provides an interactive terminal interface for managing identities, tickets, direct messaging, and anonymous token mailboxes.

```bash
siar-cli   # auto-initializes persistent identity under OS data directory
```

```text
SIAR Interactive Messenger
--------------------------
1. Show My Peer Ticket
2. Add Contact Peer Ticket
3. Send 1:1 Direct Text
4. Send Anonymous Mailbox Message
5. Check Token Mailbox
6. Exit
```

1. **Show My Peer Ticket** — displays your Base64-encoded `PeerTicket`; share out-of-band via QR code or text.
2. **Add Contact** — paste a contact's `PeerTicket`; the CLI decodes and resolves the peer's public key and endpoint address.
3. **Send Direct Text** — checks `PathTable` for active routes (`LocalLan`, `InternetDirect`, BLE) and dispatches through the routing policy engine.
4. **Send Anonymous Mailbox Message** — delivers to a relay node via an unlinkable single-use token mailbox path.

### 2. Desktop Application (`siar-desktop`)

```bash
cargo run --bin siar-desktop
```

Features:
- **Contact Roster** — peer tickets, online reachability status, link health indicators
- **MLS Group Conversations** — 1:1 and multi-member MLS groups with forward secrecy
- **Security Center** — device key management, revocation, recovery codes, compromise response
- **Media Attachments** — drag-and-drop file sharing with automatic BLAKE3 Merkle DAG chunking

### 3. Headless Emergency Relay Node (`siar-emergency-node`)

```bash
cargo run --bin siar-emergency-node
```

Behavior:
- Listens on all local network interfaces and available Bluetooth/BLE adapters
- Maintains in-memory & on-disk DTN store-carry-forward queue for offline messages
- Periodically probes nearby nodes, updates link quality metrics (`rtt_millis`, `reliability`), and flushes pending queues when a route becomes available

### 4. Android Messenger App (`apps/android`)

Grant permissions on launch:
- **Bluetooth & Nearby Devices** — `BLUETOOTH_SCAN`, `BLUETOOTH_CONNECT`, `NEARBY_WIFI_DEVICES`
- **Location** — required on Android 12 and below for physical proximity discovery (`ACCESS_FINE_LOCATION`)

Features:
- **Chats Tab** — 1:1 conversations, contact import via Peer Tickets, text + media attachments
- **Groups Tab** — MLS group creation, group invites, multi-device encrypted threads

---

## Testing & Fuzzing

### Unit & Integration Tests

```bash
# Full workspace test suite
cargo test --workspace

# End-to-end multi-node integration suite
cargo test -p siar-messaging --test end_to_end

# Mesh network simulation (siar-testkit virtual radio)
cargo test -p siar-testkit
```

### Nix Automated Checks

```bash
nix flake check
```

### Fuzzing

```bash
cargo install cargo-fuzz

# Fuzz wire frame decoder
cargo fuzz run decode_frame

# Fuzz blob frame decoder
cargo fuzz run decode_blob_frame
```

---

## License & Dual-Tier Model

SIAR employs a two-tier open-source licensing model designed for maximum library adoption while protecting user-facing standalone applications:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                            SIAR LICENSING MODEL                             │
├──────────────────────────────────────┬──────────────────────────────────────┤
│    Core Libraries (crates/*)         │    Standalone Apps (apps/*)          │
│    - siar-crypto / siar-crypto-mls   │    - apps/android (Jetpack Compose)  │
│    - siar-transport / siar-routing   │    - apps/desktop (Dioxus GUI)       │
│    - siar-storage / siar-messaging   │    - apps/cli (Terminal Node)        │
│    - siar-dtn / siar-protocol        │    - apps/emergency-node (Daemon)    │
│                                      │                                      │
│    📜 MIT License OR Apache-2.0      │    📜 GNU AGPLv3 / Commercial        │
│    (Permissive Open Source)          │    (Copyleft & Enterprise Exemption) │
└──────────────────────────────────────┴──────────────────────────────────────┘
```

### 1. Permissive Core Libraries (`crates/*`): MIT OR Apache-2.0

All underlying Rust crates and protocol engines are dual-licensed under **[MIT](LICENSE-MIT)** OR **[Apache-2.0](LICENSE-APACHE)**.
You may freely embed, link (statically or dynamically), and build proprietary or open-source applications using these crates without any commercial subscription or copyleft obligations.

### 2. Standalone Applications & Daemons (`apps/*`): GNU AGPLv3

All end-user client applications (`apps/android`, `apps/desktop`, `apps/cli`) and headless daemons (`apps/emergency-node`) are licensed under **[GNU AGPLv3](LICENSE-AGPLv3)**.
- **100% Free for Everyone** — anyone can use, inspect, modify, and self-host.
- **Copyleft on Modifications** — if you modify and distribute or run these applications over a network, you must release your modifications under AGPLv3.

### 3. Commercial Exemption (`SIAR-CEEL-1.0`)

For commercial enterprises that wish to rebrand, white-label, or host modified closed-source versions without AGPLv3 obligations:
- Full legal terms and commercial subscription tiers: [`LICENSE-COMMERCIAL.md`](LICENSE-COMMERCIAL.md)
- Contact: `licensing@siar.network`

---

### Contributing

1. Read **[CONTRIBUTING.md](CONTRIBUTING.md)** and **[CLA.md](CLA.md)** before opening a Pull Request.
2. All contributions are governed by the CLA, granting the SIAR maintainers dual-licensing rights.
3. Maintain zero-warning clean compilation across all targets (`cargo check --workspace`, `cargo clippy -D warnings --workspace`).
4. Preserve strict boundary isolation — core crates under `crates/` must remain pure-Rust without mandatory C-library linkages.
5. Platform-specific hardware integration must be strictly isolated in dedicated crates (`siar-media-android`, `siar-transport-ble-android`).

---

*Built with Rust, Kotlin, and OpenMLS by the SIAR Open Source Engineering Team.*
