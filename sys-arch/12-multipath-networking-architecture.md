# Part 12 — Multipath Networking Architecture

## Reusable P2P Communication Platform

**Status:** Architecture specification  
**Part:** 12 of 24  
**Primary language:** Rust  
**Primary goals:** simultaneous multi-link use, resilient transfers, path striping, path redundancy, seamless failover, path diversity, transport neutrality, bounded resource use, battery awareness, reusable across files/calls/emergency/custom applications

---

# 1. Purpose

A modern device may have multiple usable communication paths at the same time:

```text
Wi-Fi
Ethernet
mobile data
Iroh direct
Iroh relay
Bluetooth
Wi-Fi Direct
LAN
DTN gateway
```

A simple routing engine chooses one path.

A multipath engine can instead decide:

```text
use more than one path simultaneously
```

for selected operations.

This can provide:

- higher throughput
- better resilience
- seamless path migration
- lower interruption during mobility
- redundant delivery for critical traffic
- better large-file performance
- more robust emergency communication

But multipath is not automatically better.

It can also create:

- duplicate bandwidth cost
- more radio use
- higher battery drain
- packet reordering
- more state complexity
- congestion interaction
- harder diagnostics

Therefore the core rule is:

> **Multipath is an optimization and resilience tool, not the default mode for every operation.**

---

# 2. Architectural Position

```text
Application
    ↓
Messaging / Files / Calls / Emergency
    ↓
Delivery Requirements
    ↓
Routing Policy Engine
    ↓
Multipath Planner
    ↓
Per-Path Scheduler
    ↓
Transport Sessions
    ├── Iroh Direct
    ├── Iroh Relay
    ├── LAN
    ├── Wi-Fi Direct
    ├── Bluetooth
    └── Future Transport
```

Part 03 provides path candidates.

Part 12 decides how multiple candidates may be composed.

---

# 3. Main Multipath Modes

There are four primary strategies.

## Mode A — Single Path

```text
one active path
```

This remains the default.

## Mode B — Failover-Ready

```text
one active path
+
one warm standby
```

Only one carries normal traffic.

## Mode C — Striping

```text
different data pieces
over different paths
```

Used mainly for:

```text
large files
bulk data
```

## Mode D — Redundancy

```text
same logical data
over multiple independent paths
```

Used selectively for:

```text
SOS
critical control
key signaling
```

---

# 4. Strategy Type

```rust
pub enum MultipathStrategy {
    Single,
    WarmFailover,
    Stripe,
    Redundant,
    Hybrid,
}
```

`Hybrid` may combine:

```text
striped bulk
+
redundant control
```

---

# 5. Multipath Eligibility

An operation should declare:

```rust
pub struct MultipathRequirements {
    pub allowed: bool,
    pub strategy: MultipathPreference,
    pub max_paths: u8,
    pub require_path_diversity: bool,
    pub allow_metered_secondary: bool,
    pub allow_bluetooth_secondary: bool,
}
```

Default should normally be:

```text
allowed = false
or
allowed with conservative automatic policy
```

---

# 6. Application Examples

## Text message

Usually:

```text
single path
```

Critical text:

```text
optional redundant delivery
```

## File

```text
striping may help
```

## Call

```text
warm failover
or selective media redundancy
```

## SOS

```text
redundancy across independent paths
```

---

# 7. Path Set

```rust
pub struct PathSet {
    pub primary: PathCandidate,
    pub secondary: Vec<PathCandidate>,
}
```

The multipath planner operates on a selected path set, not all known paths.

---

# 8. Path Diversity

Two paths are only truly redundant if they fail independently.

Example:

```text
Iroh Direct over Wi-Fi
Iroh Relay over same Wi-Fi
```

share:

```text
same Wi-Fi radio
same local AP
same ISP
```

They are not fully independent.

---

# 9. Failure Domains

Represent:

```rust
pub struct FailureDomainSet {
    pub interface: Option<InterfaceId>,
    pub radio: Option<RadioId>,
    pub network: Option<NetworkId>,
    pub provider: Option<ProviderId>,
    pub relay: Option<RelayId>,
}
```

Path diversity scoring considers overlap.

---

# 10. Diversity Score

Conceptually:

```text
high:
Wi-Fi + cellular

medium:
Wi-Fi direct + Internet over same Wi-Fi

low:
two streams over same QUIC path
```

Do not treat every logical path as independent.

---

# 11. Multipath Planner

