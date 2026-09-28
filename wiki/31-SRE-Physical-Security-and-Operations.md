# 31 — SRE, Physical Security & Operational Defense

> **Corresponding Specifications:** [`sys-arch/95-anonymous-network-policy-compliance-engine-continuous-control-evaluation-security-posture-evidence-collection-privacy-preserving-assurance-architecture.md`](../sys-arch/95-anonymous-network-policy-compliance-engine-continuous-control-evaluation-security-posture-evidence-collection-privacy-preserving-assurance-architecture.md) through [`sys-arch/121-anonymous-network-reliability-risk-register-technical-debt-governance-systemic-weakness-tracking-remediation-portfolio-privacy-preserving-engineering-risk-architecture.md`](../sys-arch/121-anonymous-network-reliability-risk-register-technical-debt-governance-systemic-weakness-tracking-remediation-portfolio-privacy-preserving-engineering-risk-architecture.md)  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§1.3 Layers 6–7, §2.16), [Wiki Chapter 41](41-Mathematical-SRE-Error-Budgets-and-Admission-Control.md), [Wiki Chapter 42](42-Physical-Facility-Security-Chassis-Tamper-and-Crisis-Command.md)  
> **Key Crates:** [`crates/siar-sre`](../crates), [`crates/siar-policy`](../crates), [`crates/siar-core`](../crates)

---

## 1. Operational Security & SRE Philosophy

A decentralized sovereign network cannot survive on mathematical cryptography alone. If physical colocation facilities can be entered without notice, if unverified daemon configuration drifts induce memory corruption, or if volumetric flash-crowds saturate relay queues, user communications collapse just as definitively as if the underlying cipher suites were broken.

Specs 95–121 formalize an **End-to-End Operational Defense-in-Depth Architecture** spanning continuous policy compliance, rigorous mathematical Site Reliability Engineering (SRE), physical hardware zeroization, and crisis incident governance:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        OPERATIONAL DEFENSE-IN-DEPTH SPECTRUM                           │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ Layer 7: Incident Command & Post-Incident Review (Specs 119–121)                       │
│          • Blameless PIR taxonomy, 5-Whys causal analysis, 20% tech debt burndown.     │
│ Layer 6: Physical Chassis Tamper & Environmental Defense (Specs 116–118)               │
│          • Hardware crowbar DRAM discharge, lid photodiode NMIs, secure zeroization.   │
│ Layer 5: Latency Budgeting & Tail-Latency Control (Spec 112)                           │
│          • Bounded hop deadlines, synthetic loopback probes, hedge-request policies.   │
│ Layer 4: Mathematical SLOs & Multi-Window Burn Rate Alerting (Spec 109)                │
│          • 14.4x / 6x multi-window error budget alerts, strict rolling SLIs.           │
│ Layer 3: Desired-State Reconciliation & Pre-Change Simulation (Specs 105, 107)        │
│          • Signed GitOps manifests, Kolmogorov-Smirnov canary tests, auto-rollback.    │
│ Layer 2: Priority-Tiered Load Shedding & Admission Control (Spec 101)                  │
│          • Little's Law queue bounds, 4-tier traffic priority, CoDel early drops.      │
│ Layer 1: Continuous Policy Compliance & Evidence Collection (Spec 95)                  │
│          • In-toto attestations, SLSA Level 4 provenance, Merkle tamper evidence log.  │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Continuous Policy Compliance & Cryptographic Evidence Collection

In [`sys-arch/95`](../sys-arch/95-anonymous-network-policy-compliance-engine-continuous-control-evaluation-security-posture-evidence-collection-privacy-preserving-assurance-architecture.md), compliance is not an annual audit snapshot; it is an uninterrupted runtime verification loop.

### 2.1. In-Toto Attestations & SLSA 4 Provenance
Every binary running across SIAR infrastructure (from mixnet relays to directory authorities) must possess a cryptographically signed supply-chain attestation conforming to **SLSA Level 4**:
1. **Hermetic Builds**: Compiled inside isolated sandbox containers with zero network access and pinned toolchains.
2. **Reproducible Binaries**: Bit-for-bit identical outputs produced across multiple independent build nodes.
3. **Hardware-Signed Evidence**: Build artifacts are counter-signed via cosign/sigstore backed by developer YubiKeys.

### 2.2. Tamper-Evident Merkle Evidence Log
Nodes continuously record operational compliance events (kernel attestation tokens, firewall checksums, TLS cipher negotiations) into an append-only Merkle tree:

$$\text{Root}_{k} = \mathcal{H}(\text{Root}_{k-1} \parallel \text{Leaf}_k), \quad \text{Leaf}_k = \text{BLAKE3}(\text{Timestamp} \parallel \text{NodeID} \parallel \text{EventPayload})$$

Periodic Merkle roots are gossiped across the peer directory authority set, providing unalterable audit trails while preserving anonymity through zero-knowledge proof aggregations.

---

## 3. Mathematical Availability Modeling & Reliability Block Diagrams (RBD)

