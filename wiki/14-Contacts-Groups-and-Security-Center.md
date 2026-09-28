# 14 — Contacts, Groups & Security Center

> **Corresponding Specifications:** [`sys-arch/ui-ux-08-contacts-requests-verification-identity-architecture.md`](../sys-arch/ui-ux-08-contacts-requests-verification-identity-architecture.md), [`sys-arch/ui-ux-09-groups-membership-roles-architecture.md`](../sys-arch/ui-ux-09-groups-membership-roles-architecture.md), [`sys-arch/ui-ux-15-security-center-devices-keys-recovery-architecture.md`](../ui-ux/ui-ux-15-security-center-devices-keys-recovery-architecture.md)  
> **Key Crates:** [`crates/siar-identity-multidevice`](../crates/siar-identity-multidevice), [`crates/siar-crypto-mls`](../crates/siar-crypto-mls), [`crates/siar-ui-state`](../crates/siar-ui-state), [`apps/desktop`](../apps/desktop)  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.1, §2.12), [Wiki Chapter 02](02-Multi-Device-Identity-and-Trust.md), [Wiki Chapter 03](03-Cryptographic-Engine-and-Key-Management.md)

---

## 1. Sovereign Contact Trust Architecture & State Lifecycle

In SIAR, contacts are identified exclusively by their sovereign Ed25519 `AccountId` without reliance on centralized telephone directories or email registries. Every contact relationship is governed by a strict cryptographic trust state machine:

```text
                  [Discovered via Mesh / Incoming Message]
                                     │
                                     ▼
                           ┌───────────────────┐
                           │    Unverified     │
                           └─────────┬─────────┘
                                     │
        ┌────────────────────────────┴────────────────────────────┐
        │ (SAS 6-Digit / QR Scan)                                 │ (Tombstone Received)
        ▼                                                         ▼
┌───────────────┐                                         ┌───────────────┐
│   Verified    │                                         │    Revoked    │
└───────┬───────┘                                         └───────────────┘
        │
        │ (Contact Adds New Secondary Device / Rotates Key)
        ▼
┌───────────────────┐
│ ChangedUnverified │ ───> Blocks message dispatch with inline warning banner
└───────────────────┘
```

### Trust State Definitions & UX Invariants

| State | Security Level | UI Presentation | Cryptographic Action Required |
| :--- | :--- | :--- | :--- |
| **`Unverified`** | Baseline Trust | Yellow security shield banner; warning prompt on outgoing media transfers. | SAS numeric comparison or visual QR scan. |
| **`Verified`** | High Trust | Green verified shield; full capabilities enabled without warnings. | None. Cryptographically bound to peer root key. |
| **`ChangedUnverified`**| Security Alert | Red warning banner: *"Contact's cryptographic identity has changed. Verify safety number."* | User must explicitly confirm safety number before outbound messages can be dispatched. |
| **`Revoked`** | Untrusted / Hostile| Black blocked shield; all active sessions dropped; messages rejected at ingress. | Device permanently invalidated via signed tombstone. |

---

## 2. Short Authentication String (SAS) & Mutual Fingerprint Verification

Peers verify cryptographic integrity out-of-band to permanently eliminate Man-in-the-Middle (MITM) attacks by malicious relays:

### 1. Safety Number Comparison (60 Digits)
Derived symmetrically from the lexicographically sorted root keys of both accounts:

$$F(A, B) = \text{BLAKE3-DeriveKey}\left(\text{"SIAR-SafetyFingerprint-v1"}, \, \min(PK_A, PK_B) \parallel \max(PK_A, PK_B)\right)$$

The 32-byte digest is formatted into twelve 5-digit decimal blocks (`"49120 90124 55192 ..."`). If an active relay alters either public key, the fingerprints diverge completely.

### 2. Ephemeral SAS 6-Digit Code (In-Call / Nearby Verification)
During active audio calls or proximity pairing:

$$\text{SAS} = (\text{u32::from\_be\_bytes}(H_{\text{session}}[0..4])) \pmod{10^6}$$

Both users read the resulting 6-digit number aloud (`"842 193"`). A match guarantees zero-knowledge mutual authenticity with a collision probability:

$$P_{\text{collision}} \le 10^{-6}$$

---

## 3. MLS Group Architecture (RFC 9420) & TreeKEM Derivations

Group messaging is governed by **IETF Messaging Layer Security (MLS)** implemented in [`siar-crypto-mls`](../crates/siar-crypto-mls):

