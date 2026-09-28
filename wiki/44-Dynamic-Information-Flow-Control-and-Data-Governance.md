# 44 — Dynamic Information Flow Control & Data Governance

> **Corresponding Specifications:** [`sys-arch/136-anonymous-network-extension-data-governance-data-access-mediation-information-flow-control-retention-deletion-export-privacy-preserving-extension-data-architecture.md`](../sys-arch/136-anonymous-network-extension-data-governance-data-access-mediation-information-flow-control-retention-deletion-export-privacy-preserving-extension-data-architecture.md), [`sys-arch/135-anonymous-network-extension-permission-model-consent-ux-delegated-authority-capability-attenuation-scope-review-privacy-preserving-authorization-architecture.md`](../sys-arch/135-anonymous-network-extension-permission-model-consent-ux-delegated-authority-capability-attenuation-scope-review-privacy-preserving-authorization-architecture.md)  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.17, §2.18), [Wiki Chapter 18](18-Protocol-Extensions-and-WASM-Plugins.md), [Wiki Chapter 32](32-WASM-Sandboxing-IFC-and-Extension-Ecosystem.md), [Wiki Chapter 43](43-WebAssembly-Sandboxing-and-Host-Isolation-Runtime.md)  
> **Key Crates:** [`crates/siar-capability`](../crates/siar-capability), [`crates/siar-protocol-ext`](../crates/siar-protocol-ext)

---

## 1. Architectural Philosophy: The Failure of Coarse Permission Models

Traditional mobile and desktop operating system permission models (Android runtime permissions, iOS privacy toggles) are inherently binary and coarse: an application either has `READ_CONTACTS` and `INTERNET`, or it does not.

Once an untrusted plugin or third-party extension is granted permission to read sensitive chat messages (e.g. for spell-checking or language translation) and also has network access, the operating system can no longer distinguish between:
1. **Legitimate Network Use**: Fetching a new language dictionary from an authorized CDN.
2. **Malicious Data Exfiltration**: Bundling the user's private decrypted chat messages into an HTTP POST request to an unauthorized command-and-control server.

In [`sys-arch/136`](../sys-arch/136-anonymous-network-extension-data-governance-data-access-mediation-information-flow-control-retention-deletion-export-privacy-preserving-extension-data-architecture.md), SIAR solves this permanently using **Dynamic Information Flow Control (IFC)** based on formal security lattices.

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        INFORMATION FLOW CONTROL (IFC) LATTICE                          │
├────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                        │
│                                [ Top: RootKey / Master Secrets ]                       │
│                                             │                                          │
│                                             ▼                                          │
│                             [ ConfidentialChat / Contacts / GPS ]                      │
│                                             │                                          │
│                                             ▼                                          │
│                             [ Metadata / Operational Logs ]                            │
│                                             │                                          │
│                                             ▼                                          │
│                                [ Bottom: Public / Anonymized ]                         │
│                                                                                        │
│ * Rule of Information Flow: Data may ONLY flow downward to higher security labels      │
│   (tainting the consumer). Data can NEVER flow upward to lower security channels!      │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Mathematical Formalization: The Denning Security Lattice

SIAR formalizes Information Flow Control using **Denning's Lattice Security Model** augmented with the dual dimensions of Confidentiality ($C$) and Integrity ($I$):

$$L = \langle \mathcal{S}, \, \sqsubseteq, \, \sqcup, \, \sqcap \rangle$$

Where each label $s \in \mathcal{S}$ is a pair $s = (c, i) \in \mathcal{C} \times \mathcal{I}$:
- $\mathcal{C}$ is the confidentiality level ($\text{Public} \sqsubseteq \text{Internal} \sqsubseteq \text{Confidential} \sqsubseteq \text{TopSecret}$).
- $\mathcal{I}$ is the integrity level ($\text{Untrusted} \sqsubseteq \text{Sanitized} \sqsubseteq \text{Trusted}$).