```rust
pub trait MultipathPlanner {
    fn plan(
        &self,
        paths: &[PathCandidate],
        requirements: &DeliveryRequirements,
        context: &MultipathContext,
    ) -> MultipathPlan;
}
```

---

# 12. Multipath Plan

```rust
pub struct MultipathPlan {
    pub strategy: MultipathStrategy,
    pub paths: Vec<PlannedPath>,
    pub scheduler: MultipathSchedulingPolicy,
}
```

---

# 13. Planned Path

```rust
pub struct PlannedPath {
    pub path_id: PathId,
    pub role: PathRole,
    pub weight: u16,
    pub bandwidth_cap: Option<u64>,
}
```

---

# 14. Path Roles

```rust
pub enum PathRole {
    Primary,
    Secondary,
    Standby,
    Redundant,
    Bulk,
    Control,
}
```

---

# 15. Warm Failover

Warm failover means:

```text
primary path active
secondary path authenticated/prepared
```

If primary fails:

```text
switch immediately
```

This reduces reconnection latency.

---

# 16. Warm Failover Cost

Keeping standby path alive consumes:

```text
radio
keepalive
memory
connection slot
```

Therefore only use for:

```text
calls
critical transfer
important active session
```

---

# 17. Make-Before-Break

Preferred for seamless mobility:

```text
current path active
 ↓
new path established
 ↓
authenticated
 ↓
state synchronized
 ↓
traffic moves
 ↓
old path closes
```

---

# 18. Break-Before-Make

Use when:

```text
resource constrained
security demands old path close first
OS prevents both interfaces
```

---

# 19. Path Migration

A long-lived operation should survive path change.

Examples:

```text
file transfer:
resume missing chunks

message:
retry same MessageId

call:
rebind media/session
```

Multipath does not own application semantics.

---

# 20. Striping

Striping divides work.

Example:

```text
File chunks:
0,2,4,6 → Wi-Fi
1,3,5,7 → cellular
```

Static round-robin is simple but inefficient.

Better:

```text
weighted scheduling
```

based on measured throughput.

---

# 21. Weighted Striping

If:

```text
Wi-Fi = 80 Mbps
cellular = 20 Mbps
```

target roughly:

```text
80% chunks → Wi-Fi
20% chunks → cellular
```

with adaptation.

---

# 22. Dynamic Weight

Weights update from:

```text
observed throughput
RTT
loss
queue depth
energy policy
cost policy
```

Do not oscillate too quickly.

---

# 23. Striping Granularity

For file transfer:

```text
chunk level
```

is ideal.

For streaming media:

```text
frame/packet class
```

may be possible but much harder.

For messages:

```text
do not split tiny messages unnecessarily
```

---

# 24. Chunk-Level Multipath

Part 05 already uses chunks.

That makes file multipath straightforward:

```text
missing chunk set
 ↓
scheduler
 ↓
assign chunks to paths
```

No file format redesign required.

---

# 25. Path Work Queue

Each path maintains:

```text
assigned chunks
in-flight bytes
recent throughput
```

Scheduler balances load.

---

# 26. Work Stealing

If one path becomes slow:

```text
unstarted chunks
```

can move to faster path.

Do not duplicate already-progressing large chunks unless policy permits.

---

# 27. Chunk Reassignment

When path fails:

```text
all unverified chunks assigned there
→ return to global missing set
```

Then reassign.

---

# 28. Partial Chunk Handling

If protocol supports resumable chunk subranges:

```text
continue remaining subrange
```

Otherwise retransmit whole chunk.

Keep chunk size moderate to limit wasted work.

---

# 29. Redundant Delivery

For critical small payload:

```text
Path A
+
Path B
```

both send same logical operation.

Receiver deduplicates by:

```text
MessageId
BundleId
OperationId
```

---

# 30. Redundant Delivery Budget

Never duplicate arbitrary bulk data by default.

Policy can allow:

```text
first 4 KiB control duplicated
full payload single-path
```

This is a useful hybrid.

---

# 31. Hybrid Strategy

Example file transfer:

```text
control frames:
redundant across two paths

bulk chunks:
striped

ACKs:
best current path
```

This can improve resilience without doubling file bandwidth.

---

# 32. Critical Prefix

For emergency message with attachment:

```text
SOS text
location
thumbnail
```

can be redundantly sent.

Full video remains single/striped.

---

# 33. Reordering

Multiple paths cause out-of-order arrival.

