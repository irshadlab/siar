# SIAR: System Architecture Master Rationale & Engineering Synthesis

> **Authoritative Companion to [`sys-arch/`](sys-arch/)**  
> **Complements:** [`spec-order.md`](spec-order.md), [`ROADMAP.md`](ROADMAP.md), [`README.md`](README.md), and [`SYSTEM_COMPARISON_AND_EVALUATION.md`](SYSTEM_COMPARISON_AND_EVALUATION.md).  
> **Corpus Scope:** All 176 Architecture Specifications (Parts 01–33, UI-UX 01–27, and Parts 34–150; 514,448 lines, 875,254 words).

---

## Table of Contents

- [PART 1: The Architectural Discourse \& Comprehensive Synthesis](#part-1-the-architectural-discourse--comprehensive-synthesis)
  - [1.1 The Anatomy of `sys-arch/`: The 3 Tiers](#11-the-anatomy-of-sys-arch-the-3-tiers)
  - [1.2 What Specs 34–150 Mean \& Why They Span 116 Files](#12-what-specs-34150-mean--why-they-span-116-files)
  - [1.3 The 8 Layers of the Anonymous Network (Parts 34–150)](#13-the-8-layers-of-the-anonymous-network-parts-34150)
  - [1.4 The Server Database Architecture: PostgreSQL vs. Pure-Rust Sovereign Stack](#14-the-server-database-architecture-postgresql-vs-pure-rust-sovereign-stack)
  - [1.5 The Engineering Crucible: Why SIAR is "Notorious Hell" to Develop](#15-the-engineering-crucible-why-siar-is-notorious-hell-to-develop)
  - [1.6 Implementation Feasibility: Concrete Product vs. North Star Architecture](#16-implementation-feasibility-concrete-product-vs-north-star-architecture)
  - [1.7 User Superpowers Unlocked at Every Development Stage](#17-user-superpowers-unlocked-at-every-development-stage)
- [PART 2: Design Rationale for Every System Architecture Choice in SIAR](#part-2-design-rationale-for-every-system-architecture-choice-in-siar)
  - [2.1 Protocol Framing \& Codecs (Spec 01)](#21-protocol-framing--codecs-spec-01)
  - [2.2 Identity, Authority \& Multi-Device Keys (Specs 02 \& 15)](#22-identity-authority--multi-device-keys-specs-02--15)
  - [2.3 Dynamic Routing Policy Engine \& Multipath Bonding (Specs 03 \& 12)](#23-dynamic-routing-policy-engine--multipath-bonding-specs-03--12)
  - [2.4 Crash-Resilient Event Log, Outbox \& Stoolap Storage (Specs 04 \& 09)](#24-crash-resilient-event-log-outbox--stoolap-storage-specs-04--09)
  - [2.5 Content-Addressed BLAKE3 Merkle-DAG Blob Subsystem (Spec 05)](#25-content-addressed-blake3-merkle-dag-blob-subsystem-spec-05)
  - [2.6 Delay-Tolerant Networking (DTN) \& Epidemic Forwarding (Spec 06)](#26-delay-tolerant-networking-dtn--epidemic-forwarding-spec-06)
  - [2.7 Capability Negotiation \& Graceful Protocol Evolution (Spec 07)](#27-capability-negotiation--graceful-protocol-evolution-spec-07)
  - [2.8 Backpressure, Token Buckets \& Emergency QoS (Specs 08 \& 17)](#28-backpressure-token-buckets--emergency-qos-specs-08--17)
  - [2.9 Battery-Aware Radio Scheduling \& Proximity Discovery (Specs 13 \& 14)](#29-battery-aware-radio-scheduling--proximity-discovery-specs-13--14)
  - [2.10 Headless Daemons, Embedded Routers \& C-ABI FFI (Specs 16, 19, 20)](#210-headless-daemons-embedded-routers--c-abi-ffi-specs-16-19-20)
  - [2.11 Pure-Rust DSP \& Native Hardware Media Surfaces (Specs 25, 26, 29)](#211-pure-rust-dsp--native-hardware-media-surfaces-specs-25-26-29)
  - [2.12 Cryptographic Backbone: MLS \& Post-Compromise Security (Spec 28)](#212-cryptographic-backbone-mls--post-compromise-security-spec-28)
  - [2.13 High-Anonymity Mixnet: Loopix, Sphinx \& Cover Traffic (Specs 34–42)](#213-high-anonymity-mixnet-loopix-sphinx--cover-traffic-specs-3442)
  - [2.14 Zero-Trust Infrastructure: Measured Boot, HSMs \& Erasure (Specs 67, 71–74)](#214-zero-trust-infrastructure-measured-boot-hsms--erasure-specs-67-7174)
  - [2.15 Anti-Surveillance Services: PIR Search \& Differential Privacy (Specs 91–92)](#215-anti-surveillance-services-pir-search--differential-privacy-specs-9192)
  - [2.16 SRE, Physical Tamper Protection \& Incident Command (Specs 109–121)](#216-sre-physical-tamper-protection--incident-command-specs-109121)
  - [2.17 Engineering Traceability \& Release Evidence Archive (Specs 122–126)](#217-engineering-traceability--release-evidence-archive-specs-122126)
  - [2.18 Sandboxed WASM Plugins \& Information Flow Control (Specs 134–146)](#218-sandboxed-wasm-plugins--information-flow-control-specs-134146)
  - [2.19 Cross-Platform UI Architecture: Dioxus, Compose \& Shared State Machines](#219-cross-platform-ui-architecture-dioxus-compose--shared-state-machines)

---

# PART 1: The Architectural Discourse & Comprehensive Synthesis

## 1.1 The Anatomy of `sys-arch/`: The 3 Tiers

The [`sys-arch/`](sys-arch/) directory contains **176 specification documents** totaling **514,448 lines** and **875,254 words**. It is structured into three distinct tiers:

```text
sys-arch/ (176 Specifications · 514k+ lines)
│
├── 1. Core Mesh & Local-First Engine (Parts 01 – 33)
│      105,732 lines · 33 Specs
│      Execution: Sequenced per spec-order.md (Milestones 1, 2, 5, 6, 7).
│      Focus: P2P wire framing, multi-device MLS keys, dynamic routing policy,
│      resumable BLAKE3 blobs, DTN data mules, emergency QoS, lock-free audio DSP.
│
├── 2. UI/UX & Cross-Platform Shells (Parts ui-ux-01 – ui-ux-27)
│      72,610 lines · 27 Specs
│      Execution: Parallel vertical slices with backend engines (Milestones 3, 4, 5).
│      Focus: Dioxus 0.7 desktop shells, Android Compose shells, design tokens,
│      virtualized message lists, composer, call surfaces, pairing, security center.
│
└── 3. The Anonymous Network & Cloud Operating Ecosystem (Parts 34 – 150)
       336,106 lines · 116 Specs (Parts 34–138, 140–150; 139 reserved)
       Focus: Global Loopix mixnet, Sphinx onion packetization, zero-trust cloud servers,
       TPM measured boot, physical datacenter tamper detection, PIR search, WASM sandboxes.
```

---

## 1.2 What Specs 34–150 Mean & Why They Span 116 Files

In Parts 01–33, SIAR solves **local-first survivability**: how two or more devices communicate without internet, servers, or power grids using BLE, Wi-Fi Direct, and DTN mules.

### The Fatal Flaw of Pure End-to-End Encryption (E2EE)
When traffic leaves local ad-hoc radios and traverses the public Internet, **encryption alone fails to protect privacy**:
* **E2EE protects *what* is said (payload).**
* **E2EE does *not* protect *who* speaks to whom, *when* they speak, *how long* they speak, or *how much data* flows (metadata).**

A network-level passive observer (an ISP, autonomous system, or intelligence service) monitoring encrypted TLS/QUIC connections can reconstruct an entire organizational or social graph through **packet timing, packet size, and traffic volume analysis**.

**Part 34 ([`sys-arch/34`](sys-arch/34-mixnet-loopix-sphinx-nym-high-anonymity-transport-architecture.md))** introduces the governing axiom of the upper architecture:
> *"SIAR must treat anonymity as an explicit routing and security property, not as a side effect of encryption or relaying."*

### Why It Takes Over 100 Files
In security engineering, **anonymity is a whole-stack property**. If an adversary cannot break your packet encryption, they will correlate your packet sizes. If they cannot correlate packet sizes, they will analyze timing. If timing is masked, they will inspect database access patterns, seize unencrypted server disks, or attack third-party plugins. 

Building a truly metadata-private network requires re-engineering every computing layer from packet fragmentation to physical datacenter locks.

---

## 1.3 The 8 Layers of the Anonymous Network (Parts 34–150)

```text
┌───────────────────────────────────────────────────────────────────────────────────────────────────────┐
│                                 THE 8 ANONYMOUS NETWORK LAYERS (34–150)                               │
├────────────────────────────────┬─────────────────┬────────────────────────────────────────────────────┤
│ Architectural Layer            │ Specs           │ Core Focus & Systems Engineered                    │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 1. Core Mixnet Mechanics       │ Parts 34 – 42   │ Sphinx onion packetization, Loopix Poisson delays, │
│                                │                 │ cover traffic, Sybil-proof directories, bridges.   │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 2. Anonymous App Primitives    │ Parts 43 – 49   │ Metadata-private groups, anonymous VoIP/video      │
│                                │                 │ signaling, private presence, blind reputation.     │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 3. Sovereign Protocol Policies │ Parts 50 – 70   │ Cross-border data sovereignty, multi-authority     │
│                                │                 │ governance, anti-enumeration naming, agility.      │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 4. Zero-Trust Infrastructure   │ Parts 71 – 81   │ TPM measured boot, HSM key custody, SBOM lineage,  │
│                                │                 │ Raft consensus, secretless runtimes, mTLS.         │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 5. Private Cloud Services      │ Parts 82 – 94   │ Passkey auth, private contact graphs, PIR search,  │
│                                │                 │ Differential Privacy metrics, audit logs.          │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 6. Defense & SecOps            │ Parts 95 – 100  │ Continuous control compliance, privacy-safe SOC/IR,│
│                                │                 │ threat intelligence, rollback protection.          │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 7. SRE, Fleet & Physical Site  │ Parts 101 – 121 │ Fair load-shedding, FinOps, SRE error budgets/SLOs,│
│                                │                 │ rack tamper detection, disaster evacuation.        │
├────────────────────────────────┼─────────────────┼────────────────────────────────────────────────────┤
│ 8. Governance, SDK & Plugins   │ Parts 122 – 150 │ Requirements knowledge graph, SDKs, sandboxed WASM │
│                                │                 │ plugins, Information Flow Control, marketplace.    │
└────────────────────────────────┴─────────────────┴────────────────────────────────────────────────────┘
```

---

## 1.4 The Server Database Architecture: PostgreSQL vs. Pure-Rust Sovereign Stack

In [`sys-arch/74`](sys-arch/74-anonymous-network-database-persistent-state-transaction-boundaries-schema-evolution-storage-engine-architecture.md), SIAR establishes a **"No One-Database Dogma"** (§22). Persistence is decoupled across 4 storage classes: SQL, Key-Value, Object Storage, and Append-Only Log.

### The Standard Reference Baseline (Cloud/Enterprise)
* **Control-Plane Database: PostgreSQL** ([`sys-arch/74`](sys-arch/74-anonymous-network-database-persistent-state-transaction-boundaries-schema-evolution-storage-engine-architecture.md) §24, [`sys-arch/62`](sys-arch/62-anonymous-network-deployment-packaging-infrastructure-provisioning-bare-metal-vm-container-runtime-reproducible-operations-architecture.md) §314). Access via `SQLx` or `Diesel` behind a `Repository<T>` trait. Stores mixnet topology, node credentials, tenant configuration, and billing credits. **No plaintext user content is ever stored in the database.**
* **Bulk Media: S3-Compatible Object Store** (MinIO, Ceph, AWS S3).
* **Anti-Replay / Nonce Cache: Redis**.
* **Event Bus: Apache Kafka / NATS JetStream**.

### The 100% Pure-Rust Sovereign Alternative Stack
SIAR domain crates never import concrete database drivers. An operator running an autonomous, sovereign relay node can deploy an entirely pure-Rust stack:

```text
┌────────────────────────┬─────────────────────────┬────────────────────────────────────────────────────┐
│ Subsystem              │ Cloud Baseline in Specs │ Pure-Rust Sovereign Alternative                    │
├────────────────────────┼─────────────────────────┼────────────────────────────────────────────────────┤
│ Server Relational DB   │ PostgreSQL              │ Stoolap (standalone) or Rust-embedded RDBMS        │
│ Mailbox Ciphertext     │ SQL Partition / Redis   │ redb or fjall (Pure-Rust embedded ACID KV)         │
│ Large Media & Backups  │ AWS S3 / MinIO          │ Garage (Pure-Rust S3) or native BLAKE3 CAS         │
│ Anti-Replay Nonces     │ Redis Cluster           │ Pure-Rust Cuckoo Filters + DashMap / redb          │
│ Message Bus / PubSub   │ Apache Kafka            │ Fluvio / Iggy.rs or Tokio Broadcast Channels       │
└────────────────────────┴─────────────────────────┴────────────────────────────────────────────────────┘
```

* **`redb` / `fjall` for Mailbox Relays**: Mailboxes store blind ciphertext with short TTLs. Embedded pure-Rust key-value stores process tens of thousands of writes per second with zero external database processes.
* **`Garage` for Object Storage**: A pure-Rust, lightweight distributed object store designed for self-hosting and geo-distributed clusters.
* **Pure-Rust In-Memory Cuckoo Filters for Nonces**: Checking if a 32-byte packet nonce was seen in the last 15-minute window takes single-digit nanoseconds in memory without a network round-trip to Redis.
* **`Fluvio` / `Iggy.rs` for Streaming**: High-throughput distributed streaming brokers written 100% in Rust, eliminating JVM dependencies.

---

## 1.5 The Engineering Crucible: Why SIAR is "Notorious Hell" to Develop

Developing SIAR feels grueling because it rejects the compromises that typical communication apps rely on:

1. **The "Zero Infrastructure" Tax**: No DNS, no NTP, no APNs/FCM push servers, and no central STUN/TURN relays. Every foundational service must be implemented in Rust.
2. **Peer-to-Peer MLS Without a Delivery Service**: Implementing IETF MLS (RFC 9420) where devices advance ratchets offline and merge concurrent tree forks across ad-hoc Bluetooth links without a central sequencer.
3. **Hostile Mobile Operating Systems**: Bypassing Android Doze mode background socket limits, mitigating chip antenna contention between BLE and Wi-Fi Direct, and maintaining zero-copy memory safety across Kotlin/JNI boundaries.
4. **Pure-Rust DSP Under Lock-Free Deadlines**: Writing acoustic echo cancellation (AEC), noise suppression (NS), and sample-rate resamplers in pure Rust with sub-10ms frame latencies, where a single memory allocation causes audio dropouts.
5. **The Specification Invariant Burden**: Fulfilling 12,893+ numbered sections across 176 architecture documents with zero compiler warnings and strict crash-recovery validation.

---

## 1.6 Implementation Feasibility: Concrete Product vs. North Star Architecture

* **The Concrete Product (Specs 01–33 + UI-UX 01–27 = 60 Specs)**:  
  **Fully feasible, actionable, and substantially implemented.** 29 workspace crates already exist. Milestone 1 (Tier 0 Headless Core) is nearing full completion, with Specs 01, 02, and 03 completely closed and passing hundreds of automated tests.
* **The Extended Network Ecosystem (Specs 34–150 = 116 Specs)**:  
  This is a **North Star Architecture**. It outlines how a complete sovereign internet ecosystem functions over a multi-year horizon. Having these specifications written ensures that early decisions (packet headers, storage abstractions, capability matrices) never conflict with future requirements like mixnets or WASM sandboxing.

---

## 1.7 User Superpowers Unlocked at Every Development Stage

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        USER SUPERPOWERS UNLOCKED AT EVERY STAGE                        │
├─────────────────────────┬──────────────────────────────────────────────────────────────┤
│ Milestone               │ Concrete Real-World User Capability                          │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 1–2           │ Total Data Durability: Zero loss on battery pull; no phone   │
│ (Core & Identity)       │ numbers or central accounts; multi-device revocation.        │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 3–4           │ Sovereign Daily Messenger: Fluid 120Hz native desktop and    │
│ (App Shells & Chat UI)  │ Android UI running local-first with zero cloud dependencies. │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 5             │ Zero-Lag Calls & Local Swarms: Sub-10ms VoIP; 300MB/s file   │
│ (Media & Multipath)     │ transfers via Wi-Fi Direct; calls survive network switches.  │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 6             │ Blackout Survival: Mesh messaging over BLE/Wi-Fi Direct;     │
│ (DTN & Emergency Mesh)  │ store-carry-forward data mules; life-saving SOS beaconing.   │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 7             │ Metadata Invisibility: Loopix mixnet & Sphinx onion packets  │
│ (Mixnet Anonymity)      │ defeat ISP-level passive traffic analysis and correlation.   │
├─────────────────────────┼──────────────────────────────────────────────────────────────┤
│ Milestone 8             │ Private Cloud & Sandboxed Apps: PIR search, differential     │
│ (Ecosystem & WASM)      │ privacy metrics, and WASM plugins provably unable to spy.    │
└─────────────────────────┴──────────────────────────────────────────────────────────────┘
```

---

# PART 2: Design Rationale for Every System Architecture Choice in SIAR

This section details the architectural rationale for every major design decision across SIAR's specification corpus.

---

### 2.1 Protocol Framing & Codecs (Spec 01)
* **Choice**: **Postcard binary serialization** over Protobuf, JSON, or CBOR.
* **Rationale**: Postcard is a `#[no_std]`-compatible, pure-Rust binary format using variable-length integer encoding (LEB128). It eliminates schema compilation steps (required by Protobuf), produces significantly smaller wire frames than CBOR/JSON, and decodes with zero heap allocations on low-power 4MB embedded routers.
* **Choice**: **Two-phase capability negotiation** (`NegotiationHash` + `HandshakeNonce`).
* **Rationale**: Prevents version mismatch crashes and downgrade attacks. Peers cryptographically confirm supported extensions before sending payload frames.
* **Choice**: **`FairScheduler` & byte-bounded queues** (`siar-protocol-ext`).
* **Rationale**: In multi-service channels, an asynchronous 50MB file transfer could easily starve a low-latency voice call or emergency SOS frame. Weighted fair scheduling ensures starvation-free multiplexing under tight memory limits.

---

### 2.2 Identity, Authority & Multi-Device Keys (Specs 02 & 15)
* **Choice**: **Decoupled Root Key (Ed25519) vs. Device Keys (X25519/Ed25519)**.
* **Rationale**: A user's identity must outlive their hardware. The root key acts as a cold master authority (kept offline or in secure hardware), issuing cryptographic `DeviceCert` certificates to individual phones or laptops.
* **Choice**: **Out-of-band SAS (Short Authentication String) Pairing via QR/NFC**.
* **Rationale**: Eliminates Man-In-The-Middle (MITM) vulnerabilities during device enrollment without relying on a centralized public key infrastructure (PKI) or cloud directory.
* **Choice**: **Autonomous Device Revocation**.
* **Rationale**: Revoking a compromised device broadcasts a signed revocation tombstone across the local mesh and DTN network, immediately isolating the stolen device.

---

### 2.3 Dynamic Routing Policy Engine & Multipath Bonding (Specs 03 & 12)
* **Choice**: **Multi-metric link scoring with hysteresis** over static interface binding.
* **Rationale**: Mobile radios fluctuate constantly. Without hysteresis, route selection flaps rapidly between Wi-Fi and Cellular, causing packet reordering and jitter. Multi-metric scoring balances latency, bandwidth, packet loss, cost, and battery consumption.
* **Choice**: **Active multi-link aggregation & striping**.
* **Rationale**: Large files split chunks across available interfaces (e.g., home Wi-Fi + 5G + Wi-Fi Direct) to maximize throughput.
* **Choice**: **Seamless transport handoff**.
* **Rationale**: Real-time sessions bind to logical session IDs rather than physical IP sockets. Walking out of home Wi-Fi range transitions active calls to cellular data without dropping.

---

### 2.4 Crash-Resilient Event Log, Outbox & Stoolap Storage (Specs 04 & 09)
* **Choice**: **Transactional Outbox & Append-Only Event Log** with causal gap detection.
* **Rationale**: Guarantees that a message is never marked as sent in the UI until it is durably committed to non-volatile storage. If the OS kills the app mid-send, the recovery engine restores un-transmitted tickets upon reboot.
* **Choice**: **Stoolap pure-Rust embedded SQL** over SQLite/rusqlite.
* **Rationale**: Eliminates external C-compiler toolchain dependencies (`gcc`/`clang`/NDK bindings), ensuring hermetic, memory-safe, reproducible builds across Linux, Windows, Android, and embedded targets.

---

### 2.5 Content-Addressed BLAKE3 Merkle-DAG Blob Subsystem (Spec 05)
* **Choice**: **BLAKE3 Merkle-DAG chunking (64KB–1MB chunks)** over raw file transfers.
* **Rationale**: Files are content-addressed by their Merkle root hash. If a 1GB transfer drops at 99%, only the missing leaf chunks are requested upon reconnection.
* **Choice**: **Swarm-assisted peer caching**.
* **Rationale**: In emergency shelters or local offices, if one user downloads a disaster map or video, other nearby devices fetch chunks directly over Wi-Fi Direct without consuming external cellular bandwidth.

---

### 2.6 Delay-Tolerant Networking (DTN) & Epidemic Forwarding (Spec 06)
* **Choice**: **Spray-and-Wait and PRoPHET routing** over single-path forwarding.
* **Rationale**: In disconnected environments, a direct network path between sender and receiver may never exist concurrently. Bounded replication ("Spray-and-Wait") distributes bundle copies to physical walking nodes ("data mules") to ensure eventual delivery while preventing buffer exhaustion.
* **Choice**: **Anti-entropy bloom filters**.
* **Rationale**: When two DTN nodes pass each other on a road, they exchange compact bloom filters of their stored bundle IDs, transferring only bundles that the other node lacks.

---

### 2.7 Capability Negotiation & Graceful Protocol Evolution (Spec 07)
* **Choice**: **Additive protocol extensions** (`messaging/1`, `files/1`, `dtn/1`, `emergency/1`).
* **Rationale**: Prevents protocol ossification. Newer nodes can introduce enhanced codecs or features without breaking compatibility with older legacy repeaters.

---

### 2.8 Backpressure, Token Buckets & Emergency QoS (Specs 08 & 17)
* **Choice**: **Hard priority tiers (P0 Emergency to P3 Bulk Background)**.
* **Rationale**: During a crisis, emergency SOS beacons and location telemetry must preempt routine chat messages and file transfers.
* **Choice**: **Preemptive buffer eviction**.
* **Rationale**: If device memory fills to capacity, low-priority bulk chunks (P3) are evicted to guarantee memory for incoming life-safety packets (P0).

---

### 2.9 Battery-Aware Radio Scheduling & Proximity Discovery (Specs 13 & 14)
* **Choice**: **Duty-cycled radio sleep alignment**.
* **Rationale**: Continuous BLE scanning and Wi-Fi Direct probing depletes mobile batteries rapidly. Radio states cycle between active discovery and low-power sleep based on remaining battery level.
* **Choice**: **Cross-transport proximity abstraction**.
* **Rationale**: The application layer discovers nearby contacts uniformly, regardless of whether discovery occurred via Wi-Fi Aware (NAN), BLE beacons, or local mDNS.

---

### 2.10 Headless Daemons, Embedded Routers & C-ABI FFI (Specs 16, 19, 20)
* **Choice**: **Decoupled headless daemon runtime** (`apps/emergency-node`).
* **Rationale**: Enables SIAR to run on headless Raspberry Pi units, solar-powered field boxes, and OpenWrt routers with zero GUI dependencies.
* **Choice**: **Stable C-ABI export surface** (`crates/siar-ffi`).
* **Rationale**: Allows external languages (C, C++, Go, Python) and mobile runtimes (Kotlin via JNI, Swift via C-interop) to embed the core engine.

---

### 2.11 Pure-Rust DSP & Native Hardware Media Surfaces (Specs 25, 26, 29)
* **Choice**: **Pure-Rust lock-free audio DSP** on desktop.
* **Rationale**: Avoids linking bulky C++ WebRTC libraries while providing sub-10ms acoustic echo cancellation, noise suppression, and sample-rate drift resamplers without heap allocations in the audio path.
* **Choice**: **Direct Android `MediaCodec` hardware zero-copy surfaces**.
* **Rationale**: Decoding AV1/H.264 video in software on mobile devices causes overheating and frame drops. Decoding directly to a native hardware `Surface` minimizes CPU usage and battery drain.

---

### 2.12 Cryptographic Backbone: MLS & Post-Compromise Security (Spec 28)
* **Choice**: **IETF Messaging Layer Security (MLS, RFC 9420)** for groups and 1:1 sessions.
* **Rationale**: Traditional Signal ratchets scale at $O(N)$ for group operations, becoming inefficient in large groups. MLS scales at $O(\log N)$ while maintaining both **Forward Secrecy (FS)** and **Post-Compromise Security (PCS)**.
* **Choice**: **Explicit memory zeroization** (`zeroize`).
* **Rationale**: Sensitive cryptographic secrets, private keys, and intermediate ratchet states are overwritten with zeros immediately upon being dropped from memory.

---

### 2.13 High-Anonymity Mixnet: Loopix, Sphinx & Cover Traffic (Specs 34–42)
* **Choice**: **Stratified mixnet (Loopix)** instead of circuit-based onion routing (Tor).
* **Rationale**: Tor is vulnerable to end-to-end timing correlation attacks by global passive adversaries. Loopix introduces independent Poisson-distributed delays and stratified mix layers to defeat traffic analysis.
* **Choice**: **Sphinx packet encapsulation with cell normalization**.
* **Rationale**: All packets are padded to identical sizes. Layered encryption peels away at each hop without altering the total packet length, preventing size-based correlation.
* **Choice**: **Decoupled anonymous rendezvous points for attachments** (Spec 40).
* **Rationale**: Large attachments cannot be routed directly through high-latency mixnet buffers without causing congestion. Payloads are encrypted and stored at anonymous rendezvous points; only the retrieval tokens traverse the mixnet.

---

### 2.14 Zero-Trust Infrastructure: Measured Boot, HSMs & Erasure (Specs 67, 71–74)
* **Choice**: **TPM measured boot and remote host attestation** (Spec 71).
* **Rationale**: Server nodes operating in cloud or third-party datacenters cannot enroll in the cluster without cryptographically proving their firmware and binary integrity.
* **Choice**: **Hardware Security Module (HSM) signing ceremonies** (Spec 72).
* **Rationale**: Root cluster keys and directory authority keys never touch general-purpose server memory.
* **Choice**: **Cryptographic erasure** (Spec 67).
* **Rationale**: Standard disk file deletion leaves forensic traces. Encrypting data at rest and shredding the corresponding decryption key renders the underlying storage permanently unrecoverable.

---

### 2.15 Anti-Surveillance Services: PIR Search & Differential Privacy (Specs 91–92)
* **Choice**: **Private Information Retrieval (PIR)** for search indexing.
* **Rationale**: Standard search queries reveal user intent and interest graphs to the server. PIR allows clients to query remote indices without the server discovering which record was retrieved.
* **Choice**: **Differential Privacy and secure cryptographic aggregation** for telemetry.
* **Rationale**: Operators need cluster health metrics, but raw telemetry enables user surveillance. Mathematical noise injection ensures individual user actions cannot be reconstructed from aggregate statistics.

---

### 2.16 SRE, Physical Tamper Protection & Incident Command (Specs 109–121)
* **Choice**: **Physical chassis tamper detection and hardware chain-of-custody** (Spec 117).
* **Rationale**: Nation-state adversaries may attempt physical hardware attacks against server racks. Tamper triggers execute automated memory zeroization.
* **Choice**: **Strict mathematical SLO error budgets and fair load-shedding** (Specs 101, 109).
* **Rationale**: Protects cluster stability during DDoS attacks or resource saturation without sacrificing high-priority traffic.

---

### 2.17 Engineering Traceability & Release Evidence Archive (Specs 122–126)
* **Choice**: **Engineering Knowledge Graph mapping requirements to tests** (Spec 123).
* **Rationale**: In safety-critical distributed systems, every design invariant must map directly to automated test assertions.
* **Choice**: **Cryptographic Release Evidence Repository** (Spec 126).
* **Rationale**: Release binaries must be accompanied by cryptographic proofs of passing compliance and security test suites before production deployment.

---

### 2.18 Sandboxed WASM Plugins & Information Flow Control (Specs 134–146)
* **Choice**: **WebAssembly (WASM) capability sandboxing** for third-party extensions.
* **Rationale**: Prevents untrusted third-party code from accessing host system resources, memory, or local storage.
* **Choice**: **Information Flow Control (IFC)** (Spec 136).
* **Rationale**: If an extension reads decrypted conversation data, the runtime revokes its outbound network access, preventing data exfiltration.

---

### 2.19 Cross-Platform UI Architecture: Dioxus, Compose & Shared State Machines
* **Choice**: **Dioxus 0.7 Desktop (Linux/Windows) + Jetpack Compose (Android)**.
* **Rationale**: Dioxus provides memory-efficient, reactive native desktop rendering without the heavy memory footprint of Electron. Jetpack Compose provides first-class native Android UI performance and gesture handling.
* **Choice**: **Framework-agnostic UI state machine crate** (`siar-ui-state`).
* **Rationale**: Desktop and Android applications share identical state transitions, delivery status models, and pairing state machines, eliminating duplicated frontend business logic.

---

## Master Architecture Dependency Flow

```text
               ┌────────────────────────────────────────────────────────┐
               │    Tier 0 Core Engine (Specs 01-09)                    │
               │    Envelopes, Multi-Device Keys, Routing, DTN, Blobs   │
               └───────────────────────────┬────────────────────────────┘
                                           │
                                           ▼
               ┌────────────────────────────────────────────────────────┐
               │    Tier 1 Security Backbone (Spec 28)                  │
               │    Double Ratchet, OpenMLS, AEAD                       │
               └───────────────────────────┬────────────────────────────┘
                                           │
                                           ▼
               ┌────────────────────────────────────────────────────────┐
               │    High-Anonymity Mixnet Plane (Specs 34-42)           │
               │    Sphinx Framing, Poisson Delays, Cover Loops, Nym    │
               └───────────────────────────┬────────────────────────────┘
                                           │
                   ┌───────────────────────┴───────────────────────┐
                   ▼                                               ▼
┌──────────────────────────────────────┐       ┌──────────────────────────────────────┐
│ Anonymous App Layer (Specs 43-49)    │       │ Zero-Trust Infrastructure (50-81)    │
│ Private Groups, Calls, Presence      │       │ Attestation, HSM, Consensus, mTLS    │
└──────────────────┬───────────────────┘       └──────────────────┬───────────────────┘
                   │                                               │
                   └───────────────────────┬───────────────────────┘
                                           ▼
               ┌────────────────────────────────────────────────────────┐
               │ Private Cloud & Social Surfaces (Specs 82-94)          │
               │ Feeds, Channels, PIR Search, Differential Privacy      │
               └───────────────────────────┬────────────────────────────┘
                                           │
                                           ▼
               ┌────────────────────────────────────────────────────────┐
               │ SRE, Physical Fleet & Threat Defense (Specs 95-121)    │
               │ Tamper Detection, Error Budgets, Incident Command      │
               └───────────────────────────┬────────────────────────────┘
                                           │
                                           ▼
               ┌────────────────────────────────────────────────────────┐
               │ Developer Platform & Sandboxed WASM Plugins (122-150)  │
               │ SDKs, Capability Brokers, IFC Governance, Marketplace  │
               └────────────────────────────────────────────────────────┘
```