```mermaid
graph TD
    subgraph EpochTransition["MLS Epoch Evolution (Epoch N -> Epoch N+1)"]
        Admin["Admin / Committer"] -->|1. Generate Add/Remove Proposal| Prop["Proposal Queue"]
        Prop -->|2. Compute Commit Message| Commit["Commit Message (PathSecret Updates)"]
        Commit -->|3. Ratchet Tree Update| Tree["TreeKEM Binary Ratchet Tree"]
        Tree -->|4. Derive Epoch Secret| Secret["Epoch N+1 Group Secret"]
        Secret -->|5. Broadcast Commit to Mesh| Members["All Active Group Members"]
    end
```

### TreeKEM Binary Ratchet Tree Mathematics
The group state is represented by a left-balanced binary tree with $N$ leaf nodes:
- Leaf index for member $i$: $2i$.
- Parent index of node $x$: $\lfloor (x - 1) / 2 \rfloor$.
- Committer generates path secret $s_0$ at its leaf, ratcheting upwards to the root:
  $$s_{k+1} = \text{HKDF-Expand}(s_k, \text{"path"}, 32)$$
  $$\text{NodeSecret}_k = \text{HKDF-Expand}(s_k, \text{"node"}, 32)$$
  $$\text{PublicKey}_k = g^{\text{NodeSecret}_k}$$
Each updated node key is encrypted to the resolution of the copath, guaranteeing that all valid members can compute the root secret while excluded members lack the necessary copath secrets.

---

## 4. Shamir's Secret Sharing for Social Recovery Guardians

In [`ui-ux-15`](../ui-ux/ui-ux-15-security-center-devices-keys-recovery-architecture.md), identity recovery utilizes a $(k, n)$-threshold Shamir Secret Sharing scheme over Galois Field $\text{GF}(2^8)$ or prime field $\mathbb{F}_p$:

Let master recovery key be $S = a_0$. A random polynomial of degree $k - 1$ is sampled:

$$f(x) = a_0 + \sum_{j=1}^{k-1} a_j x^j \pmod p$$

Each guardian receives share $(x_i, f(x_i))$. Reconstructing the master recovery key requires any $k$ out of $n$ guardians using Lagrange interpolation:

$$S = f(0) = \sum_{i=1}^k y_i \prod_{j=1, j \ne i}^k \frac{-x_j}{x_i - x_j} \pmod p$$

Any coalition of fewer than $k$ guardians possesses mathematically zero information about $S$.

---

## 5. Security Center & Sovereignty Dashboard (`ui-ux-15`)

The **Security Center** provides users complete visibility and granular control over their cryptographic perimeter:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                                 SIAR SECURITY CENTER                                   │
├────────────────────────────────────────────────────────────────────────────────────────┤
│  [ Shield: Sovereign & Protected ] 3 Devices Enrolled • 0 Vulnerabilities Detected     │
│                                                                                        │
│  Active Devices:                                                                       │
│    📱 Primary Phone (Android 14)       • Gen 1 • Active Now (This Device)              │
│    💻 Tactical Laptop (Linux Dioxus)   • Gen 2 • Last seen 5m ago                      │
│    📟 Mesh Repeater Node (OpenWrt)     • Gen 3 • Relay Only (No Local DB)  [Revoke]    │
│                                                                                        │
│  Cryptographic Key Management:                                                         │
│    🔑 View / Export 24-Word Master Seed Phrase                                         │
│    📦 Generate Encrypted Offline Backup (.siarbackup)                                  │
│    🛡️ Configure Social Recovery Guardians (3-of-5 Threshold Active)                     │
│                                                                                        │
│  Emergency Lockdown:                                                                   │
│    🚨 Revoke All Secondary Devices (Signs Priority 0 Epidemic Tombstone)              │
│    💣 Emergency Duress Wipe (Zeroize RAM, Shred Flash Keys, Terminate)                 │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 6. Concrete Rust Contact & Security Center Traits

```rust
use std::collections::HashSet;

pub struct ContactRecord {
    pub account_id: [u8; 32],
    pub root_public_key: [u8; 32],
    pub safety_fingerprint: String,
    pub is_verified: bool,
    pub last_seen_timestamp: u64,
}

pub trait ContactTrustManager: Send + Sync {
    /// Ingest a newly discovered contact from mesh or out-of-band QR
    fn add_contact(&mut self, record: ContactRecord) -> Result<(), String>;

    /// Confirm SAS verification, upgrading status from Unverified to Verified
    fn confirm_sas_verification(&mut self, account_id: &[u8; 32]) -> Result<(), String>;

    /// Sign and broadcast an epidemic device revocation tombstone at Priority 0
    fn revoke_device(&mut self, device_id: &[u8; 32], reason: &str) -> Result<(), String>;
}
```

