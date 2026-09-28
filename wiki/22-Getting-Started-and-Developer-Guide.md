# 22 — Developer & Getting Started Guide

> **Target Audience:** Core Contributors, Integration Engineers, Application Developers, Security Researchers  
> **Repository:** [`github.com/irshadali5/siar`](https://github.com/irshadali5/siar)  
> **Workspace Composition:** 39 Domain Crates, 4 Applications, 176 Architecture Specifications  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.17), [Wiki Chapter 21](21-Testing-Fuzzing-and-Network-Diagnostics.md), [Wiki Chapter 45](45-Engineering-Traceability-Knowledge-Graph-and-Release-Evidence.md)

> [!CAUTION]
> ### ⚠️ FOUNDATIONAL DEVELOPMENT NOTICE — NOT CURRENTLY USABLE ON ANY DEVICE
>
> * **Active Foundational Development:** SIAR is currently in pre-alpha foundational systems development. **It is NOT YET USABLE on any daily-driver mobile phone, desktop, or operational device.** End-user applications and installable packages will become operational upon the completion of the first formal milestone release (Milestone 1 / v0.1.0+).
> * **AI / LLM Development Transparency Disclosure:** SIAR's extensive architectural specifications (176 specifications across 514k+ lines), wiki documentation, and codebase are **heavily developed with the assistance of advanced Artificial Intelligence / Large Language Models (AI/LLMs)** working under human architectural direction and verification.
> * **URGENT: Security Researchers, Cryptographers & Protocol Engineers Required:** Because SIAR is designed for post-infrastructure, disaster-recovery, and high-threat environments, **independent third-party security researchers and protocol engineers are urgently needed** to perform formal mathematical verification, cryptographic audits, memory safety inspections, and fuzz testing across the cryptography (`siar-crypto`, `siar-crypto-mls`), wire framing (`siar-protocol`, `siar-protocol-ext`), transport/routing (`siar-transport`, `siar-routing-policy`), storage, and media subsystems to discover and eliminate potential vulnerabilities before real-world deployment.
>
> **DO NOT deploy SIAR in production, life-safety, high-threat, or operational environments until formal independent audits and the v0.1.0 milestone release are complete.**

---

## 1. Prerequisites & Environment Setup

SIAR is developed in modern Rust (2021 Edition, pinned to `rust-version = "1.91"`). The codebase enforces zero C-runtime dependencies for its embedded storage and cryptographic pipeline, ensuring friction-free cross-compilation across diverse target platforms.

### Option A: Hermetic Nix Flake (Recommended)
SIAR includes a fully hermetic Nix environment (`flake.nix`, `shell.nix`) providing compiler toolchains, Android NDK tools, and all native GUI/audio libraries (GTK3, WebKit2GTK, ALSA, OpenSSL, CMake):

```bash
# Enter the fully provisioned dev environment
nix develop

# Or build binaries directly via Nix
nix build .#siar-cli
nix build .#siar-desktop
nix build .#siar-emergency-node

# Run Nix check suite
nix flake check
```

### Nix Flake Configuration (`flake.nix` excerpt)
```nix
{
  description = "SIAR Sovereign Infrastructure & Anonymous Mesh Network";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };
  outputs = { self, nixpkgs, rust-overlay }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; overlays = [ (import rust-overlay) ]; };
      rustToolchain = pkgs.rust-bin.stable."1.91.0".default.override {
        extensions = [ "rust-src" "rust-analyzer" "clippy" ];
        targets = [ "aarch64-linux-android" "armv7-linux-androideabi" "x86_64-unknown-linux-musl" ];
      };
    in {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustToolchain pkg-config openssl alsa-lib dbus
          gtk3 webkitgtk_4_1 clang llvm android-tools
        ];
      };
    };
}
```

### Option B: Manual Toolchain Setup
```bash
# 1. Install Rust via rustup (Rust 1.91.0 or newer required)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable
rustup component add clippy rustfmt

# 2. Install native system dependencies (Ubuntu/Debian)
sudo apt update && sudo apt install -y \
    build-essential pkg-config libssl-dev libasound2-dev \
    libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libdbus-1-dev

# 3. Clone the repository
git clone https://github.com/irshadali5/siar.git
cd siar
```

---

## 2. Workspace Organization & Architecture Map (39 Domain Crates)

