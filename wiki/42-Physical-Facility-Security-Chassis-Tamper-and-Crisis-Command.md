# 42 — Physical Facility Security, Chassis Tamper & Crisis Command

> **Corresponding Specifications:** [`sys-arch/116-anonymous-network-data-center-edge-site-facility-power-cooling-environmental-monitoring-privacy-preserving-physical-site-reliability-architecture.md`](../sys-arch/116-anonymous-network-data-center-edge-site-facility-power-cooling-environmental-monitoring-privacy-preserving-physical-site-reliability-architecture.md), [`sys-arch/117-anonymous-network-physical-security-tamper-detection-site-access-control-chain-of-custody-asset-protection-privacy-preserving-facility-security-architecture.md`](../sys-arch/117-anonymous-network-physical-security-tamper-detection-site-access-control-chain-of-custody-asset-protection-privacy-preserving-facility-security-architecture.md), [`sys-arch/118-anonymous-network-physical-disaster-preparedness-fire-flood-earthquake-response-site-evacuation-asset-salvage-emergency-logistics-privacy-preserving-facility-continuity-architecture.md`](../sys-arch/118-anonymous-network-physical-disaster-preparedness-fire-flood-earthquake-response-site-evacuation-asset-salvage-emergency-logistics-privacy-preserving-facility-continuity-architecture.md), [`sys-arch/119-anonymous-network-crisis-command-emergency-decision-authority-multi-team-coordination-communications-situation-awareness-privacy-preserving-incident-command-architecture.md`](../sys-arch/119-anonymous-network-crisis-command-emergency-decision-authority-multi-team-coordination-communications-situation-awareness-privacy-preserving-incident-command-architecture.md)  
> **Complements:** [`SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md`](../SIAR_SYSTEM_ARCHITECTURE_DESIGN_RATIONALE.md) (§2.16), [Wiki Chapter 31](31-SRE-Physical-Security-and-Operations.md), [Wiki Chapter 37](37-Hardware-Root-of-Trust-TPM-Measured-Boot-and-HSM.md)  
> **Key Crates:** [`crates/siar-sre`](../crates), [`crates/siar-crypto`](../crates), [`crates/siar-core`](../crates)

---

## 1. The Physical Threat Horizon: Beyond Network Packets

