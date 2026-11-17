# SIAR Security Architecture: Adversarial Mesh Resilience & Threat Mitigation

**Document Version:** 1.0.0  
**Status:** Approved Architecture Reference  
**Scope:** Core Mesh Security, Byzantine Peer Resistance, Flood Prevention, Replay Defense, Identity Spoofing

---

## 1. Executive Summary

A common question in decentralized, peer-to-peer (P2P) systems is:

> *"Does SIAR prevent exploitation if a bad actor modifies the core code (not just the UI/client), joins the mesh, and attempts to flood the network, replay/duplicate messages, or spoof user and device identities?"*

**The short answer is yes.**

In decentralized networks, distinguishing between a "modified UI client" and a "modified core engine" is an illusion. Any adversary has full physical and software control over their own hardware: they can clone the open-source repository, modify any Rust crate, strip out all local checks and rate limits, compile a hostile binary, and inject arbitrary raw frames into Bluetooth, Wi-Fi, or IP interfaces.

SIAR's security model is designed precisely for this scenario under an **Adversarial / Zero-Trust Threat Model**:

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│                       ZERO-TRUST PERIMETER PRINCIPLE                         │
├──────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  [ Malicious Node ]                     [ Honest Node ]                      │
│  - Runs modified core                   - Runs unmodified SIAR core          │
│  - Strips local TokenBuckets            - Ingress TokenBucket active         │
│  - Sets hop_limit = 255                 - Decrements hop_limit; drops at 0   │
│  - Blasts 100,000 pkts/s                - Quota throttles to 5 req/s (Drop)  │
│  - Claims victim's DeviceId             - Ed25519 signature verify FAILS    │
│  - Replays old packets                  - ReplayGuard & SeenBundles DROP     │
│                            ═══════════>                                      │
│                            Radio / Wire                                      │
│                                                                              │
│  Rule: Security never relies on the sender's code. Every honest node         │
│        independently validates, authenticates, and throttles all ingress.    │
└──────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Threat Model: The "Hostile Modified Core" Node