The repository is structured into a clean monorepo comprising 39 domain crates and 4 application targets:

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
│   ├── [Core Domain, Identity & Trust]
│   │   ├── siar-domain/                  # Core entities: AccountId, DeviceId, Ticket, SafetyFingerprint
│   │   ├── siar-crypto/                  # Ed25519, X25519, ChaCha20-Poly1305, zeroize primitives
│   │   ├── siar-crypto-mls/              # IETF MLS (RFC 9420) Tree-KEM 1:1 and group E2EE engine
│   │   ├── siar-identity-multidevice/    # Multi-device authority, device certs, trust store, SAS pairing
│   │   └── siar-identity-audit-recorder/ # Monotonic append-only audit trail for identity lifecycle & revocations
│   ├── [Protocols, Capability & Extension Engine]
│   │   ├── siar-protocol/                # Wire envelopes, Postcard binary codec, frame types
│   │   ├── siar-protocol-ext/            # Extensible protocol engine: FairScheduler, BoundedQueue, health
│   │   └── siar-capability/              # Two-phase capability negotiation & codec matrices
│   ├── [Mesh Routing, Policy & Dynamic Connectivity]
│   │   ├── siar-routing-policy/          # Multi-metric candidate scoring, hysteresis, decide_route
│   │   └── siar-connectivity/            # Cross-transport state engine, connection pooling & link probes
│   ├── [Multi-Transport Physical Sockets]
│   │   ├── siar-transport/               # Transport manager, pooled socket multiplexer & lifecycle
│   │   ├── siar-transport-ble/           # Linux/cross-platform Bluetooth Low Energy (L2CAP CoC) driver
│   │   ├── siar-transport-ble-android/   # Android native Bluetooth Low Energy transport driver
│   │   ├── siar-transport-bluetooth-classic/ # High-throughput RFCOMM Bluetooth Classic transport
│   │   ├── siar-transport-wifi-direct/   # High-bandwidth Wi-Fi Direct P2P ad-hoc transport
│   │   └── siar-transport-wifi-aware/    # Wi-Fi Aware (NAN — Neighbor Awareness Networking) transport
│   ├── [DTN, Emergency Priority & Life-Safety Services]
│   │   ├── siar-dtn-bundle/              # Bundle framing, custody receipts & Spray-and-Wait forwarding
│   │   ├── siar-dtn-bundle-service/      # Background daemon service orchestrating DTN bundle sync
│   │   ├── siar-emergency/               # 5-tier priority class queuing (P0 Life-Safety to P4 Bulk)
│   │   └── siar-emergency-service/       # Autonomous SOS beacon broadcaster & triage packet engine
│   ├── [Storage, Blobs, Event Logging & Reliability]
│   │   ├── siar-storage/                 # Pure-Rust Stoolap embedded relational SQL (Messages, Outbox)
│   │   ├── siar-event-log/               # Monotonic append-only event log & causal gap detection
│   │   ├── siar-event-notify/            # Internal reactive pub-sub event notification dispatcher
│   │   ├── siar-event-registry/          # Global schema registry of system events and audit subscribers
│   │   ├── siar-blob-manifest/           # SIMD BLAKE3 Merkle DAG blob chunking & AEAD encryption
│   │   ├── siar-file-transfer-service/   # Peer-assisted swarm blob chunk distribution coordinator
│   │   ├── siar-resource-limits/         # Token-bucket backpressure, ring buffers & queue drop policies
│   │   ├── siar-crash-recovery/          # WAL recovery, transactional checkpoints & corrupt state isolation
│   │   ├── siar-startup-recovery/        # Cold-boot consistency verification & orphaned ticket repair
│   │   └── siar-remote-ingestion/        # Ingestion gateway for external telemetry and bridge adapters
│   ├── [Messaging Orchestration & UI State Machines]
│   │   ├── siar-messaging/               # MessageService, GroupService, Ticket manager, multi-node tests
│   │   └── siar-ui-state/                # Framework-agnostic UI state machines & Security Center
│   ├── [Realtime Media, Voice/Video & Hardware Codecs]
│   │   ├── siar-media-core/              # Media traits, raw video/audio buffers, sample clocks
│   │   ├── siar-media-audio/             # Pure-Rust lock-free audio DSP (Opus, AEC, NS, AGC, <10ms latency)
│   │   ├── siar-media-av1/               # Desktop dav1d AV1 video decoder with lookahead decoding
│   │   ├── siar-media-android/           # Android MediaCodec hardware surface zero-copy pipeline
│   │   ├── siar-media-image/             # Image processing, format transcoding & responsive thumbnails
│   │   └── siar-calls/                   # Realtime P2P media call session protocols & signaling
│   └── [Simulation & Test Harness]
│       └── siar-testkit/                 # In-memory virtual radio mesh simulator & link impairments
├── sys-arch/                             # 176 System Architecture Specifications
└── wiki/                                 # 50-Chapter Comprehensive Architectural Wiki
```

---

## 3. Compiling & Testing the Workspace

```bash
# 1. Fast compile-check across all 33 crates and test targets
cargo check --workspace --tests

# 2. Run unit tests across all crates
cargo test --workspace

# 3. Run multi-node end-to-end cryptographic integration test
cargo test -p siar-messaging --test end_to_end