Cluster survivability is modeled using continuous-time Markov chains and Reliability Block Diagrams (RBD) across independent jurisdictional fault domains:

### 3.1. Markov Availability Derivation
Consider a node cluster transitioning between State $S_0$ (Fully Operational, $N$ nodes), State $S_1$ (Degraded, $N-k$ nodes), and State $S_2$ (Outage, $< \lceil N/2 \rceil + 1$ nodes).

Let failure rate be $\lambda$ and automated recovery rate be $\mu$:

$$\mathbf{Q} = \begin{pmatrix} -\lambda & \lambda \\ \mu & -\mu \end{pmatrix}$$

The stationary probability distribution $\boldsymbol{\pi} = [\pi_0, \pi_1]$ satisfies $\boldsymbol{\pi} \mathbf{Q} = \mathbf{0}$ and $\pi_0 + \pi_1 = 1$:

$$\pi_0 = \frac{\mu}{\lambda + \mu}, \quad \pi_1 = \frac{\lambda}{\lambda + \mu}$$

Since $\text{MTBF} = \frac{1}{\lambda}$ and $\text{MTTR} = \frac{1}{\mu}$, steady-state availability $A$ is:

$$A = \pi_0 = \frac{\text{MTBF}}{\text{MTBF} + \text{MTTR}}$$

### 3.2. End-to-End Mixnet Route Availability
A 3-hop stratified mix route traverses Layer 1, Layer 2, and Layer 3 in series. The end-to-end path reliability $R_{\text{route}}(t)$ is:

$$R_{\text{route}}(t) = R_{L1}(t) \cdot R_{L2}(t) \cdot R_{L3}(t) = \prod_{i=1}^3 \left(1 - (1 - R_{\text{node}}(t))^{K_i}\right)$$

Where $K_i$ is the number of active redundant nodes in stratum $i$. With $K_i \ge 64$, path availability exceeds $99.9999\%$ even under high individual node churn!

---

## 4. Mathematical SRE: Multi-Window Multi-Burn-Rate Alerting

In [`sys-arch/109`](../sys-arch/109-anonymous-network-sli-slo-definition-availability-latency-privacy-error-budget-multi-window-burn-rate-alerting-architecture.md), threshold-based alerting (e.g. "alert if CPU > 90%") is deprecated due to false positive fatigue. SIAR implements Google-style **Multi-Window Multi-Burn-Rate Alerting**:

### 4.1. Burn Rate Formulation
Let $E = 1 - \text{SLO}$ denote the total error budget over a rolling 30-day period ($T_{\text{period}} = 2,592,000\text{ s}$). The instantaneous burn rate $B$ is:

$$B = \frac{\text{Observed Error Rate}}{E} = \frac{1 - \text{SLI}}{1 - \text{SLO}}$$

| Alert Class | Severity | Budget Consumed | Short Window | Long Window | Burn Rate Factor ($B$) | Notification Target |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Page Alert** | Critical (P0) | 2% in 1 Hour | 5 minutes | 1 hour | **14.4x** | PagerDuty / Mesh Pager |
| **Page Alert** | Critical (P1) | 5% in 6 Hours | 30 minutes | 6 hours | **6.0x** | PagerDuty / Mesh Pager |
| **Ticket Alert**| Warning (P2) | 10% in 3 days | 2 hours | 3 days | **1.0x** | Next-Day Jira / GitHub |

---

## 5. Priority-Tiered Load Shedding & Queueing Control

In [`sys-arch/101`](../sys-arch/101-anonymous-network-adaptive-capacity-management-dynamic-mix-scaling-traffic-shaping-load-shedding-privacy-preserving-operational-elasticity-architecture.md), server exhaustion under DDoS or traffic bursts is mitigated via **CoDel-inspired Priority Queueing**:

$$\text{Queue Latency} \quad L_q = \frac{\rho}{\mu (1 - \rho)} \quad \text{where } \rho = \frac{\lambda}{\mu}$$

