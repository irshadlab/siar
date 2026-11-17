# Field Operations & Disaster Mesh Operator Guide

## Resilient P2P Communication in Infrastructure-Denial & Emergency Scenarios

> **Scope Note:** Low-level transport routing engines, multi-path scheduling, DTN store-carry-forward mechanics, BLE/Wi-Fi proximity drivers, emergency priority queues, embedded Linux node daemons, and E2EE cryptography are specified in:
> - [`sys-arch/03`](file:///home/irshad/Projects/siar/sys-arch/03-transport-routing-policy-engine-architecture.md) — Transport & Routing Policy Engine
> - [`sys-arch/06`](file:///home/irshad/Projects/siar/sys-arch/06-dtn-store-carry-forward-architecture.md) — DTN Store-Carry-Forward Architecture
> - [`sys-arch/12`](file:///home/irshad/Projects/siar/sys-arch/12-multipath-networking-architecture.md) — Multipath Networking Architecture
> - [`sys-arch/13`](file:///home/irshad/Projects/siar/sys-arch/13-battery-aware-scheduling-architecture.md) — Battery-Aware Scheduling Architecture
> - [`sys-arch/14`](file:///home/irshad/Projects/siar/sys-arch/14-proximity-abstraction-architecture.md) — Proximity Abstraction (BLE / Wi-Fi Direct / Wi-Fi Aware)
> - [`sys-arch/15`](file:///home/irshad/Projects/siar/sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md) — QR & NFC Bootstrap Pairing Architecture
> - [`sys-arch/17`](file:///home/irshad/Projects/siar/sys-arch/17-emergency-priority-classes-architecture.md) — Emergency Priority Classes Architecture
> - [`sys-arch/18`](file:///home/irshad/Projects/siar/sys-arch/18-network-diagnostics-path-visualization-architecture.md) — Network Diagnostics & Path Visualization
> - [`sys-arch/20`](file:///home/irshad/Projects/siar/sys-arch/20-embedded-linux-node-architecture.md) — Embedded Linux Node Architecture
> - [`sys-arch/28`](file:///home/irshad/Projects/siar/sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md) — Production Security & Key Management
>
> This document defines the **field operational workflows, deployment topologies, triage procedures, and operator protocols** for executing communications when central infrastructure is unavailable.

---

## 1. Operational Hierarchy & Environmental Degradation

SIAR is engineered to degrade gracefully across six distinct operational tiers without requiring the user to switch applications or alter identity:

```text
Tier 1: Global Internet Available
└── Direct Iroh QUIC + Central Relay Fallback (Full Audio/Video/Large Files)

Tier 2: Cellular/Backbone Severed, Local Wi-Fi Router Intact
└── Autonomous LAN Discovery + Direct Subnet QUIC (Full Speed, Zero Internet)

Tier 3: Power Grid Outage, No Routers, Nearby Wi-Fi Devices Present
└── Wi-Fi Aware NAN Clusters & Wi-Fi Direct Ad-Hoc Groups (High Bandwidth Mesh)

Tier 4: Extreme Power Constraints / Range-Only Scenarios
└── Bluetooth Classic (RFCOMM) & Bluetooth Low Energy (GATT) Proximity Links

Tier 5: Disconnected Island Communities & Moving Entities
└── Delay-Tolerant Store-Carry-Forward (DTN) Mule Transport

Tier 6: Critical Distress (Disaster / Search & Rescue)
└── Preempting Emergency Priority SOS Broadcasts across all active radio layers
```

---

## 2. Field Operational Profiles

### A. Civilian User Profile
- **Default State:** Passive listening and opportunistic message forwarding.
- **Power Policy:** Maximum battery preservation. Radio scans throttled according to battery level (see [`sys-arch/13`](file:///home/irshad/Projects/siar/sys-arch/13-battery-aware-scheduling-architecture.md)).
- **Action Triggers:** Single-tap emergency SOS actions (`I'm Safe`, `Need Medical Help`, `Need Water/Food`, `Need Shelter`, `Report Hazard`).
- **Privacy Controls:** Automatic rotating BLE identifiers (see [`sys-arch/14`](file:///home/irshad/Projects/siar/sys-arch/14-proximity-abstraction-architecture.md)) and configurable location fuzzing (exact, approximate 500m, or landmark-only).

### B. First Responder / Field Triage Profile
- **Default State:** Active disaster mesh coordination and triage channel listener.
- **Power Policy:** Continuous scanning on supported hardware (vehicle power or portable power banks).
- **Capabilities:**
  - High-priority emergency broadcast override (see [`sys-arch/17`](file:///home/irshad/Projects/siar/sys-arch/17-emergency-priority-classes-architecture.md)).
  - Local emergency triage casualty tracking.
  - Authority alert signing and broadcast verification.

### C. Community Shelter / Fixed Node Profile
- **Hardware:** Embedded Linux device (e.g., Raspberry Pi, OpenWrt router) with solar/battery backup (see [`sys-arch/20`](file:///home/irshad/Projects/siar/sys-arch/20-embedded-linux-node-architecture.md)).
- **Role:** Autonomous DTN message repository and local emergency bulletin board hub.
- **Uplink Behavior:** Acts as an automatic gateway if intermittent satellite (Starlink), cellular, or packet radio backhaul becomes available.

---

## 3. Field Triage & Emergency Communication Flows

### SOS Distress Workflow
```text
Civilian in Distress
        │
        ▼ (Presses "Need Medical Help")
Structured Compact SOS Envelope Generated (< 128 Bytes)
        │ (Signed with Account Root Key, sys-arch/02)
        ▼
Broadcast over BLE Beacon + Wi-Fi Aware NAN (P0 Priority, sys-arch/17)
        │
        ├── Encountered by Passing Device (Mule)
        │       │
        │       ▼
        │   Stored in DTN Vault (sys-arch/06) & Replicated to Next Peer
        │
        └── Encountered by Shelter / Triage Node
                │
                ▼
            Alerts Field Medics & Pins to Local Emergency Board
                │
                ▼ (When Internet Restores / Gateway Reached)
            Flushes Outbox to External Emergency Services
```

### Semantic Emergency Action Matrix

| Emergency Code | Payload Size | Default Radio Priority | Forwarding Rule |
| :--- | :--- | :--- | :--- |
| `SOS_MEDICAL` | ~96 bytes | **P0 Emergency (Immediate Preemption)** | Infinite hop replication within TTL budget |
| `SOS_TRAPPED` | ~96 bytes | **P0 Emergency (Immediate Preemption)** | Infinite hop replication within TTL budget |
| `STATUS_SAFE` | ~64 bytes | **P1 Critical (High Priority)** | Replicate to contacts and local mesh board |
| `RESOURCE_REQ` (Water/Food)| ~128 bytes | **P2 Interactive Priority** | Replicate within 5 km locality radius |
| `HAZARD_ALERT` (Fire/Flood) | ~160 bytes | **P1 Critical (High Priority)** | Immediate geographic broadcast to all nearby nodes |

---

## 4. Local Emergency Bulletin Board Operating Model

In disaster zones without central servers, shelter nodes and mobile clients maintain a decentralized, cryptographically verified **Local Emergency Board**:

```text
┌──────────────────────────────────────────────────────────────┐
│ 🚨 LOCAL DISASTER BULLETIN BOARD (Offline Community Hub)     │
├──────────────────────────────────────────────────────────────┤
│ 📍 Shelter: Central High School (Power: Solar | Water: YES)  │
│ 🕒 Last Updated: 10 mins ago via Verified Node [Shelter-01]  │
├──────────────────────────────────────────────────────────────┤
│ 🔴 URGENT NOTICES                                            │
│ • [12:30] Bridge on 4th Street Impassable (Flooding)        │
│ • [11:15] Medical supplies arriving at North Clinic (14:00)  │
├──────────────────────────────────────────────────────────────┤
│ 🟢 SAFE CHECK-INS (42 Total Nearby)                          │
│ • Alice Cooper (Verified Contact) — "Safe at Shelter"        │
│ • Bob Rahman (Verified Contact) — "Heading to North Ridge"   │
├──────────────────────────────────────────────────────────────┤
│ ⚠️ PENDING SOS REQUESTS (Carried by Mesh)                    │
│ • [12:45] Medical Assistance Needed — Sector 4 (~400m away)   │
└──────────────────────────────────────────────────────────────┘
```

1. **Anti-Spam & Quota Discipline:** Local bulletin board entries enforce strict cryptographic signatures and size quotas to prevent denial-of-service on memory-constrained devices (see [`sys-arch/08`](file:///home/irshad/Projects/siar/sys-arch/08-resource-limits-backpressure-architecture.md)).
2. **Authority Verification:** Public disaster alerts from official agencies require Ed25519 signature validation against pre-configured public authority keys or out-of-band QR trust bootstrap (see [`sys-arch/15`](file:///home/irshad/Projects/siar/sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md) and [`sys-arch/28`](file:///home/irshad/Projects/siar/sys-arch/28-production-security-e2ee-key-management-privacy-architecture.md)).

---

## 5. Field Deployment Topologies

```text
                  [ Starlink / Intermittent Backhaul ]
                                   │
                                   ▼
                    ┌─────────────────────────────┐
                    │ Community Gateway Node      │
                    │ (Solar + Embedded Linux)    │
                    └──────────────┬──────────────┘
                                   │
                    ┌──────────────┴──────────────┐
                    │ Wi-Fi LAN / Wi-Fi Direct    │
                    ▼                             ▼
        ┌───────────────────────┐     ┌───────────────────────┐
        │ Shelter Relay Node    │     │ First Responder Unit  │
        └───────────┬───────────┘     └───────────┬───────────┘
                    │                             │
               BLE / Wi-Fi                   BLE / Wi-Fi
                    │                             │
                    ▼                             ▼
        ┌───────────────────────┐     ┌───────────────────────┐
        │ Civilian Device A     │◄───►│ Civilian Device B     │
        └───────────────────────┘     └───────────────────────┘
                    ▲
                    │ (Physical Movement / DTN Mule)
                    ▼
        ┌───────────────────────┐
        │ Isolated Pocket Node  │
        └───────────────────────┘
```

---

## 6. Field Operator Safety & Operational Checklist

Before entering an off-grid or disaster operation area:
1. **Pre-Pair Critical Contacts:** Perform in-person QR code verification with team members and family while online or co-located (see [`sys-arch/15`](file:///home/irshad/Projects/siar/sys-arch/15-qr-nfc-bootstrap-pairing-architecture.md)).
2. **Pre-Cache Cryptographic Keys:** Ensure contact identity prekeys and public directories are synced locally.
3. **Configure Battery Preservation Threshold:** Set battery governor to trigger aggressive low-power scanning when device battery falls below 20% (see [`sys-arch/13`](file:///home/irshad/Projects/siar/sys-arch/13-battery-aware-scheduling-architecture.md)).
4. **Verify Storage Quotas:** Ensure DTN cache allocation has at least 500 MB available for emergency message carrying (see [`sys-arch/06`](file:///home/irshad/Projects/siar/sys-arch/06-dtn-store-carry-forward-architecture.md)).
5. **Verify Radio Permissions:** Verify Bluetooth and Wi-Fi Direct permissions are granted to avoid runtime permission prompts during critical events.