Protocol must tolerate:

```text
chunk 5 before chunk 1
ACK from path B before data on path A
```

Do not rely on total network arrival order.

---

# 34. Ordering Domains

Define:

```text
logical message order
chunk order
control order
```

separately.

File chunks can arrive out of order.

Control protocol may need stream-local sequence.

---

# 35. Reassembly Buffer

For protocols requiring order:

```text
bounded reorder buffer
```

Never buffer unboundedly waiting for missing data.

---

# 36. Head-of-Line Avoidance

One advantage of multipath is avoiding one slow path blocking all data.

Use independent chunk/stream state.

---

# 37. ACK Strategy

ACKs should reflect logical completion, not physical path.

Example:

```text
Chunk 10 received via cellular
```

ACK may return via Wi-Fi.

Path symmetry is not required.

---

# 38. Path-Independent IDs

All IDs must be independent of path.

Examples:

```text
TransferId
ChunkId
MessageId
BundleId
```

This is essential for migration.

---

# 39. Congestion Control Boundary

Transport implementations such as QUIC/Iroh own transport congestion control.

Multipath scheduler must not reinvent per-path congestion algorithms.

It uses:

```text
throughput
RTT
loss
queue feedback
```

to adjust assignment.

---

# 40. Cross-Path Congestion

Two paths may share underlay.

If both use same Wi-Fi interface, striping may only create competition.

Path diversity model should detect this.

---

# 41. Shared Bottleneck Detection

Potential signals:

```text
same interface
same gateway
correlated throughput collapse
```

Future optimization.

Start with known underlay metadata.

---

# 42. Path Cost

Each path has:

```text
monetary cost
energy cost
latency
bandwidth
```

Multipath can become expensive.

A high-throughput file should not automatically consume cellular data if Wi-Fi alone is sufficient.

---

# 43. Metered Secondary Policy

Default:

```text
do not add metered path
for throughput boost
```

unless:

```text
user allows
critical deadline
emergency override
```

---

# 44. Cellular Assist

Optional feature:

```text
Use mobile data to speed large transfer
```

must be explicit user policy.

---

# 45. Battery Cost

Two radios active simultaneously increase energy use.

Part 13 will provide deeper battery policy.

Part 12 should expose:

```text
estimated multipath energy penalty
```

and allow policy to disable.

---

# 46. Battery-Saver Behavior

In battery saver:

```text
Single
or WarmFailover only
```

Avoid active striping across multiple radios.

---

# 47. Thermal Behavior

If device hot:

```text
collapse multipath
reduce CPU/network parallelism
```

---

# 48. Resource Integration

Part 08 must grant:

```text
additional connection permits
stream permits
buffer permits
bandwidth budget
```

before enabling multipath.

---

# 49. Max Paths

Hard limit:

```text
2–3 active paths
```

is usually enough.

Do not support dozens without a real use case.

---

# 50. Path Admission

Before adding secondary path:

```text
does benefit exceed cost?
```

Check:

```text
resource permits
battery
metered policy
path diversity
estimated throughput gain
```

---

# 51. Benefit Score

Conceptually:

```text
benefit =
    throughput gain
  + resilience gain
  + deadline improvement

cost =
    energy
  + monetary
  + complexity
  + resource usage
```

Enable only when:

```text
benefit > policy threshold
```

---

# 52. Throughput Gain Estimate

Estimate from:

```text
current primary saturation
secondary independent bandwidth
shared underlay
```

Do not simply add advertised bandwidth.

---

# 53. Resilience Gain

High when:

```text
failure domains differ
```

Low when:

```text
same radio/network
```

---

# 54. Warm Standby vs Active Striping

If resilience is goal but throughput is already sufficient:

```text
warm standby
```

may be better than active striping.

---

# 55. Path Quality Monitor

Each active path reports:

```text
RTT
throughput
loss
jitter
queue delay
health
```

Multipath scheduler consumes these.

---

# 56. Sampling

Do not sample too aggressively.

Use existing transport telemetry.

Avoid extra probe traffic where possible.

---

# 57. Smoothing

Use EWMA or similar smoothing for:

```text
throughput
RTT
loss
```

to reduce oscillation.

---

# 58. Path Degradation

If path health moves:

```text
Healthy → Degraded
```

scheduler reduces weight.

At:

```text
Unreachable
```

remove path.

---

# 59. Weight Decay

Gradually reduce traffic before full path removal where appropriate.

