# 22 — Developer & Getting Started Guide

> **Target Audience:** Core Contributors, Integration Engineers, Application Developers  
> **Repository:** [`github.com/irshadali5/siar`](https://github.com/irshadali5/siar)  
> **Workspace Composition:** 33 Crates, 4 Applications, 176 Architecture Specifications  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.17), [Wiki Chapter 21](21-Testing-Fuzzing-and-Network-Diagnostics.md), [Wiki Chapter 45](45-Engineering-Traceability-Knowledge-Graph-and-Release-Evidence.md)

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

## 2. Workspace Organization & Architecture Map

The repository is structured into a clean monorepo:

```
siar/
├── apps/
│   ├── android/            # Kotlin Jetpack Compose app shell & JNI bridge
│   ├── cli/                # Interactive terminal messenger & diagnostics tool
│   ├── desktop/            # Dioxus 0.7 native Rust GUI application
│   └── emergency-node/     # Headless background repeater daemon for SBCs
├── crates/
│   ├── siar-crypto/        # Hybrid Post-Quantum KEM, Ed25519, ChaCha20, BLAKE3
│   ├── siar-crypto-mls/    # IETF MLS (RFC 9420) TreeKEM group ratchets
│   ├── siar-storage/       # Pure-Rust Stoolap embedded SQL & WAL persistence
│   ├── siar-messaging/     # Outbox engine, delivery tickets, conversation models
│   ├── siar-routing-policy/# Multi-metric path scorer & hysteresis policy
│   ├── siar-connectivity/  # Connection pooling, peer link health, probing
│   ├── siar-dtn/           # Delay-Tolerant Networking, Spray-and-Wait, Bloom sync
│   ├── siar-transport/     # Abstract TransportDriver & StreamChannel traits
│   ├── siar-transport-ble/ # Bluetooth LE L2CAP CoC driver
│   ├── siar-transport-wifi/# Wi-Fi Direct and Wi-Fi Aware NAN drivers
│   ├── siar-calls/         # Realtime voice/video signaling & session management
│   ├── siar-media-audio/   # Pure-Rust audio DSP (AEC, NS, AGC, Opus encoder)
│   ├── siar-media-av1/     # Hardware-accelerated AV1 zero-copy video packetizer
│   ├── siar-ui-state/      # Reactive Unidirectional Data Flow state engine
│   ├── siar-protocol-ext/  # 108/108 spec-complete protocol extension engine
│   └── siar-testkit/       # Virtual multi-hop mesh simulation testbed
├── sys-arch/               # 176 System Architecture Specifications
└── wiki/                   # 50-Chapter Comprehensive Architectural Wiki
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