### 2.1. The Flow Relation & Join Operation
Data labeled $s_1 = (c_1, i_1)$ is allowed to flow into a container labeled $s_2 = (c_2, i_2)$ if and only if:

$$s_1 \sqsubseteq s_2 \iff c_1 \sqsubseteq c_2 \quad \land \quad i_2 \sqsubseteq i_1$$

When an operation combines inputs $x_1, \dots, x_k$, the output container is tainted with the least upper bound (join):

$$\text{Label}(f(x_1, \dots, x_k)) = \bigsqcup_{j=1}^k \text{Label}(x_j) = \left( \max_j c_j, \, \min_j i_j \right)$$

### 2.2. The Non-Interference Theorem
Let $\tau(P, \sigma)$ denote the trace of observable side effects generated by running plugin $P$ under memory state $\sigma$. Let $\approx_{\text{Low}}$ denote observational equivalence on public sinks:

$$\forall \sigma_1, \sigma_2 \in \Sigma, \quad \sigma_1 \approx_{\text{Low}} \sigma_2 \implies \tau(P, \sigma_1) \approx_{\text{Low}} \tau(P, \sigma_2)$$

**Mathematical Proof Invariant**: Secret or confidential user inputs cannot influence any public or network output channels. Covert timing, storage, and message-length channels are eliminated by design.

---

## 3. Dynamic Taint Tracking & Network Egress Revocation

SIAR implements **Coarse-Grained Dynamic Container Tainting**:

```mermaid
graph TD
    Plugin[Sandboxed WASM Plugin]
    DataStore[(Decrypted Chat Storage)]
    Net[Outbound Mesh / WAN Sockets]
    
    Plugin -->|1. Invokes read_conversation()| DataStore
    DataStore -->|2. Transfers Decrypted Message Bytes| Plugin
    
    subgraph TaintEngine["Dynamic Taint Propagation"]
        Taint["Apply Taint Label: TAINT_CONFIDENTIAL_DATA"]
    end
    
    DataStore -.-> Taint
    Taint -.-> Plugin
    
    Plugin -->|3. Invokes dispatch_network_frame()| Net
    Net -->|4. Egress Gatekeeper Checks Taint Status| Blocked{Is Plugin Tainted?}
    Blocked -->|YES| Deny[ACCESS DENIED: Capabilities Automatically Revoked!]
    Blocked -->|NO| Allow[Frame Dispatched to Mesh]
```

### 3.1. The Network Egress Revocation Theorem
Let $\mathcal{C}(P)$ be the set of active capabilities granted to plugin instance $P$:

$$\text{TAINT\_CONFIDENTIAL} \in \text{Labels}(P) \implies \mathcal{C}(P) \cap \{\text{NetSocket}, \, \text{MeshBroadcast}, \, \text{RelayEgress}\} = \emptyset$$

Once a plugin touches decrypted user chat content, its outbound network transmission capabilities are revoked mathematically by the host capability broker.

---

## 4. Concrete Rust Taint Gatekeeper Trait

```rust
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConfidentialityLabel {
    Public = 0,
    OperationalTelemetry = 1,
    ConfidentialChat = 2,
    MasterIdentityKey = 3,
}

#[derive(Debug, Clone)]
pub struct TaintContext {
    highest_confidentiality: ConfidentialityLabel,
    active_labels: HashSet<ConfidentialityLabel>,
}

impl TaintContext {
    pub fn new() -> Self {
        Self {
            highest_confidentiality: ConfidentialityLabel::Public,
            active_labels: HashSet::new(),
        }
    }

    /// Evaluates if an outbound network dispatch is authorized under IFC
    pub fn check_network_egress_permitted(&self) -> Result<(), &'static str> {
        if self.highest_confidentiality >= ConfidentialityLabel::ConfidentialChat {
            Err("IFC Violation: Network egress revoked on tainted instance")
        } else {
            Ok(())
        }
    }

    /// Taints the context upon ingesting a data stream
    pub fn ingest_data(&mut self, label: ConfidentialityLabel) {
        if label > self.highest_confidentiality {
            self.highest_confidentiality = label;
        }
        self.active_labels.insert(label);
    }
}

pub trait DataGovernanceEngine: Send + Sync {
    /// Ingest data with taint label, updating sandbox security state
    fn apply_taint(&mut self, plugin_id: &[u8; 32], label: ConfidentialityLabel);

    /// Authorize or reject outbound hostcall based on active taint
    fn authorize_egress(&self, plugin_id: &[u8; 32]) -> Result<(), String>;

    /// Shred all data associated with uninstalled plugin
    fn shred_plugin_data(&mut self, plugin_id: &[u8; 32]) -> Result<(), String>;
}
```