This can prevent sudden reordering burst.

---

# 60. Path Recovery

A failed path may later recover.

Re-add only after:

```text
probe/health threshold
cooldown
```

Avoid flapping.

---

# 61. Hysteresis

Multipath membership needs hysteresis too.

Do not add/remove secondary path every few seconds.

---

# 62. Multipath State Machine

```text
Single
 ↓
Evaluating
 ↓
SecondaryConnecting
 ↓
MultipathActive
 ↓
Degraded
 ↓
Single
```

Alternative:

```text
Failed
DisabledByPolicy
```

---

# 63. Plan Epoch

```rust
pub struct MultipathPlanEpoch(u64);
```

When plan changes:

```text
epoch increments
```

Workers ignore stale assignments.

---

# 64. Assignment Token

Chunk assignment includes:

```text
plan epoch
path id
chunk id
```

If plan changes before send:

```text
stale assignment can be cancelled
```

---

# 65. File Multipath Architecture

```text
Missing Chunks
     ↓
Multipath Scheduler
 ┌──────┼──────┐
 │      │      │
Wi-Fi Cellular Relay
 │      │      │
 └──────┼──────┘
     Receiver
```

Verified chunk bitmap remains authoritative.

---

# 66. File Completion

Transfer completes when:

```text
all required chunks verified
```

regardless of path.

---

# 67. File Path Switch

A transfer can start:

```text
Iroh relay
```

then later add:

```text
LAN
```

and move most remaining chunks to LAN.

---

# 68. Source Multipath

Future multi-source:

```text
Bob phone
Bob laptop
relay cache
```

all serving same BlobId.

This is source-level multipath plus transport multipath.

Keep these separate concepts.

---

# 69. Multi-Source Scheduler

Can choose:

```text
source
+
path
+
chunk
```

This is advanced and should be added after single-source multipath is stable.

---

# 70. Messaging Multipath

Ordinary message:

```text
single path
```

Potential critical message:

```text
redundant
```

Do not stripe a 500-byte message.

---

# 71. Receipt Multipath

Delivery ACK can use whichever path is available.

No need to mirror inbound path.

---

# 72. Call Multipath

Realtime media multipath is harder.

Possible modes:

```text
audio primary
audio redundant on backup
video on high-bandwidth path
control redundant
```

Avoid full packet striping initially.

---

# 73. Audio Redundancy

For critical/high-loss calls:

```text
duplicate selected audio frames
```

over independent secondary path.

Can improve continuity at bandwidth cost.

---

# 74. Video Keyframe Redundancy

Optional future:

```text
keyframes redundant
delta frames single-path
```

This is codec/media-specific and belongs above generic multipath scheduler.

---

# 75. Media Path Split

Example:

```text
audio → cellular low-jitter
video → Wi-Fi high-bandwidth
```

Possible if media engine supports independent streams.

---

# 76. Call Handover

Important practical goal:

```text
Wi-Fi leaves range
 ↓
cellular path already warm
 ↓
call continues
```

This is more valuable initially than full aggregate bandwidth.

---

# 77. Emergency Multipath

Emergency policy may use:

```text
Internet path
+
nearby DTN copy
```

This is not simultaneous packet striping.

It is logical redundant delivery across different delivery systems.

---

# 78. Multipath + DTN

A critical message can be:

```text
sent direct over Iroh
+
stored as DTN bundle
```

Destination deduplicates.

This is powerful for disaster resilience.

---

# 79. DTN Copy Retirement

If direct delivery ACK arrives:

```text
cancel/retire remaining DTN replicas
```

best effort.

---

# 80. Relay + Direct

Iroh may move between relay/direct internally.

Your platform should not duplicate unnecessary multipath logic if Iroh already optimizes that transport path.

Treat Iroh path state as one logical transport candidate unless there is a real separate path benefit.

---

# 81. Iroh Boundary

Do not try to micromanage QUIC internals from application scheduler.

Use Iroh-provided connection/path metrics where available.

---

# 82. Explicit Multi-Interface Iroh

If future Iroh supports multiple simultaneous interface paths directly, adapter can expose that as native multipath capability.

Until then, platform-level multipath can operate across separate transport sessions.

---

# 83. Capability Negotiation

Part 07 should expose:

```text
multipath supported
max active paths
striping supported
redundancy supported
```

for protocol operations where peer cooperation matters.

---

# 84. Capability Example