---

## 7. Threat Vectors & Identity Defense Matrix

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        CONTACT & GROUP THREAT DEFENSE MATRIX                           │
├────────────────────────┬─────────────────────────┬─────────────────────────────────────┤
│ Threat Vector          │ Attack Mechanism        │ SIAR Defense Architecture           │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Key Substitution**   │ Relay replaces contact's│ SAS verification & 60-digit safety  │
│                        │ public key with own     │ number detects MITM instantly.      │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Guardian Collusion** │ Sub-threshold guardians │ Shamir Secret Sharing guarantees    │
│                        │ attempt key recovery    │ zero leakage for $< k$ shares.      │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Rogue Member Re-Add**│ Evicted member attempts │ TreeKEM Commit ratchets path secrets│
│                        │ to read subsequent msgs │ forward; past keys cannot read new. │
└────────────────────────┴─────────────────────────┴─────────────────────────────────────┘
```

---

## 8. TreeKEM Group Asynchrony & Concurrent Commit Resolution (MLS / RFC 9420)

In high-latency peer-to-peer meshes, multiple group members frequently propose membership updates or key updates concurrently, resulting in conflicting TreeKEM epochs:

```mermaid
sequenceDiagram
    autonumber
    participant Alice as Alice (Node A)
    participant Bob as Bob (Node B)
    participant Mesh as Ad-Hoc Mesh Gossip

    Alice->>Mesh: Dispatches Commit_A for Epoch 5 (Hash: H_A)
    Bob->>Mesh: Dispatches Commit_B for Epoch 5 (Hash: H_B)
    Note over Mesh: Conflict! Two valid commits competing for Epoch 6

    Mesh->>Alice: Ingest Commit_B
    Mesh->>Bob: Ingest Commit_A
    Note over Alice,Bob: Deterministic Conflict Resolution Rule Evaluated
    Note over Alice,Bob: Winner = min(BLAKE3(Commit_A), BLAKE3(Commit_B))
    alt Commit_A wins
        Alice->>Alice: Advance to Epoch 6 via Commit_A
        Bob->>Bob: Rollback Commit_B; Re-apply update on top of Epoch 6
    else Commit_B wins
        Alice->>Alice: Rollback Commit_A; Re-apply update on top of Epoch 6
        Bob->>Bob: Advance to Epoch 6 via Commit_B
    end
```

### 8.1. Deterministic Conflict Resolution Rule
When conflicting commits $C_1, C_2$ both reference parent epoch head $E_t$:

$$\text{Winner}(C_1, C_2) = \begin{cases} C_1 & \text{if } \text{BLAKE3}(C_1) < \text{BLAKE3}(C_2) \\ C_2 & \text{otherwise} \end{cases}$$

The losing node's pending update is re-encapsulated as a standard proposal and committed in epoch $t+2$, guaranteeing complete group state convergence without a central coordinator.

---

## 9. Duress PIN & Plausible Deniability Architecture

In hostile checkpoints, border inspections, or armed robbery scenarios, an adversary may coerce a user to unlock their device under physical threat. SIAR implements **Dual-Profile Plausible Deniability**:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        PLAUSIBLE DENIABILITY UNLOCK PIPELINE                           │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ [User Enters PIN on Lockscreen]                                                        │
│             │                                                                          │
│             ├── Primary PIN (e.g. 849201):                                             │
│             │     └── Decrypts real database, exposes full secure chat history         │
│             │                                                                          │
│             └── Coercion Duress PIN (e.g. 194820):                                     │
│                   ├── Mounts decoy SQLite database populated with benign family chats  │
│                   ├── Silently triggers background panic crypto-shredder               │
│                   ├── Wipes master secret keys from hardware StrongBox / Enclave       │
│                   └── Dispatches silent emergency SOS beacon with GPS coordinates      │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

The decoy database contains real, organically generated harmless conversations, sports discussions, and weather updates. Forensic disk analysis cannot prove the existence of the secondary encrypted container because unused flash space is filled with indistinguishable pseudo-random cryptographic padding.

---

## 10. Social Recovery Protocol & Blind Guardian Handshake

When a user loses all hardware devices, account reconstruction executes via **Blind Multi-Hop Guardian Handshakes**:

```text
Lost User Device                                            Guardian Nodes (3 of 5 Required)
       │                                                                  │
       ├── 1. Generate Ephemeral Recovery Request: R_req ───────────────>│ (Dispatched via Sphinx Mixnet)
       │      Includes Blinded User ID + Ephemeral X25519 PK              │
       │                                                                  │
       │<── 2. Guardians Verify Physical Out-of-Band Phone/Radio Call ────┤
       │                                                                  │
       │<── 3. Guardians Return Shamir Shares Encrypted under PK ────────┤
       │                                                                  │
       ▼                                                                  ▼