When cryptographic cipher suites and overlay routing protocols are mathematically impenetrable, sophisticated adversaries (nation-state intelligence agencies, militarized police units, hostile private intelligence contractors) bypass software defenses by attacking **physical infrastructure directly**:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        PHYSICAL ADVERSARY CAPABILITY MATRIX                           │
├───────┬──────────────────────────┬────────────────────────────┬────────────────────────┤
│ Tier  │ Adversary Profile        │ Attack Vectors             │ SIAR Defense Mechanism │
├───────┼──────────────────────────┼────────────────────────────┼────────────────────────┤
│ T1    │ Opportunistic Burglary / │ Chassis theft, disk        │ Full NVMe OPAL 2.0 SED │
│       │ Facility Intruders       │ extraction, USB malware    │ AES-256-XTS + TPM seal │
├───────┼──────────────────────────┼────────────────────────────┼────────────────────────┤
│ T2    │ Rogue Colocation Techs / │ PCIe bus interposers, BMC  │ Microswitches, lid     │
│       │ Malicious Insiders       │ firmware tampering, JTAG   │ photodiodes, BMC disable│
├───────┼──────────────────────────┼────────────────────────────┼────────────────────────┤
│ T3    │ Intelligence Agencies /  │ Interdiction shipping,     │ Hardware crowbar DRAM  │
│       │ Advanced Black-Bag Teams │ cold-boot liquid nitrogen  │ discharge (< 800 µs),  │
│       │                          │ memory chip desoldering    │ epoxy potted circuitry │
├───────┼──────────────────────────┼────────────────────────────┼────────────────────────┤
│ T4    │ Militarized State Raids /│ Armed kinetic facility     │ Automated Scorched-    │
│       │ Hostile Judicial Seizure │ entry, rapid power cut     │ Earth zeroization &    │
│       │                          │ and asset confiscation     │ instant key shredding  │
└───────┴──────────────────────────┴────────────────────────────┴────────────────────────┘
```

Specs 116–119 formalize SIAR's defense architecture extending sovereignty to **physical chassis hardware sensors, explosive sub-millisecond RAM zeroization, datacenter disaster resilience, and crisis incident command hierarchies**.

---

## 2. Chassis Tamper Circuitry & Sub-Millisecond RAM Zeroization

In [`sys-arch/117`](../sys-arch/117-anonymous-network-physical-security-tamper-detection-site-access-control-chain-of-custody-asset-protection-privacy-preserving-facility-security-architecture.md), sovereign relay nodes are enclosed in custom tamper-reactive server chassis:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        HARDWARE CHASSIS TAMPER PERIMETER                               │
├────────────────────────────────────────────────────────────────────────────────────────┤
│ [1U/2U Bare-Metal Server Chassis]                                                      │
│   ├── Light Sensors: High-sensitivity photodiodes (> 0.2 Lux trigger threshold).       │
│   ├── Chassis Switches: Dual micro-switches detecting top lid mechanical separation.  │
│   ├── 3-Axis MEMS Accelerometer: Detects rack extraction or kinetic shock (> 1.5G).    │
│   ├── Conductive Mesh Enclosure: Detects drill bit penetration through chassis walls.  │
│   └── Dedicated LiFePO4 Backup Battery: Powers tamper logic during external power cut. │
│                                                                                        │
│                                  │ (Tamper Condition Triggered)                        │
│                                  ▼                                                     │
│ [Autonomous Crowbar Discharge Subsystem: Execution Latency < 800 µs]                   │
│   ├── Asserts CPU Non-Maskable Interrupt (NMI) -> Immediate kernel halt.               │
│   ├── Low-RDS(on) Power MOSFET Gate Driven High -> Short circuits DRAM VDD to Ground. │
│   ├── Fuses Hardware TPM Tamper Lock Register -> Erases Primary Storage Seeds.         │
│   └── NVMe Self-Encrypting Drive (SED) Crypto Erase Fired -> Deletes master drive key. │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### 2.1. Electronic Circuit Analysis & Cold-Boot Physics

Standard DRAM storage capacitors maintain bit patterns for up to $120\text{ seconds}$ after power loss, especially when cooled with compressed aerosol gas (liquid nitrogen cools chips to $77\text{ K}$, extending retention to hours). SIAR custom chassis deploy active crowbar discharge circuits:

The DRAM voltage decay under active crowbar discharge is governed by:

$$V(t) = V_0 \cdot e^{-\frac{t}{R_{\text{on}} \cdot C_{\text{eff}}}}$$

Where:
- $V_0 = 1.20\text{ V}$ (DDR4/DDR5 nominal core supply voltage).
- $C_{\text{eff}} \approx 450\ \mu\text{F}$ (total decoupling and cell capacitance across all DIMMs).
- $R_{\text{on}} \le 15\text{ m}\Omega$ (drain-to-source on-resistance of the low-impedance N-channel power MOSFET).

The time constant $\tau$:

$$\tau = R_{\text{on}} \cdot C_{\text{eff}} = 0.015\ \Omega \times 4.5 \times 10^{-4}\ \text{F} \approx 6.75\ \mu\text{s}$$

To reach the sub-threshold logic level $V_{\text{IL}} \le 0.1\text{ V}$:

$$t_{\text{discharge}} = \tau \cdot \ln\left(\frac{1.20}{0.10}\right) \approx 6.75\ \mu\text{s} \times 2.48 \approx 16.7\ \mu\text{s} \ll 800\ \mu\text{s}$$

Residual electric charges are wiped within $< 20\ \mu\text{s}$, completely scrambling silicon memory cells into pure thermal noise ($\mathcal{H} \approx 8.0\text{ bits/byte}$) before physical access can be achieved.

### 2.2. Faraday Cage Shielding Effectiveness (TEMPEST Defense)
Server halls are lined with copper mesh RF attenuation barriers. The skin depth $\delta$ of copper at frequency $f$ is:

$$\delta = \sqrt{\frac{\rho}{\pi f \mu_r \mu_0}}$$

Shielding Effectiveness $SE$ in decibels:

$$SE(\text{dB}) = R(\text{Reflection}) + A(\text{Absorption}) + B(\text{Multi-reflection}) \ge 80\text{ dB}$$

Preventing side-channel electromagnetic eavesdropping on CPU memory buses from external surveillance vans.

---

## 3. Production Rust Implementation: Hardware Scorched-Earth Controller

The zeroization daemon operates at highest real-time priority (`SCHED_FIFO` 99) with pinned memory (`mlockall`):

```rust
use std::sync::atomic::{AtomicBool, Ordering};