```rust
pub struct MultipathCapability {
    pub max_paths: u8,
    pub striping: bool,
    pub redundant_control: bool,
    pub chunk_reassignment: bool,
}
```

---

# 85. Peer Cooperation

File striping only needs receiver to accept chunks out-of-order.

If files/1 already supports that, multipath may not need separate wire protocol.

Do not add protocol negotiation where local scheduling alone is sufficient.

---

# 86. Multipath Metadata Privacy

Do not expose:

```text
user has cellular
user has Ethernet
exact ISP
```

to peer unless necessary.

Keep detailed path topology local.

---

# 87. Local Planner Only

Most multipath decisions should remain local.

Peer receives normal protocol traffic.

This reduces protocol complexity.

---

# 88. Path Binding

For some operations, path switch may require:

```text
session rebinding
```

Protocol must authenticate that new path belongs to same peer/session identity.

---

# 89. Cross-Path Session Binding

Use:

```text
DeviceId
SessionId
OperationId
```

to prove continuity.

Do not trust same IP/account display name.

---

# 90. Anti-Hijack

Attacker must not inject:

```text
"secondary path"
```

into active transfer.

Every path must complete normal identity/session authentication.

---

# 91. Reordering Attack Surface

Multiple authenticated paths still require:

```text
sequence validation
duplicate suppression
bounded reorder
```

---

# 92. Replay Protection

Stable IDs plus sequence/epoch prevent stale chunks/control frames from old path plan.

---

# 93. Path Closure

On removing path:

```text
stop assigning new work
drain/cancel in-flight
release permits
close session if unused
```

---

# 94. Graceful Drain

For reliable file chunks:

```text
allow in-flight chunk finish
```

if path merely being de-prioritized.

---

# 95. Immediate Abort

If:

```text
security failure
revocation
```

close immediately.

---

# 96. Cancellation

User cancels operation:

```text
cancel across all paths
```

One path cannot continue independently.

---

# 97. Operation Completion

When operation complete:

```text
stop duplicate paths
cancel outstanding assignments
release resources
```

---

# 98. Straggler Problem

One slow path may hold final chunk.

Scheduler can hedge/reassign:

```text
if expected completion too slow
→ duplicate final missing chunk on faster path
```

This is useful for tail latency.

---

# 99. Hedged Chunk

For final few chunks:

```text
send same chunk on second path after delay
```

First verified copy wins.

Use sparingly.

---

# 100. Tail Optimization

Hedging is more useful near transfer completion than full-time duplication.

---

# 101. Cost Control

Track:

```text
duplicate bytes
metered bytes
secondary-path bytes
```

Multipath should have measurable benefit.

---

# 102. Multipath Efficiency Metric

```text
useful_bytes / transmitted_bytes
```

High redundancy lowers efficiency.

---

# 103. Throughput Gain Metric

```text
multipath throughput
/
best single-path throughput
```

If gain near 1.0, disable striping.

---

# 104. Resilience Metric

Track:

```text
operations saved by secondary path
handover success
```

---

# 105. Path Flap Metric

Track:

```text
add/remove transitions per session
```

High count indicates unstable policy.

---

# 106. Diagnostics

Advanced:

```text
Strategy: Stripe
Paths:
  Wi-Fi 70%
  Cellular 30%

Shared underlay: no
Duplicate bytes: 0
Path switches: 1
```

---

# 107. User UI

Normal UI should not expose path internals.

Possible indicator:

```text
Using multiple networks
```

only if useful.

---

# 108. User Control

Settings:

```text
Use mobile data to improve large transfers
Use redundant delivery for emergency messages
```

Avoid complex manual path selection.

---

# 109. Data Usage Warning

If multipath activates metered secondary:

```text
user policy must already permit
```

Do not surprise users with cellular usage.

---

# 110. Resource Profiles

Embedded:

```text
single path only
```

MobileLow:

```text
warm failover limited
```

Desktop:

```text
file striping allowed
```

Server:

```text
multiple high-bandwidth paths possible
```

---

# 111. Network Interface Abstraction

```rust
pub struct NetworkInterfaceId;
```

Path candidates may bind to interface metadata.

---

# 112. Interface State

```text
Up
Down
Metered
Roaming
Constrained
```

reported by platform adapter.

---

# 113. Wi-Fi + Ethernet

Desktop:

```text
Ethernet primary
Wi-Fi standby
```

useful for seamless cable disconnect.

---

# 114. Wi-Fi + Cellular