As formalized in [Spec 28: Production Security Architecture](file:///home/irshad/Projects/siar/sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md#L53-L75) and [Spec 06: DTN Store-Carry-Forward](file:///home/irshad/Projects/siar/sys-arch/06-dtn-store-carry-forward-architecture.md#L2441-L2455):

* **Assumed Hostile or Compromised:**
  * Any nearby Bluetooth/Wi-Fi mesh neighbor
  * Any DTN carrier mule or store-carry-forward node
  * Any public relay server or Iroh endpoint
  * Any third-party plugin or foreign FFI caller
  * Stolen physical devices or restored old backups
* **Trust Roots (Zero Blind Trust):**
  * The local unlocked device's secure keystore
  * Explicitly authorized companion devices linked to the account
  * Cryptographically verified contacts (established out-of-band via QR/NFC)
  * Explicit organization/authority root public keys

Under this model, **all data received over any network or radio interface is treated as hostile input until mathematically proven otherwise.**

---

## 3. Defense Deep Dives

### 3.1. Preventing Mesh Flooding & Denial of Service (DoS)

#### Attack Scenario
An attacker modifies [`crates/siar-resource-limits`](file:///home/irshad/Projects/siar/crates/siar-resource-limits) on their local node, strips out token buckets, removes request throttles, sets arbitrary hop limits, and blasts high-frequency packets into the local radio mesh.

#### Defensive Layers Enforced by Honest Nodes

1. **Ingress Token Buckets & Trust-Tiered Quotas** ([`siar-resource-limits::peer_quota`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/peer_quota.rs)):
   * When an unknown peer connects, the honest node's local admission controller assigns it to [`TrustClass::Unknown`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/peer_quota.rs#L18).
   * **Unknown Default Caps:**
     * `max_requests_per_sec`: **5 requests/second** (enforced by local [`TokenBucket`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/token_bucket.rs))
     * `max_inbound_bytes`: **4 MB total**
     * `max_dtn_relay_bytes`: **1 MB total**
     * `max_active_streams`: **4**
   * Packets exceeding these rates are dropped at ingress with `DropReason::RateExceeded` before hitting memory or disk.
   * **Rule (§30):** *"Never make trusted peers unlimited."* Even local and organization devices have bounded quotas.
2. **Priority Queue Isolation (Weighted Fair Queueing)** ([`siar-resource-limits::queue`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/queue.rs), [`bandwidth_fairness`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/bandwidth_fairness.rs)):
   * Traffic is routed into six isolated priority queues: `Critical`, `Control`, `Interactive`, `RealtimeMedia`, `Normal`, and `Bulk`.
   * Unauthenticated or bulk flood traffic cannot consume buffer slots or bandwidth shares allocated to `Control` or `Critical` operations.
3. **Hop Limit & Monotonic Decrement** ([`siar-dtn-bundle::bundle`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/bundle.rs#L71-L89)):
   * Even if an attacker injects a bundle with `hop_limit = 255`, every honest node decrements `hop_limit -= 1` on each hop (`DtnBundle::forwarded()`).
   * When `hop_limit == 0`, the packet is unconditionally dropped.
4. **Controlled Replication (Spray-and-Wait vs. Epidemic Flooding)** ([`siar-dtn-bundle::spray`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/spray.rs), [`forwarding`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/forwarding.rs#L24-L34)):
   * Spec 06 §24 explicitly rejects epidemic flooding as a default: *"Send everything to everyone... unsuitable... battery drain, bandwidth waste, storage explosion."*
   * Honest nodes strictly enforce **Spray-and-Wait** replication budgets. A bundle carries a finite `replication_budget`. Once exhausted, carriers hold the bundle for direct delivery only and will not spray it to new peers.
5. **Bounded Wire Limits** ([`siar-protocol::limits`](file:///home/irshad/Projects/siar/crates/siar-protocol/src/limits.rs), [`siar-protocol-ext::framing`](file:///home/irshad/Projects/siar/crates/siar-protocol-ext/src/framing.rs)):
   * Remote frames are bounded (`MAX_TEXT_FRAME_BYTES = 64KB`, `MAX_CONTROL_FRAME_BYTES = 256KB`).
   * Remote headers indicating hostile oversized lengths are rejected before allocating memory buffers.

---

### 3.2. Preventing Message Duplication & Replay Attacks

#### Attack Scenario
An attacker sniffs legitimate, signed envelopes traversing the mesh and replays them thousands of times across different mesh paths, direct connections, and DTN carriers.

#### Defensive Layers Enforced by Honest Nodes

1. **Mesh Forwarding Deduplication** ([`siar-dtn-bundle::dedup::SeenBundles`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/dedup.rs)):
   * Every honest relay node maintains a bounded LRU cache of recently processed [`BundleId`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/types.rs)s.
   * `check_and_record()` inspects the ID. If already seen, the bundle is dropped immediately (`return true;`). It is never forwarded or processed twice, preventing broadcast storm formation.
2. **Cryptographic Sliding-Window Replay Guard** ([`siar-crypto::replay::ReplayGuard`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/replay.rs)):
   * At the final recipient, messages are authenticated and tracked per stream key: `(conversation, sender_device, epoch)`.
   * A bounded sliding window checks sequence counters:
     * Duplicate counter within window $\rightarrow$ Rejected as [`ReplayError::Duplicate`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/replay.rs#L26).
     * Stale counter behind the window $\rightarrow$ Rejected as [`ReplayError::TooOld`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/replay.rs#L28).
   * A security epoch advance (e.g., after key rotation or device revocation) starts a clean sequence space, invalidating all past stream counters.
3. **State & Event Idempotency** ([`siar-identity-multidevice::reconciliation`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/reconciliation.rs#L140-L208)):
   * [`EventDeduplicator`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/reconciliation.rs#L140) hashes incoming event bytes using **Blake3**. Duplicate deliveries are guaranteed no-ops.
   * [`ReplayProtectionIndex`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/reconciliation.rs#L179) enforces generation monotonicity. Replaying older directory snapshots triggers [`IdentityError::RollbackRejected`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/error.rs).
4. **Crash-Safe Outbox/Inbox Deduplication** ([`siar-crash-recovery::messaging_recovery`](file:///home/irshad/Projects/siar/crates/siar-crash-recovery/src/messaging_recovery.rs)):
   * Deduplicates `MessageId` and `EventId` across device restarts, retransmissions, and crash-before-ACK scenarios.

---

### 3.3. Preventing Identity Spoofing & Impersonation

#### Attack Scenario
An attacker modifies their core code to set the `sender` field of an envelope or packet to a victim’s `DeviceId` or `AccountId`, or advertises transport endpoints belonging to another user.

#### Defensive Layers Enforced by Honest Nodes

1. **Cryptographic Identity Roots (Ed25519 / X25519)** ([`siar-crypto::identity`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/identity.rs), [`siar-identity-multidevice::root_key`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/root_key.rs)):
   * Accounts and devices have no concept of phone numbers, SMS, or usernames as security roots.
   * Identity is rooted strictly in local cryptographic keypairs (Ed25519 signing + X25519 key agreement).
   * **Modifying core code does not produce private keys.** An attacker cannot compute a valid Ed25519 signature without the victim's private key, which is kept in hardware keystores (Android Keystore, Apple Secure Enclave, or TPM).
   * Honest nodes run [`verifying_key.verify(message, signature)`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/identity.rs#L58-L60). Forged signatures immediately error with `CryptoError::InvalidSignature`.
2. **Two-Hop Key Binding Verification** ([`siar-identity-multidevice::transport_key_binding`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/transport_key_binding.rs)):
   * A device cannot claim a transport endpoint (IP address, BLE MAC, or route token) without a [`TransportKeyBinding`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/transport_key_binding.rs#L58) signed by the device's certified signing key.
   * That device key must be signed by the account's root identity key via a [`DeviceCertificate`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/certificate.rs).
   * **Security Invariant 1:** *"No authorization without a valid signature."* Directories signed by illegitimate keys are rejected outright.
   * **Security Invariant 4:** *"Transport identity cannot substitute for account identity."* Trust is strictly tied to directory credentials, never to transport endpoints.
3. **End-to-End Encryption & MLS Tree-KEM** ([`siar-crypto-mls`](file:///home/irshad/Projects/siar/crates/siar-crypto-mls), [`siar-protocol::v1`](file:///home/irshad/Projects/siar/crates/siar-protocol/src/v1.rs#L47-L75)):
   * Group conversations use RFC 9420 Messaging Layer Security (MLS).
   * Intermediary relays only handle opaque ciphertexts.
   * Without group epoch secrets, an attacker running modified code cannot decrypt messages or forge legitimate MLS application frames.
4. **Device Clone Detection** ([`siar-crypto::clone_detection`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/clone_detection.rs)):
   * Every active installation generates an ephemeral [`DeviceInstanceId`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/clone_detection.rs#L24).
   * If an attacker clones a device image or restores a backup concurrently, [`CloneDetector`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/clone_detection.rs#L73) detects the mismatch and triggers isolation.
5. **Immediate Cryptographic Revocation** ([`siar-identity-multidevice::revocation`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/revocation.rs), [`session_cache`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/session_cache.rs)):
   * If a device key is compromised, any companion device signs a **Device Revocation Certificate**.
   * Honest nodes add the device to their [`RevocationCache`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/session_cache.rs#L28), advance the MLS epoch, and permanently reject subsequent connections from that device.
6. **Sybil Resistance** ([`sys-arch/39`](file:///home/irshad/Projects/siar/sys-arch/39-mixnet-directory-node-admission-identity-sybil-resistance-topology-governance-architecture.md#L426-L550)):
   * An attacker can generate 10,000 Ed25519 keypairs locally.
   * In SIAR, newly generated identities receive **zero trust**: they are quarantined in `TrustClass::Unknown` with heavy throughput constraints and cannot access private conversations or contact lists without explicit out-of-band verification (QR/NFC pairing or verified invite tokens).

---

## 4. Byzantine & Physical Limits: What Software Can and Cannot Prevent

In distributed systems, certain physical-layer and Byzantine behaviors cannot be prevented by software alone. SIAR documents and mitigates these transparently:

| Threat Vector | Can Software Stop It? | SIAR Architectural Mitigation |
| :--- | :---: | :--- |
| **Identity Spoofing** | **Yes** | Ed25519 digital signatures, two-hop key bindings, MLS Tree-KEM. |
| **Packet Tampering** | **Yes** | AEAD ciphertexts, Blake3 payload integrity hashes. |
| **Mesh Broadcast Storms** | **Yes** | `SeenBundles` LRU deduplication, hop-count decrement, Spray-and-Wait budgets. |
| **Replay Attacks** | **Yes** | `ReplayGuard` sliding windows, epoch ratchets, monotonic state generations. |
| **Mesh Ingress Flooding** | **Yes** | TokenBucket rate limits, per-peer quotas, Weighted Fair Queueing (WFQ). |
| **"Black Hole" Drop Attack**<br>*(Rogue node accepts a bundle then deletes it)* | **Mitigated** | **Software cannot force remote hardware to transmit.** Mitigated by **Multi-path routing** ([`sys-arch/12`](file:///home/irshad/Projects/siar/sys-arch/12-multipath-networking-architecture.md)), **Spray-and-Wait carrier redundancy** ([`siar-dtn-bundle::spray`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/spray.rs)), and delivery ACKs. |
| **Sybil Flood of Unknown IDs**<br>*(Attacker generates 1,000 new keypairs)* | **Mitigated** | **Anyone can compute keypairs.** Mitigated by quarantining all unknown IDs in `TrustClass::Unknown` with strict quotas (5 req/s) and zero access to private groups. |
| **Physical RF Airwave Jamming**<br>*(Attacker blasts 2.4 GHz noise)* | **Mitigated** | **Physical-layer physics cannot be solved in code.** Mitigated by **Multi-transport failover**: automatically switching between BLE, Wi-Fi Aware, Wi-Fi Direct, Classic BT, and IP/Iroh relays. |

---

## 5. Architectural Threat Matrix

| Threat Category | Invariant / Rule | Mitigating Module | Result on Attack Attempt |
| :--- | :--- | :--- | :--- |
| **Rogue Ingress Flood** | §29 "Per-Peer Quotas"<br>§30 "Trust-Aware Quotas" | [`siar-resource-limits::peer_quota`](file:///home/irshad/Projects/siar/crates/siar-resource-limits/src/peer_quota.rs) | Throttled by TokenBucket; dropped with `DropReason::RateExceeded`. |
| **Hop Limit Bypass** | §21 "Hop Limit Decrement" | [`siar-dtn-bundle::bundle`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/bundle.rs) | Decremented on arrival; dropped at 0. |
| **Epidemic Mesh Storm** | §24 "Epidemic Routing Rejected" | [`siar-dtn-bundle::forwarding`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/forwarding.rs) | Spray-and-Wait replication budget bounds copies. |
| **Duplicate Mesh Storm** | §189 "Deduplication" | [`siar-dtn-bundle::dedup`](file:///home/irshad/Projects/siar/crates/siar-dtn-bundle/src/dedup.rs) | `SeenBundles` LRU cache drops duplicate `BundleId`. |
| **Message Replay** | Part 28 §16 "Replay Protection" | [`siar-crypto::replay`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/replay.rs) | `ReplayGuard` sliding window rejects duplicate or old counters. |
| **State Snapshot Rollback** | §120 "Rollback Protection" | [`siar-identity-multidevice::trust_store`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/trust_store.rs) | Monotonic generation check rejects older state (`RollbackRejected`). |
| **Sender Identity Spoofing** | §6 "Cryptographic Identity" | [`siar-crypto::identity`](file:///home/irshad/Projects/siar/crates/siar-crypto/src/identity.rs) | Signature check fails (`CryptoError::InvalidSignature`). |
| **Transport Endpoint Hijack** | Invariant 4 "Transport != Account" | [`siar-identity-multidevice::transport_key_binding`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/transport_key_binding.rs) | Requires root-signed certificate chain; rejected without signature. |
| **Eavesdropping on Relay** | §8 "Opaque Relaying" | [`siar-protocol::mesh`](file:///home/irshad/Projects/siar/crates/siar-protocol/src/mesh.rs), [`siar-crypto-mls`](file:///home/irshad/Projects/siar/crates/siar-crypto-mls) | Relays see only ciphertext; MLS Tree-KEM protects group messages. |
| **Compromised/Stolen Device** | §159 "Stolen Device Threat" | [`siar-identity-multidevice::session_cache`](file:///home/irshad/Projects/siar/crates/siar-identity-multidevice/src/session_cache.rs) | Signed revocation certificate ratchets epoch and permanently rejects device. |
| **Hostile Framing Allocation** | Plan.md §61 "Decode Limits" | [`siar-protocol-ext::framing`](file:///home/irshad/Projects/siar/crates/siar-protocol-ext/src/framing.rs) | Opaque frames exceeding byte limits rejected before allocation. |

---

## 6. Conclusion

SIAR is engineered on the premise that **the network is hostile and remote peers may run arbitrary, malicious code.** 

Modifying SIAR's core code only alters the behavior of the attacker's local machine. The moment an adversarial node transmits packets across radio or IP links, it is intercepted by honest nodes whose unmodified admission controllers, signature verifiers, deduplicators, and replay guards neutralize the attack at ingress.