# 4. Run property-based invariant tests (10,000 cases)
PROPTEST_CASES=10000 cargo test -p siar-protocol --test proptests

# 5. Build optimized release binaries
cargo build --release --workspace
```

---

## 4. Running Applications Locally

### 1. Desktop GUI (Dioxus 0.7)
```bash
# Launch native desktop application with hardware acceleration
cargo run --bin siar-desktop
```

### 2. Interactive Terminal Messenger (`siar-cli`)
```bash
# Launch interactive terminal UI (Ratatui TUI)
cargo run --bin siar-cli

# Run one-off diagnostic commands
cargo run --bin siar-cli -- peers list
cargo run --bin siar-cli -- send --to <ACCOUNT_ID> --msg "Hello over mesh"
```

### 3. Headless Emergency Repeater Daemon
```bash
# Launch autonomous background node with in-memory storage
cargo run --bin siar-emergency-node -- --config config/emergency-node.toml
```

---

## 5. Local Multi-Node Virtual Mesh Testbed (Linux Namespaces & Netem)

To simulate a real multi-node mesh topology on a single Linux development machine without physical radio hardware, use Linux **Network Namespaces** with `tc netem` latency/loss injection:

```bash
# 1. Create two isolated network namespaces
sudo ip netns add node_alpha
sudo ip netns add node_beta

# 2. Create virtual ethernet link between namespaces
sudo ip link add veth_a type veth peer name veth_b
sudo ip link set veth_a netns node_alpha
sudo ip link set veth_b netns node_beta

# 3. Assign IP addresses and bring up interfaces
sudo ip netns exec node_alpha ip addr add 10.200.1.1/24 dev veth_a
sudo ip netns exec node_alpha ip link set veth_a up
sudo ip netns exec node_alpha ip link set lo up

sudo ip netns exec node_beta ip addr add 10.200.1.2/24 dev veth_b
sudo ip netns exec node_beta ip link set veth_b up
sudo ip netns exec node_beta ip link set lo up

# 4. Inject 40ms latency and 3% packet drop to simulate RF fade
sudo ip netns exec node_alpha tc qdisc add dev veth_a root netem delay 40ms 10ms loss 3%
sudo ip netns exec node_beta tc qdisc add dev veth_b root netem delay 40ms 10ms loss 3%

# 5. Launch Node Alpha in namespace 1
sudo ip netns exec node_alpha cargo run --bin siar-cli -- --db /tmp/alpha.db --bind 10.200.1.1:7420

# 6. In a second terminal, launch Node Beta in namespace 2
sudo ip netns exec node_beta cargo run --bin siar-cli -- --db /tmp/beta.db --bind 10.200.1.2:7420
```

---

## 6. Embedded Developer Rust Integration Example

The following self-contained Rust program demonstrates how third-party developers embed the SIAR core engine within custom software applications:

```rust
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize SIAR Sovereign Storage Engine
    let storage_config = siar_storage::StorageConfig {
        db_path: "/tmp/my_embedded_node.db".into(),
        enable_wal: true,
        crypto_shredding: false,
    };
    let storage = siar_storage::SovereignStorage::open(storage_config).await?;

    // 2. Generate or Load Ephemeral Identity Keypair
    let identity = siar_crypto::IdentityKeyPair::generate();
    println!("Node initialized with Account ID: {}", identity.public_key().to_hex());

    // 3. Instantiate Outbox and Mesh Routing Engine
    let (event_tx, mut event_rx) = mpsc::channel(100);
    let router = siar_routing_policy::MeshRouter::new(identity.clone(), storage.clone(), event_tx);

    // 4. Background task: Listen for incoming messages and delivery receipts
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                siar_routing_policy::MeshEvent::MessageReceived { from, payload } => {
                    let text = String::from_utf8_lossy(&payload);
                    println!("[RECV from {}]: {}", from.to_hex(), text);
                }
                siar_routing_policy::MeshEvent::DeliveryAckReceived { ticket_id } => {
                    println!("[ACK]: Message ticket {} successfully delivered!", ticket_id);
                }
            }
        }
    });

    // 5. Send an encrypted message across the mesh
    let peer_target = siar_crypto::PublicKey::from_hex("a8f9c104e7b8923a41b5d6c7e8f90123")?;
    let ticket = router.enqueue_outbox(peer_target, b"Hello from embedded SIAR!").await?;
    println!("Message enqueued. Outbox Ticket ID: {}", ticket);

    // Keep process alive
    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    Ok(())
}
```

---

## 7. Performance Profiling & Undefined Behavior Verification

SIAR mandates rigorous profiling and verification workflows before merging code into mainline:

```bash
# 1. CPU Flamegraph Profiling (Identifies hot paths in cryptography & routing)
cargo flamegraph --bin siar-cli -- --bench-crypto