Mobile:

```text
Wi-Fi primary
cellular standby
```

for call continuity.

---

# 115. LAN + Iroh

File transfer:

```text
LAN high bandwidth
Iroh secondary control/fallback
```

---

# 116. Bluetooth + Wi-Fi

Nearby:

```text
BLE control/discovery
Wi-Fi Direct bulk
```

This is transport-role multipath rather than aggregate striping.

---

# 117. Control/Data Split

A robust pattern:

```text
control on reliable established path
bulk on high-bandwidth path
```

If bulk path dies, control remains.

---

# 118. Dedicated Control Path

For large file:

```text
Iroh relay control
LAN data
```

can be useful when LAN path unstable.

---

# 119. Control Path Failure

If control path fails but bulk alive:

```text
promote another path
```

Do not unnecessarily kill transfer.

---

# 120. Independent Stream State

Each path/session maintains:

```text
send state
health
metrics
```

but logical operation state remains shared.

---

# 121. Shared Transfer State

```text
global missing chunks
verified chunks
completion
```

not duplicated per path.

---

# 122. Scheduler Architecture

```text
Global Work Set
    ↓
Path Scorer
    ↓
Weighted Allocator
    ↓
Per-Path Queues
    ↓
Transports
```

Feedback returns to scorer.

---

# 123. Weighted Allocator

```rust
pub trait MultipathAllocator {
    fn assign(
        &mut self,
        work: &[WorkItem],
        paths: &[PathState],
    ) -> Vec<PathAssignment>;
}
```

---

# 124. Initial Allocator

Start with:

```text
weighted least-loaded
```

rather than complex optimization.

---

# 125. Weighted Least-Loaded

Score:

```text
in-flight bytes / estimated throughput
```

Assign next chunk to smallest estimated completion time.

---

# 126. Completion-Time Estimate

```text
queue_delay
+
chunk_size / throughput
+
RTT factor
```

approximate is enough.

---

# 127. Path Weight Floor

Avoid starving secondary completely if warm measurement needed.

Can send occasional small probe/work if policy permits.

---

# 128. Probe Traffic

Prefer natural application traffic for measurement.

Avoid synthetic bandwidth probes on mobile unless necessary.

---

# 129. Multipath Persistence

Do not persist path assignments as durable truth.

Persist:

```text
operation progress
```

On restart:

```text
replan from current paths
```

---

# 130. Crash Recovery

After crash:

```text
verified chunks restored
all paths considered gone
new plan generated
```

Simple and robust.

---

# 131. Event Log Integration

Part 04 may record meaningful:

```text
TransferPathChanged
```

only if useful to product/history.

Most path changes belong in diagnostics.

---

# 132. Resource Limit Integration

Every additional path consumes:

```text
connection slot
stream slot
memory
bandwidth
energy
```

Part 08 admission is mandatory.

---

# 133. Backpressure Integration

Per-path queue full:

```text
allocator stops assigning
```

Global producer slows.

No unbounded buffer between paths.

---

# 134. Battery Integration

Part 13 may collapse strategy:

```text
Stripe → Single
```

when low battery.

---

# 135. Thermal Integration

Path set can shrink dynamically.

---

# 136. Capability Integration

Part 07 may limit:

```text
max parallel chunk streams
```

Multipath effective concurrency respects that.

---

# 137. Relay Infrastructure Integration

Part 11 can supply:

```text
multiple relay candidates
```

but using two relays simultaneously only helps if policy and failure domains justify.

---

# 138. DTN Integration

Part 06 can act as a logically independent redundant delivery system.

This is often more valuable for SOS than duplicating over two Internet relays.

---

# 139. Fuzz/Test Integration

Part 10 needs:

```text
path addition/removal
duplicate chunk
late ACK
stale plan epoch
reordering
shared-underlay cases
```

---

# 140. Simulation

Use deterministic multi-path simulator:

```text
Path A:
100 Mbps, 20 ms

Path B:
20 Mbps, 80 ms

Path C:
metered, 50 Mbps
```

Test policy decisions.

---

# 141. Striping Test

Expected:

```text
A receives majority chunks
B receives smaller share
C unused if metered forbidden
```

---

# 142. Failure Test

Kill A midway.

Expected:

```text
B takes remaining
verified progress retained
```

---

# 143. Recovery Test

Restart process.

Expected:

```text
no path assignments restored
missing chunks replanned
```

---

# 144. Shared Underlay Test