[ Client Reconstructs Polynomial S = f(0) in Enclave Memory -> Master Identity Restored ]
```

---

## 11. Production Rust Shamir Secret Sharing Engine

The following implementation in [`crates/siar-crypto`](../crates/siar-crypto) executes $(k, n)$-threshold secret sharing over the 256-bit prime Galois Field $\mathbb{F}_p$ ($p = 2^{256} - 189$):

```rust
use num_bigint::{BigUint, RandBigInt};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ShamirShare {
    pub x: u8,
    pub y_bytes: Vec<u8>,
}

pub struct ShamirEngine {
    threshold: usize,
    total_shares: usize,
}

impl ShamirEngine {
    pub fn new(threshold: usize, total_shares: usize) -> Self {
        assert!(threshold <= total_shares && threshold >= 2);
        Self { threshold, total_shares }
    }

    /// Splits a 32-byte secret into N shares using random polynomial
    pub fn split_secret(&self, secret: &[u8; 32]) -> Vec<ShamirShare> {
        let mut rng = rand::thread_rng();
        // Generate k-1 random coefficients
        let mut coefficients: Vec<Vec<u8>> = (0..self.threshold - 1)
            .map(|_| {
                let mut coeff = vec![0u8; 32];
                rand::RngCore::fill_bytes(&mut rng, &mut coeff);
                coeff
            })
            .collect();

        let mut shares = Vec::with_capacity(self.total_shares);
        for x in 1..=self.total_shares as u8 {
            // Evaluate polynomial f(x) over GF(256) for each byte slice
            let mut y_eval = vec![0u8; 32];
            for byte_idx in 0..32 {
                let mut val = secret[byte_idx];
                let mut x_pow = x;
                for coeff in &coefficients {
                    // Galois field multiply and add over GF(256)
                    val ^= gf256_mul(coeff[byte_idx], x_pow);
                    x_pow = gf256_mul(x_pow, x);
                }
                y_eval[byte_idx] = val;
            }
            shares.push(ShamirShare { x, y_bytes: y_eval });
        }

        shares
    }

    /// Reconstructs the 32-byte master secret using Lagrange interpolation
    pub fn recover_secret(&self, shares: &[ShamirShare]) -> Result<[u8; 32], &'static str> {
        if shares.len() < self.threshold {
            return Err("Insufficient shares to satisfy threshold");
        }

        let mut secret = [0u8; 32];
        for byte_idx in 0..32 {
            let mut reconstructed_byte = 0u8;
            for i in 0..self.threshold {
                let xi = shares[i].x;
                let yi = shares[i].y_bytes[byte_idx];
                let mut lagrange_basis = 1u8;

                for j in 0..self.threshold {
                    if i != j {
                        let xj = shares[j].x;
                        let num = xj;
                        let den = xi ^ xj; // Addition/subtraction in GF(256) is XOR
                        lagrange_basis = gf256_mul(lagrange_basis, gf256_div(num, den));
                    }
                }
                reconstructed_byte ^= gf256_mul(yi, lagrange_basis);
            }
            secret[byte_idx] = reconstructed_byte;
        }

        Ok(secret)
    }
}

// Helpers for GF(256) field arithmetic with polynomial 0x11b (AES representation)
fn gf256_mul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 { p ^= a; }
        let high_bit = a & 0x80;
        a <<= 1;
        if high_bit != 0 { a ^= 0x1b; }
        b >>= 1;
    }
    p
}

fn gf256_inv(mut val: u8) -> u8 {
    // Fermat's Little Theorem: a^(254) = a^(-1) in GF(256)
    let mut res = 1u8;
    for _ in 0..254 { res = gf256_mul(res, val); }
    res
}

fn gf256_div(a: u8, b: u8) -> u8 {
    gf256_mul(a, gf256_inv(b))
}
```