pub static PANIC_TRIGGERED: AtomicBool = AtomicBool::new(false);

#[repr(C, packed)]
pub struct TamperStatusRegisters {
    pub lid_open: bool,
    pub photodiode_illuminated: bool,
    pub shock_threshold_exceeded: bool,
    pub mesh_continuity_broken: bool,
}

pub struct HardwareTamperController {
    gpio_base_addr: usize,
    nvme_ctrl_addr: usize,
}

impl HardwareTamperController {
    pub fn new(gpio_base: usize, nvme_ctrl: usize) -> Self {
        Self {
            gpio_base_addr: gpio_base,
            nvme_ctrl_addr: nvme_ctrl,
        }
    }

    /// Executed via hardware interrupt handler (NMI or dedicated GPIO IRQ)
    #[inline(always)]
    pub unsafe fn execute_scorched_earth(&self, status: TamperStatusRegisters) -> ! {
        PANIC_TRIGGERED.store(true, Ordering::SeqCst);

        // 1. Immediately trigger crowbar MOSFET gate via memory-mapped I/O
        let crowbar_reg = (self.gpio_base_addr + 0x14) as *mut u32;
        core::ptr::write_volatile(crowbar_reg, 0x0000_0001);

        // 2. Erase CPU L1, L2, L3 caches and invalidate pipelines
        core::arch::x86_64::_mm_mfence();
        core::arch::x86_64::_wbinvd();

        // 3. Issue NVMe Sanitize Crypto Scramble command (OPAL 2.0 PSID erase)
        let nvme_sanitize_reg = (self.nvme_ctrl_addr + 0x20) as *mut u32;
        core::ptr::write_volatile(nvme_sanitize_reg, 0xDEAD_BEEF);

        // 4. Blow TPM hardware tamper lock fuse
        let tpm_fuse_reg = (self.gpio_base_addr + 0x28) as *mut u32;
        core::ptr::write_volatile(tpm_fuse_reg, 0x0000_FFFF);

        // 5. Permanent CPU halt
        loop {
            core::arch::x86_64::_mm_pause();
        }
    }
}
```

---

## 4. Facility Disaster Evacuation & Tactical Field Trailers (`sys-arch/118`)

When earthquakes, hurricanes, or armed conflicts threaten a fixed server facility, operations shift to **Tactical Mobile Nodes**:

```mermaid
graph TD
    Trigger[Disaster / Seizure Alert] --> FastEvac[Automated Evacuation Sequence]
    
    subgraph Data Destruction (< 5 seconds)
        FastEvac --> NVMe[NVMe Format --ses=2 Cryptographic Erase]
        FastEvac --> TPM[TPM Permanent Lockout Purge]
    end
    
    subgraph Traffic Failover (< 15 seconds)
        FastEvac --> BGP[Withdraw BGP Anycast Prefix]
        FastEvac --> DNS[Migrate Anonymous Ingress to Geodistributed Relays]
    end
    
    subgraph Tactical Mobilization
        FastEvac --> Trailer[Deploy Tactical Field Comms Trailer]
        Trailer --> SAT[Starlink / OneWeb LEO Satellite Uplink]
        Trailer --> LoRa[LoRa / Wi-Fi Mesh Relay Mast]
    end