A and B same Wi-Fi.

Expected:

```text
planner may choose warm failover
instead of active striping
```

---

# 145. Cost Test

Secondary metered.

Policy:

```text
allow_metered_secondary=false
```

Expected:

```text
never used for throughput boost
```

---

# 146. Emergency Test

Paths:

```text
Internet
BLE/DTN
```

Expected:

```text
critical payload duplicated logically
```

---

# 147. Call Handover Test

```text
Wi-Fi active
cellular standby
Wi-Fi disappears
```

Expected:

```text
handover bounded interruption
```

---

# 148. Reorder Test

Chunks arrive:

```text
5,2,4,1,3
```

Receiver completes correctly.

---

# 149. Stale Assignment Test

Plan epoch changes.

Old worker submits chunk assignment.

Expected:

```text
ignore/cancel stale plan work
```

---

# 150. Duplicate Completion Test

Same chunk arrives simultaneously on two paths.

Expected:

```text
one verified state transition
```

---

# 151. Path Security Test

Unauthenticated secondary attempts join transfer.

Expected:

```text
reject
```

---

# 152. Resource Exhaustion Test

Try enabling 10 paths.

Global max:

```text
2
```

Expected:

```text
only 2 admitted
```

---

# 153. Performance Benchmarks

Measure:

```text
single path throughput
striped throughput
CPU overhead
memory overhead
battery proxy
duplicate bytes
handover latency
```

---

# 154. Benefit Gate

Enable multipath by default only after benchmarks show clear improvement for target workloads.

---

# 155. No Universal Aggregation Promise

Some OS/platform/network combinations may not allow simultaneous useful interfaces.

Architecture must degrade to:

```text
single path
```

without feature failure.

---

# 156. Android Constraints

Android may manage:

```text
cellular/Wi-Fi routing
network binding
background restrictions
```

Kotlin/platform adapter may expose available network handles.

Rust owns multipath policy.

---

# 157. Android Network Binding

Platform adapter can provide:

```text
network-specific socket/session binding capability
```

where available.

Do not duplicate policy in Kotlin.

---

# 158. iOS Constraints

iOS may restrict direct control over interface selection.

Support multipath only where platform APIs permit.

Do not promise identical behavior across platforms.

---

# 159. Desktop Control

Linux/Windows/macOS generally provide more network-interface visibility, but implementation details differ.

Keep adapter-specific.

---

# 160. Headless Linux

Server/headless node may have:

```text
Ethernet
Wi-Fi
multiple NICs
multiple uplinks
```

Multipath can be particularly useful there.

---

# 161. Source Address Stability

Path migration must not assume stable IP.

Identity/session continuity comes from cryptographic peer identity.

---

# 162. NAT Changes

Mobile path may rebind NAT.

Transport/session layer handles reconnection.

Operation state remains independent.

---

# 163. QoS Separation

Realtime and bulk traffic can intentionally use different paths.

Example:

```text
call audio → low-latency path
file sync → high-throughput path
```

---

# 164. Multi-Operation Coordination

Scheduler across operations should avoid:

```text
file multipath consuming all paths
```

while call active.

Part 08 global resource scheduler decides.

---

# 165. Path Reservation

A call can reserve primary path bandwidth.

File multipath uses remaining capacity.

---

# 166. Multipath Quotas

Per application:

```text
max secondary cellular bytes/day
max redundant emergency bytes
```

optional product policy.

---

# 167. Telemetry Privacy

Do not export:

```text
exact network names
IP addresses
carrier
```

unless explicitly needed.

Aggregate:

```text
wifi
cellular
lan
relay
```

---

# 168. Metrics

Useful:

```text
multipath sessions
strategy distribution
path switches
handover success
striping gain
duplicate bytes
metered secondary bytes
```

---

# 169. Diagnostics Reason Codes

```rust
pub enum MultipathReason {
    LargeTransfer,
    Deadline,
    PrimaryDegraded,
    EmergencyRedundancy,
    CallHandover,
    DisabledByBattery,
    DisabledByCost,
    SharedFailureDomain,
}
```

---

# 170. Error Model

```rust
pub enum MultipathError {
    NoEligibleSecondary,
    ResourceDenied,
    PolicyDenied,
    AuthenticationFailed,
    PathFailed,
    Unsupported,
    Cancelled,
}
```

---

# 171. Public API

Most applications should not call low-level multipath APIs.

They declare:

```text
allow_multipath
priority
deadline
cost policy
```

Routing/file/media layers invoke planner.

---

# 172. Advanced API

For infrastructure/custom product:

```rust
let plan = multipath.plan(request, candidates)?;
```

Useful for diagnostics/testing.

---

# 173. Suggested Crate Structure

```text
crates/comm-multipath/
├── src/
│   ├── lib.rs
│   ├── strategy.rs
│   ├── plan.rs
│   ├── planner.rs
│   ├── path_set.rs
│   ├── diversity.rs
│   ├── allocator.rs
│   ├── scheduler.rs
│   ├── metrics.rs
│   ├── hysteresis.rs
│   ├── failover.rs
│   ├── redundancy.rs
│   ├── diagnostics.rs
│   └── error.rs
└── Cargo.toml
```

---

# 174. Initial Production Scope

Implement first:

```text
single path baseline
warm failover
make-before-break
file chunk striping
weighted allocator
path diversity metadata
metered policy
resource admission
path health/hysteresis
stale plan epoch
```

Then:

```text
tail hedging
control/data split
selective emergency redundancy
call handover
```

Defer initially:

```text
full realtime packet aggregation
complex multi-source swarming
ML path prediction
```

---

# 175. Implementation Phases

## Phase 1 — Models

```text
MultipathStrategy
PathSet
FailureDomains
MultipathPlan
```

## Phase 2 — Warm Failover

```text
secondary path
make-before-break
handover
```

## Phase 3 — File Striping

```text
chunk scheduler
weighted assignment
reassignment
```

## Phase 4 — Diversity/Cost

```text
underlay overlap
metered policy
battery/resource checks
```

## Phase 5 — Redundancy

```text
critical control
SOS duplicate
tail hedging
```

## Phase 6 — Media

```text
call handover
audio/control redundancy
```

## Phase 7 — Hardening

```text
simulation
reordering
failure
resource
security
benchmarks
```

---

# 176. Definition of Done

Part 12 is complete when:

- multipath is optional rather than mandatory
- single-path behavior remains correct
- file transfer can stripe chunks across at least two authenticated paths
- path failure returns unfinished chunks to scheduler
- verified chunks are never re-downloaded unnecessarily
- make-before-break handover works where platform permits
- path diversity considers shared failure domains
- metered secondary paths obey user policy
- battery/resource pressure can collapse multipath safely
- duplicate critical delivery is deduplicated
- path assignments are not persisted as durable truth
- crash recovery replans from operation progress
- stale plan epochs are rejected
- one operation cannot exceed global path/stream limits
- call/media can use warm failover without requiring full packet striping
- DTN can act as independent redundant delivery for emergency traffic
- diagnostics explain why multipath was enabled/disabled
- simulation/failure/reorder/cost/resource/security tests exist

---

# 177. Relationship to Earlier Parts

Part 12 builds on:

```text
01 — Protocol Extension System
02 — Multi-Device Identity
03 — Transport & Routing Policy Engine
04 — Offline Event Log
05 — Robust File / Blob Subsystem
06 — DTN / Store-Carry-Forward
07 — Capability Negotiation
08 — Resource Limits & Backpressure
09 — Crash Recovery
10 — Fuzzing & Protocol Test Suite
11 — Relay / Self-Hosted Infrastructure
```

It directly supports:

```text
13 — Battery-Aware Scheduling
14 — Proximity Abstraction
16 — Daemon & Headless Runtime
17 — Emergency Priority Architecture
18 — Network Diagnostics & Path Visualization
20 — Embedded Linux Node
23 — External Interoperability Suite
```

---

# 178. Final Principle

Multipath should make this kind of behavior possible:

```text
A 5 GB file starts over Iroh relay.

LAN becomes available.
LAN becomes the high-bandwidth primary data path.

The relay remains a control/fallback path.

Wi-Fi drops briefly.
The transfer continues over relay.

Wi-Fi returns.
The scheduler moves remaining chunks back.

No chunk already verified is lost.
No transfer restarts from zero.
```

And for a call:

```text
Wi-Fi active
+
cellular standby

Wi-Fi disappears
→ cellular takes over
```

And for emergency delivery:

```text
Internet send
+
DTN local copy
```

The value of multipath is not merely "more bandwidth."

Its real purpose is:

```text
resilience
continuity
controlled redundancy
and efficient use of multiple available paths
```

without sacrificing battery, privacy, cost control, or architectural simplicity.
