# Contributing to SIAR

First off, thank you for considering contributing to **SIAR** (*Survivable Identity & Autonomous Routing*)! 🚀

SIAR is a zero-infrastructure, multi-transport, offline-first decentralized messaging and delay-tolerant networking (DTN) platform built to guarantee private, autonomous communications for humanity.

---

## Table of Contents

1. [Code of Conduct](#code-of-conduct)
2. [Contributor License Agreement (CLA)](#contributor-license-agreement-cla)
3. [Architecture & Design Principles](#architecture--design-principles)
4. [Development Environment & Prerequisites](#development-environment--prerequisites)
5. [Development Workflow](#development-workflow)
6. [Code Style & Best Practices](#code-style--best-practices)
7. [Testing, Benchmarking & Fuzzing](#testing-benchmarking--fuzzing)
8. [Submitting a Pull Request](#submitting-a-pull-request)
9. [Reporting Security Vulnerabilities](#reporting-security-vulnerabilities)

---

## Code of Conduct

We are committed to providing a friendly, safe, and welcoming environment for everyone, regardless of background, gender identity, sexual orientation, disability, physical appearance, race, age, or religion. Please be respectful, constructive, and collaborative in all discussions, issues, and PR reviews.

---

## Contributor License Agreement (CLA)

All contributions to SIAR are governed by the **[SIAR Contributor License Agreement](file:///home/irshad/Projects/siar/CLA.md)**.

- Core libraries under `crates/*` are licensed under **MIT OR Apache-2.0**.
- Standalone applications and daemons under `apps/*` are licensed under **GNU AGPLv3**.
- Maintainers hold the right to grant commercial exemptions under `SIAR-CEEL-1.0` to commercial entities, with 100% of licensing fees reinvested into core development.

By opening a Pull Request or submitting a patch, you agree to the terms in **[`CLA.md`](file:///home/irshad/Projects/siar/CLA.md)**. We recommend signing your commits with `git commit -s` (`Signed-off-by`).

---

## Architecture & Design Principles

Before writing code, please understand SIAR's architectural boundaries:

```
┌────────────────────────────────────────────────────────┐
│               APPS LAYER (GNU AGPLv3)                  │
│  apps/cli, apps/desktop, apps/android,                 │
│  apps/emergency-node                                   │
└───────────────────────────┬────────────────────────────┘
                            │
┌───────────────────────────▼────────────────────────────┐
│         CORE LIBRARIES LAYER (MIT OR Apache-2.0)       │
│  crates/siar-messaging, crates/siar-storage,           │
│  crates/siar-crypto-mls, crates/siar-transport,        │
│  crates/siar-routing-policy, crates/siar-dtn-bundle,   │
│  crates/siar-connectivity, etc.                        │
└────────────────────────────────────────────────────────┘
```

1. **Rust is the Primary Core Engine (95%+ Codebase)**: All cryptography, binary wire serialization, DTN queues, routing tables, storage engines, and multi-transport socket management are written in pure modern Rust (2021 Edition).
2. **Kotlin for Android UI Only**: Kotlin is strictly used in `apps/android` for Jetpack Compose UI and native Android OS hardware bindings, interfacing directly with the Rust engine via zero-copy C-ABI JNI bridges.
3. **Strict Boundary Isolation**:
   - `crates/siar-domain`: Pure domain models, zero heavy I/O dependencies.
   - `crates/siar-storage`: Pure-Rust embedded SQL storage engine abstraction.
   - `crates/siar-transport`: Transport-agnostic pool (BLE, Wi-Fi Direct, Wi-Fi Aware, BT Classic, Iroh QUIC).
4. **Zero Panic in Production Code**: Do not use `unwrap()` or `expect()` in non-test code paths. Always use idiomatic error propagation via `Result<T, E>` and `thiserror`.

---

## Development Environment & Prerequisites

### 1. Rust Toolchain
Install Rust using [rustup](https://rustup.rs/):
```bash
rustup toolchain install 1.91
rustup default 1.91
rustup component add clippy rustfmt
```

### 2. Android NDK (For Android Development)
If you plan to compile native JNI libraries for Android:
```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

---

## Development Workflow

### 1. Clone & Build
```bash
git clone https://github.com/irshadali5/siar.git
cd siar
cargo build --workspace
```

### 2. Create a Feature Branch
Use descriptive branch names:
```bash
git checkout -b feat/my-new-feature
# or
git checkout -b fix/routing-deadlock
```

### 3. Verify Code Quality Locally
Before pushing your branch, run the local verification suite:
```bash
# 1. Format check
cargo fmt --all -- --check

# 2. Clippy linter (zero warnings allowed)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Workspace unit tests
cargo test --workspace

# 4. Dependency license & security audit
cargo deny check
```

---

## Code Style & Best Practices

- **Error Handling**: Use [`thiserror`](https://docs.rs/thiserror) for typed, actionable errors in `crates/*` libraries; use [`anyhow`](https://docs.rs/anyhow) for application-level error contexts in `apps/*`.
- **Async Runtime**: Use `tokio` for async orchestration. Never block async executor worker threads with long-running synchronous CPU operations (use `tokio::task::spawn_blocking` where appropriate).
- **Zero-Copy & Memory Efficiency**: Favor `bytes::Bytes` and borrowed slices (`&[u8]`) over cloning large memory buffers.
- **Documentation**: Provide clear Rust doc comments (`///`) on all public types, structs, methods, and traits.

---

## Testing, Benchmarking & Fuzzing

### Unit & Integration Tests
Write tests alongside modules in `#[cfg(test)]` submodules or in `tests/` integration directories:
```bash
cargo test -p siar-messaging
cargo test -p siar-storage
```

### Fuzzing
For binary protocol framing and wire codecs (`crates/siar-protocol`), run the cargo-fuzz targets:
```bash
cargo install cargo-fuzz
cd fuzz
cargo fuzz run decode_frame
cargo fuzz run decode_blob_frame
```

---

## Submitting a Pull Request

1. **Commit Messages**: Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:
   - `feat(transport): add adaptive Wi-Fi Aware channel bonding`
   - `fix(storage): resolve integer constraint in message repository`
   - `docs(readme): update deployment topology guide`
   - `test(mls): add out-of-order epoch verification test`
2. **Sign Your Commits**: Sign commits using `git commit -s -m "..."` to indicate acceptance of the [CLA](file:///home/irshad/Projects/siar/CLA.md).
3. **Open Pull Request**: Push your branch to GitHub and open a PR against `main`. Provide a clear description of:
   - The problem being solved.
   - The proposed solution and design decisions.
   - What tests were added or executed to verify correctness.

---

## Reporting Security Vulnerabilities

Security and cryptographic integrity are paramount for SIAR.

If you discover a security vulnerability or cryptographic flaw:
- **DO NOT open a public GitHub issue.**
- Report the vulnerability responsibly via email to **`security@siar.network`** or directly to **`irshad@siar.network`**.
- We will acknowledge receipt within 24 hours and coordinate a responsible patch and disclosure timeline.