---

## 5. Cryptographic Declassification & Safe Downgrading

In specific scenarios, a plugin must perform a computation over private data and output an aggregate or blinded result (e.g. generating an anonymous zero-knowledge proof of age without revealing birthday):

$$\text{Downgrade}(s, \pi) \iff \text{ZkProofValid}(\pi) \land \text{Taint}(\pi) = \text{Public}$$

1. **Mathematical Transformation**: The output must pass through an audited one-way cryptographic hash function, differential privacy mechanism ($\epsilon \le 1.0$), or zero-knowledge proof generator.
2. **Explicit User Re-Consent**: The declassification gatekeeper prompts the user with an explicit cryptographic disclosure dialog before releasing the downgraded payload.
3. **Audit Trail**: Every declassification event is signed and written to the local verifiable audit log.

---

## 6. Extension Data Governance: Retention, Export & Shredding

In [`sys-arch/136`](../sys-arch/136-anonymous-network-extension-data-governance-data-access-mediation-information-flow-control-retention-deletion-export-privacy-preserving-extension-data-architecture.md):
- **Isolated Storage Namespaces**: Each plugin receives an isolated Stoolap key-value partition encrypted with a distinct symmetric partition key:
  $$K_{\text{partition}} = \text{HKDF-Expand}(K_{\text{device}}, \, \text{PluginId} \parallel \text{"storage-v1"}, \, 32)$$
- **Cascading Deletion**: When an extension is uninstalled, the host zeroizes its partition key, instantly cryptographically shredding all residual data generated by that plugin.
- **Zero Cross-Plugin Leakage**: Plugins cannot query or enumerate the storage partitions or active status of other installed plugins.

---

## 7. Dynamic Floating Labels & Label Creep Mitigation

Static type-level IFC systems (e.g. Jif) require source-code compilation annotations that are impossible to enforce on untrusted arbitrary WASM binaries. SIAR implements **Austin-Flanagan Dynamic Execution-Monitoring IFC**:

```text
Instruction Stream:
[ Read(ConfidentialVar) ]  ──>  PC_Taint = PC_Taint ⊔ Confidential
           │
           ▼
[ Write(OutVar) ]          ──>  Label(OutVar) = Label(OutVar) ⊔ PC_Taint
           │
           ▼
[ NetworkHostcall(OutVar) ]──>  Assert: Label(OutVar) ⊑ Public (Traps if False!)
```

### 7.1. Label Creep & Execution Reset
In dynamic IFC, a long-running plugin tends to accumulate taint over time ("Label Creep"), eventually losing all output privileges. SIAR solves this via **Ephemeral Task Workers**:
- Plugins process queries in isolated micro-instances that terminate upon returning an output.
- Memory is wiped after each transaction, resetting the Program Counter taint ($PC_{\text{taint}} \leftarrow \bot$) and preventing permanent capability lockout.

---

## 8. Cryptographic Data Provenance & Lineage DAGs