```

### Tactical Field Communications Unit Specs
- **Power**: 4.8 kW rooftop solar PV array with 24 kWh LiFePO4 battery storage and clean bio-diesel generator.
- **Backhaul**: Dual auto-pointing Starlink / OneWeb LEO satellite terminals bonded with multi-carrier 5G cellular modems.
- **Radio Mast**: Pneumatic 15-meter telescoping mast broadcasting high-power LoRa ($868/915\text{ MHz}$) and long-range directional Wi-Fi ($5.8\text{ GHz}$), bridging isolated off-grid civilian shelters back into the global SIAR network.

---

## 5. Crisis Incident Command System (ICS) & DEFCON Escalation

In [`sys-arch/119`](../sys-arch/119-anonymous-network-crisis-command-emergency-decision-authority-multi-team-coordination-communications-situation-awareness-privacy-preserving-incident-command-architecture.md), operational posture escalates through formal DEFCON levels:

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        SIAR OPERATIONAL DEFCON ESCALATION MATRIX                       │
├──────────┬───────────────────────────┬─────────────────────────────────────────────────┤
│ Condition│ Incident Context          │ Operational Mandates & Authority                │
├──────────┼───────────────────────────┼─────────────────────────────────────────────────┤
│ DEFCON 5 │ Normal Operations         │ Standard GitOps deployments; automated SRE      │
│          │                           │ canary verifications; weekly risk reviews.      │
├──────────┼───────────────────────────┼─────────────────────────────────────────────────┤
│ DEFCON 4 │ Elevated Surveillance /   │ Freeze non-critical rollouts; increase Poisson  │
│          │ Active Probing Detected   │ mix cover traffic; rotate node TLS keys daily.  │
├──────────┼───────────────────────────┼─────────────────────────────────────────────────┤
│ DEFCON 3 │ Targeted Cyber Attack /   │ Incident Commander appointed; emergency tactical│
│          │ Multi-Node DDoS           │ mesh channel activated; priority load shedding. │
├──────────┼───────────────────────────┼─────────────────────────────────────────────────┤
│ DEFCON 2 │ Physical Perimeter Breach/│ Isolate affected datacenter region; revoke      │
│          │ Facility Raid in Progress │ directory voting keys; arm hardware crowbars.   │
├──────────┼───────────────────────────┼─────────────────────────────────────────────────┤
│ DEFCON 1 │ Armed Kinetic Seizure /   │ SCORCHED EARTH PROTOCOL: Trigger sub-ms crowbar │
│          │ Scorched-Earth Directive  │ discharge, flash NVMe shred, brick bare-metal.  │
└──────────┴───────────────────────────┴─────────────────────────────────────────────────┘
```

### Two-Man Rule Cryptographic Scorched-Earth Directive
Manual execution of DEFCON 1 requires cryptographic consensus under a **2-of-3 threshold signature** signed by physical hardware tokens held by the Incident Commander, Security Officer, and Operations Lead:

$$\sigma_{\text{scorched}} = \text{BLS-Combine}(\sigma_1, \sigma_2) \in \mathbb{G}_1$$

No single captured or coerced engineer can trigger or abort a scorched-earth directive unilaterally.

---

## 6. Physical Facility Security Threat Matrix

```text
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                        PHYSICAL SECURITY THREAT & DEFENSE MATRIX                       │
├────────────────────────┬─────────────────────────┬─────────────────────────────────────┤
│ Threat Vector          │ Attack Mechanism        │ SIAR Defense Architecture           │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Cold-Boot Attack**   │ Liquid nitrogen cools   │ Active crowbar MOSFET shorts DRAM   │
│                        │ RAM chips to dump keys  │ capacitors to ground in < 20 µs.    │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Bus Interposer**     │ Hardware sniffer on     │ Transparent memory encryption via   │
│                        │ PCIe or DDR buses       │ AMD SEV-SNP / Intel TDX AES engines.│
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **TEMPEST RF Leakage** │ Surveillance van probes │ Copper mesh Faraday cage hall with  │
│                        │ CPU RF emissions        │ > 80 dB attenuation from 10M-10GHz. │
├────────────────────────┼─────────────────────────┼─────────────────────────────────────┤
│ **Hostile Raid Seizure**│ Armed forces breach site│ Chassis switches + photodiodes fire │
│                        │ to confiscate hardware  │ instant NVMe cryptographic shred.   │
└────────────────────────┴─────────────────────────┴─────────────────────────────────────┘
```
