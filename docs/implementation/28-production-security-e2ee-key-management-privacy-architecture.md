# Part 28 — Production Security / E2EE Key Management / Privacy Architecture

Source spec: `sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md` — 127 numbered sections.

## Implementation status

**🟡 Substantially advanced across multiple dedicated security rounds.**
Covers message envelopes, replay protection, persistent identity derivation, persistent security logs, abuse gates, 1:1 pairwise MLS post-quantum sessions, FROST threshold root ceremonies, hybrid post-quantum signatures, BLS audit aggregation, BBS relay credentials, and mutation fuzz testing.

## Implementing crate(s)

- `siar-crypto` — device certificates, message envelopes, replay protection, key store abstractions, KDF secret derivation (`DeviceIdentity::derive_secret`).
- `siar-identity-multidevice` — device revocation, trust states, safety fingerprints, SAS codes, root identity keys, linking protocol.
- `siar-crypto-mls` — MLS groups, persistent group sessions, 1:1 pairwise MLS sessions (`siar-crypto-mls::pairwise`), post-quantum ciphersuites.
- `siar-abuse` — admission control, peer quotas, stranger budgets, inbox limits, blocklist enforcement, emergency authority.
- `siar-crypto-advanced` — FROST root threshold keys (`frost_root`), ML-DSA-65/Ed25519 hybrid signatures (`hybrid`), BLS multi-signature audit aggregation (`bls_audit`), and BBS zero-knowledge relay credentials (`bbs_relay`).
- `apps/root-ceremony` (`siar-root`) — CLI for distributed key generation and threshold signing ceremonies.

---

## Architecture & Subsystems Implemented

### 1. Ingress Admission & Abuse Resistance (`siar-abuse`, §60-67)
- **Admission Pipeline (§61)**: Cheap-validation-first pipeline: Length → Version → Blocklist → Rate limit → Duplicate check → Authorization. Blocked or oversized items allocate zero buffers.
- **Quotas (§62)**: Six distinct quotas per peer, plus a shared global budget for all unknown peers (Sybil resistance bounding collective resource consumption).
- **Message Requests & File Offers (§63-64)**: Bounded inbox for unknown peers; automatic downloads disabled for untrusted senders; oversized file offers rejected.
- **Blocklist Enforcement (§65)**: Evaluated before cryptographic verification. Blocking immediately revokes active sessions; unblocking never silently restores trust.
- **Pairing & Emergency Policies (§60, §67)**: Exponential cooldown on failed pairing; unforgeable emergency authority (remote SOS requests require local validation).
- **Ingress Gate Wiring**: `siar-messaging::IngressGuard` integrates `siar-abuse::Gate` into the daemon receive loop, keyed on QUIC endpoint IDs. Also wired into `apps/emergency-node` to gate incoming DTN bundles and relay check-ins.

### 2. Forward-Secret Pairwise Messaging via OpenMLS 1:1
- Implemented in `siar-crypto-mls::pairwise` using two-member MLS groups.
- Post-quantum hybrid ciphersuite: `MLS_128_MLKEM768X25519_CHACHA20POLY1305_SHA384_MLDSA44` (ML-KEM-768 + X25519 key exchange, ML-DSA-44 signatures).
- DTN adaptations: alternating commit turns (creator at even epochs, peer at odd epochs) to prevent commit collisions across offline delays; bounded buffer for future epoch messages; replay detection on old commits.
- Replaced the experimental hand-rolled ratchet (`siar-ratchet` / `siar-secure-session`) with standardized OpenMLS primitives.

### 3. Advanced Cryptography (`siar-crypto-advanced`)
- **FROST Root Authority (`frost_root`)**: `frost-ed25519` 3.0 (RFC 9591). Real threshold DKG without a central dealer. Yields standard Ed25519 public keys and signatures verifiable with standard Ed25519 verifiers.
- **Post-Quantum Hybrid Signatures (`hybrid`)**: ML-DSA-65 + Ed25519 composite signatures following `draft-ietf-lamps-pq-composite-sigs`. Applied as countersignatures on `DeviceCertificate`.
- **BLS Multi-Signature Audit Aggregation (`bls_audit`)**: `blst` min-pk with IETF Proof-of-Possession. Compact aggregation of hundreds of signer attestations into a single 96-byte signature + bitmap.
- **Anonymous Relay Credentials (`bbs_relay`)**: `zkryptium` 0.8 (IETF BBS draft-14). Privacy-preserving passes with hidden random IDs and selective disclosure of epochs and tiers. Upstream bug (panic on <240-byte proof and trailing bytes) identified and patched.
- **Root Ceremony CLI (`siar-root`)**: Supports `dkg1/2/3`, `sign1`, `package`, `sign2`, `aggregate`, and `verify`. Share files sealed with Argon2id + XChaCha20-Poly1305.

### 4. Transport Identity & State Persistence
- **Stable Transport Identity**: Fixed critical issue where ephemeral Iroh keys caused addresses and conversation IDs to drift on restart; endpoint keys are now deterministically derived from persistent identity via domain-separated KDF (`DeviceIdentity::derive_secret`).
- **Persistent Security Log**: Stored in stoolap table `security_events`; tracks identity key transitions, SAS verification changes, and security warnings across restarts.

### 5. Mutation Fuzzing (Hardening Round 2026-11-08)
- Deterministic, seeded mutation fuzzing executed on stable Rust:
  - Admin request decoding (20,000 inputs, live server barrage).
  - Node provisioning payloads and state files (6,000 mutants).
  - `RuntimeConfig::parse` validation rules (15,000 inputs).
  - Tamper-evident audit log: exhaustive single-byte mutations at every position.
  - Ceremony secret files, BLS attestations, and hybrid signatures.
- Caught and fixed edge-case bugs: non-UTF-8 node state files failing closed rather than crashing; non-UTF-8 audit log lines triggering proper quarantine.

---

## Known Gaps / Open Items

- **Default Message Path Migration**: Pairwise OpenMLS sessions currently require explicit session upgrade; wiring OpenMLS 1:1 as the default out-of-the-box transport session for all conversations is the remaining migration step.
- **Security Safe Modes & Profiles**: §79-80 security safe mode and §106-110 security profiles.
- **Cross-Implementation Test Vectors**: §93 test vectors for hybrid composite drafts and BBS passes.
- **Independent Security Review**: External review of cryptographic ceremony pipelines and memory zeroization across all workspace crates.