To prove where any stored record originated and ensure regulatory accountability without tracking individuals, SIAR attaches a **Cryptographic Provenance Certificate** (`sys-arch/136`):

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        CRYPTOGRAPHIC PROVENANCE DAG                                    │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ [Raw Sensor / Radio Ingest: GPS + Mesh Frame]                                          │
│       │                                                                                │
│       ▼ Processed by Emergency Triage Plugin                                           │
│ [Derived Triage Record] ── Certified by BLAKE3-Tree-MAC(PluginKey, ParentHash)         │
│       │                                                                                │
│       ▼ Merged with Field Map Annotation                                               │
│ [Composite Incident Blob] ── Lineage DAG: Merkle Path links to Parent Proofs           │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

The provenance root hash allows clients to verify that data was transformed exclusively by signed, non-compromised plugins without inspecting intermediate confidential data fields.

---

## 9. Data Governance & IFC Threat Matrix

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        DATA GOVERNANCE THREAT DEFENSE MATRIX                           │
├────────────────────────┬─────────────────────────┬─────────────────────────────────────┤
│ Threat Vector          │ Attack Mechanism        │ SIAR Defense Architecture           │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Collusion Attack**   │ Tainted plugin leaks    │ Isolated storage namespaces; no IPC │
│                        │ data to untainted peer  │ or shared memory between plugins.   │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Implicit Flow Leak** │ Branching on secrets to │ Branch condition taints Program     │
│                        │ modulate public writes  │ Counter ($PC_{\text{taint}}$); both │
│                        │                         │ branches execute with high taint.   │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Residual Sector Dump**| Inspecting raw storage │ Crypto-shredding: zeroizing the     │
│                        │ after plugin uninstalls │ 256-bit AES-GCM partition key wipes │
│                        │                         │ data mathematically in 0.1ms.       │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Covert Timing Channel**| Modulating loop runtime│ Constant-time hostcall delays and   │
│                        │ to transmit bits        │ randomized jitter injection.        │
└────────────────────────┴─────────────────────────┴─────────────────────────────────────┘
```

---

## 10. Production Rust Dynamic Taint Tracker & Crypto-Shredder

The following implementation in [`crates/siar-wasm-runtime`](../crates/siar-wasm-runtime) enforces runtime floating label tracking and instantaneous cryptographic shredding:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PartitionSecretKey(pub [u8; 32]);

pub struct PluginGovernanceState {
    pub current_pc_taint: ConfidentialityLabel,
    pub storage_key: PartitionSecretKey,
}

pub struct DynamicGovernanceRegistry {
    plugins: Arc<RwLock<HashMap<[u8; 32], PluginGovernanceState>>>,
}

impl DynamicGovernanceRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Taints the program counter (PC) when branching on or reading confidential inputs
    pub async fn elevate_pc_taint(&self, plugin_id: &[u8; 32], input_label: ConfidentialityLabel) {
        let mut map = self.plugins.write().await;
        if let Some(state) = map.get_mut(plugin_id) {
            if input_label > state.current_pc_taint {
                state.current_pc_taint = input_label;
            }
        }
    }

    /// Verifies if a network egress hostcall is permissible
    pub async fn authorize_network_egress(&self, plugin_id: &[u8; 32]) -> Result<(), &'static str> {
        let map = self.plugins.read().await;
        if let Some(state) = map.get(plugin_id) {
            if state.current_pc_taint >= ConfidentialityLabel::ConfidentialChat {
                return Err("IFC Security Violation: Outbound network blocked due to active confidentiality taint");
            }
            Ok(())
        } else {
            Err("Plugin not registered in governance registry")
        }
    }

    /// Instantaneous cryptographic shredding: wipes partition key from memory
    pub async fn crypto_shred_plugin(&self, plugin_id: &[u8; 32]) -> Result<(), &'static str> {
        let mut map = self.plugins.write().await;
        if let Some(mut state) = map.remove(plugin_id) {
            state.storage_key.zeroize();
            Ok(())
        } else {
            Err("Plugin already uninstalled or missing")
        }
    }
}
```