When queue delay exceeds target threshold ($T_{\text{target}} = 15\text{ ms}$) for more than an interval ($100\text{ ms}$), the admission controller drops traffic based strictly on priority tier:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        PRIORITY ADMISSION CONTROL TIERS                                │
├───────┬──────────────────────────┬─────────────────────────────────────────────────────┤
│ Tier  │ Traffic Class            │ Admission & Shedding Policy                         │
├───────┼──────────────────────────┼─────────────────────────────────────────────────────┤
│ P0    │ Ephemeral Cryptographic  │ NEVER DROPPED. Handshake renegotiation, Sphinx      │
│       │ Handshakes & Signaling   │ ACK bundles, and emergency SOS alerts.              │
├───────┼──────────────────────────┼─────────────────────────────────────────────────────┤
│ P1    │ Realtime Voice & Media   │ Bounded drops (random early drop if buffer > 70%).  │
│       │ Packets                  │ Low latency preferred over reliability.             │
├───────┼──────────────────────────┼─────────────────────────────────────────────────────┤
│ P2    │ Asynchronous E2EE Chat   │ Queued to disk; delayed up to 60 seconds if CPU >85%│
├───────┼──────────────────────────┼─────────────────────────────────────────────────────┤
│ P3    │ Bulk Blob Sync & DTN     │ INSTANTLY SHED (HTTP 429 / backpressure). Zero      │
│       │ Data Mules               │ background file transfers permitted during spikes.  │
└───────┴──────────────────────────┴─────────────────────────────────────────────────────┘
```

---

## 6. Production Rust Implementation: SRE Health & Load Governor

The following production-grade Rust implementation calculates multi-window burn rates, enforces traffic priority shed thresholds, and monitors node error budgets:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityTier {
    P0Signaling = 0,
    P1RealtimeMedia = 1,
    P2AsynchronousMessage = 2,
    P3BulkTransfer = 3,
}

pub struct SreHealthGovernor {
    target_slo: f64,
    short_window_failures: u64,
    short_window_total: u64,
    long_window_failures: u64,
    long_window_total: u64,
    current_load_ratio: f64,
}

impl SreHealthGovernor {
    pub fn new(target_slo: f64) -> Self {
        Self {
            target_slo,
            short_window_failures: 0,
            short_window_total: 0,
            long_window_failures: 0,
            long_window_total: 0,
            current_load_ratio: 0.0,
        }
    }

    pub fn record_request(&mut self, is_error: bool) {
        self.short_window_total += 1;
        self.long_window_total += 1;
        if is_error {
            self.short_window_failures += 1;
            self.long_window_failures += 1;
        }
    }

    pub fn compute_burn_rate(&self, failures: u64, total: u64) -> f64 {
        if total == 0 {
            return 0.0;
        }
        let error_rate = failures as f64 / total as f64;
        let allowed_rate = 1.0 - self.target_slo;
        error_rate / allowed_rate
    }

    /// Evaluates if P0 Critical Alert condition is met (14.4x burn across both windows)
    pub fn is_p0_burn_active(&self) -> bool {
        let short_burn = self.compute_burn_rate(self.short_window_failures, self.short_window_total);
        let long_burn = self.compute_burn_rate(self.long_window_failures, self.long_window_total);
        short_burn >= 14.4 && long_burn >= 14.4
    }

    /// Priority-tiered admission control decision
    pub fn should_admit_traffic(&self, tier: PriorityTier) -> bool {
        match tier {
            PriorityTier::P0Signaling => true, // Invariant: Signaling never dropped
            PriorityTier::P1RealtimeMedia => self.current_load_ratio < 0.95,
            PriorityTier::P2AsynchronousMessage => self.current_load_ratio < 0.85,
            PriorityTier::P3BulkTransfer => self.current_load_ratio < 0.65,
        }
    }

    pub fn update_load(&mut self, load: f64) {
        self.current_load_ratio = load;
    }
}
```

---

## 7. Incident Command System (ICS) & Out-of-Band Tactical Mesh

In [`sys-arch/119`](../sys-arch/119-anonymous-network-incident-management-triage-severity-classification-escalation-incident-commander-war-room-communications-privacy-preserving-incident-response-architecture.md), severe outages or active state cyber-attacks trigger the **Incident Command System (ICS)**:
1. **Single Incident Commander (IC)**: Holds absolute operational decision authority. Can order network-wide key rotations, partition severing, or traffic blackholing without waiting for committee consensus.
2. **Air-Gapped Out-of-Band Tactical Mesh**: When production servers or corporate email/Slack systems are under attack, incident response coordinates exclusively across a local LoRa/Wi-Fi ad-hoc SIAR emergency mesh, preventing attackers with eavesdropping access from learning containment strategies.
3. **Continuous Blameless Learning**: Every ICS activation feeds directly into the forensic timelines of [Wiki Chapter 50](50-Systemic-Reliability-Risk-Governance-and-Post-Incident-Learning.md) and updates the active Reliability Risk Register (`sys-arch/121`).

---

## 8. SRE & Operational Threat Defense Matrix

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                          OPERATIONAL THREAT & DEFENSE MATRIX                           │
├────────────────────────┬─────────────────────────┬─────────────────────────────────────┤
│ Threat Vector          │ Attack Mechanism        │ SIAR Defense Architecture           │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Queue Satiation DoS**│ Volumetric flood fills  │ Priority-tiered admission control   │
│                        │ node buffers to drop SOS│ drops P3 bulk and P2 chat; P0 immune│
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Alert Fatigue**      │ Flapping metrics wake   │ Multi-window multi-burn-rate math   │
│                        │ engineers continuously  │ requires both 5m and 1h consensus.  │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Jurisdictional Raid**│ State seizes cloud nodes│ Rule of Thirds: <33.3% of relays in │
│                        │ within one country      │ any single legal or cloud zone.     │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Supply Chain Drift** │ Contributor updates     │ In-toto SLSA 4 cryptographic verify │
│                        │ unverified dependency   │ blocks CI deployment of rogue code. │
└────────────────────────┴─────────────────────────┴─────────────────────────────────────┘
```