# 2. Memory Leak & Heap Allocation Profiling (heaptrack)
heaptrack cargo run --release --bin siar-desktop

# 3. Source-Based Code Coverage (Requires LLVM tools)
cargo llvm-cov --all-features --workspace --html
xdg-open target/llvm-cov/html/index.html

# 4. Miri Undefined Behavior & Memory Safety Verification
cargo +nightly miri test -p siar-crypto
cargo +nightly miri test -p siar-protocol
```

---

## 8. Continuous Integration Quality Gates Matrix

| Pipeline Gate | Tool / Command | Enforcement Threshold |
| :--- | :--- | :--- |
| **Strict Formatting** | `cargo fmt --all -- --check` | Zero diff allowed |
| **Compiler Warnings** | `cargo clippy --all-targets -- -D warnings` | Zero warnings permitted |
| **Security Auditing** | `cargo audit --deny warnings` | Zero high/critical CVEs |
| **Dead Dependency Check** | `cargo machete` | Zero unused crate dependencies |
| **Undefined Behavior** | `cargo +nightly miri test` | Clean exit with zero UB warnings |
| **ASan / UBSan Fuzzing** | `cargo fuzz run <target> -- -max_total_time=3600` | 100M executions with zero crashes |
| **Reproducible Builds** | `nix build .#siar-cli` | Bit-for-bit identical hashes across runners |

---

## 9. Security Auditing, Formal Verification & Vulnerability Research Guidelines

> [!IMPORTANT]
> ### 🛡️ Call for Independent Security Researchers & Cryptographers
> 
> Because SIAR is designed for mission-critical, post-infrastructure, and high-threat environments, independent third-party audits, cryptographic reviews, and formal verifications are essential before any production release.
> 
> **AI / LLM Development Disclosure:**  
> SIAR’s architecture specifications and initial crate implementations are **heavily developed with the assistance of advanced Artificial Intelligence / Large Language Models (AI/LLMs)** working under human engineering direction. While all code adheres to `#![deny(unsafe_code)]`, strict typing, and comprehensive test suites, automated and AI-generated systems must be subjected to rigorous external human cryptographic audit, penetration testing, and formal verification.

### Core Audit Priority Surfaces

1. **Cryptographic Primitives & Key Ratchets ([`crates/siar-crypto`](../crates/siar-crypto), [`crates/siar-crypto-mls`](../crates/siar-crypto-mls))**:
   - IETF MLS (RFC 9420) Tree-KEM epoch transitions, path secret derivations, and out-of-order commit handling.
   - ML-KEM-768 / Kyber post-quantum hybrid KEM implementation and side-channel resistance.
   - Strict memory zeroization verification (`zeroize::Zeroize`) to prevent key residue in RAM dumps.
2. **Wire Framing, Deserialization & Schedulers ([`crates/siar-protocol`](../crates/siar-protocol), [`crates/siar-protocol-ext`](../crates/siar-protocol-ext))**:
   - Postcard binary deserialization fuzzing to detect buffer overflows, panics, or infinite recursion.
   - Variable-length integer (LEB128) parsing edge cases.
   - Token-bucket rate limiter and `FairScheduler` starvation / denial-of-service resilience.
3. **Transport Multiplexing & Routing Security ([`crates/siar-transport`](../crates/siar-transport), [`crates/siar-routing-policy`](../crates/siar-routing-policy))**:
   - QUIC connection migration security and replay protection.
   - Bluetooth Low Energy L2CAP CoC packet injection, spoofing, and MTU fragmentation handling.
   - Wi-Fi Direct and Wi-Fi Aware (NAN) peer discovery and unassociated beacon parsing.
4. **Storage & Transactional Crash Recovery ([`crates/siar-storage`](../crates/siar-storage), [`crates/siar-crash-recovery`](../crates/siar-crash-recovery))**:
   - Write-Ahead Log (WAL) atomicity and crash recovery under sudden power loss simulation.
   - Cryptographic shredding verification: ensuring data at rest is mathematically irrecoverable once a key is zeroized.
5. **Realtime Media Pipelines & DSP ([`crates/siar-calls`](../crates/siar-calls), [`crates/siar-media-audio`](../crates/siar-media-audio))**:
   - Lock-free audio buffer bounds, avoiding priority inversion or deadlock under real-time audio thread constraints.

### Vulnerability Reporting & Responsible Disclosure
If you identify a security flaw, cryptographic vulnerability, or potential exploit in SIAR:
1. **Do not disclose publicly** in GitHub Issues or public forums.
2. Submit a confidential report to the core security team with reproduction steps, PoC code, and impacted crate identifiers.
3. Reports are prioritized and reviewed within 48 hours, followed by coordinated disclosure and public attribution upon patch release.

