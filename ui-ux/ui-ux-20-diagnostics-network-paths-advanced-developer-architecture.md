# UI/UX Part 20 — Diagnostics, Network Paths & Advanced Developer UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 20  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete diagnostics, observability, network-path visualization, connectivity health, call/media metrics, transfer diagnostics, background-delivery state, plugin/runtime health, support-bundle generation, privacy/redaction, developer tools, structured logs, safe repair actions, and Rust presentation contracts across desktop and Android.

---

# 1. Purpose

A production communication platform needs enough visibility to answer questions like:

```text
Why is this message still queued?
Why is this call reconnecting?
Am I connected directly or through a relay?
Is the peer reachable over LAN?
Is Bluetooth mesh active?
Why is a transfer slow?
Is Android background delivery restricted?
Is a plugin failing?
Is the local database healthy?
Is the search index rebuilding?
Did a security event block communication?
```

The product must expose this information without turning normal users into protocol engineers.

The governing principle is:

> **Diagnostics should explain system health and actionable failure causes while keeping private content, cryptographic secrets, and low-level implementation noise out of ordinary UX.**

---

# 2. Architectural Position

```text
Rust Core / Runtime
      │
      ├── transport health
      ├── path observations
      ├── relay state
      ├── LAN / proximity state
      ├── sync / delivery state
      ├── call/media metrics
      ├── transfer metrics
      ├── storage health
      ├── plugin health
      ├── security state
      └── logs / traces / counters
      │
      ▼
Diagnostics Presentation Service
      │
   ┌──┴─────────────┐
   │                │
Dioxus           Compose
Desktop          Android
```

---

# 3. Two Diagnostic Layers

The product should have:

```text
User Diagnostics
Developer Diagnostics
```

These are different surfaces.

---

# 4. User Diagnostics

Purpose:

```text
understand whether something is working
understand what to try next
share safe support information
```

---

# 5. Developer Diagnostics

Purpose:

```text
inspect internal IDs
protocol path state
timings
counters
logs
event streams
resource usage
```

Protected by:

```text
Developer Mode
```

---

# 6. Normal User vs Developer Language

Normal:

```text
Connected through relay
```

Developer:

```text
Path: Relay
RTT: 112 ms
ALPN: comm/msg/v1
QUIC session: established
```

---

# 7. Main Diagnostics Categories

Recommended:

```text
Overview
Network
Messages & Sync
Calls & Media
Files & Transfers
Background Delivery
Storage
Search
Plugins
Security
System
Logs
Support Bundle
Developer Tools
```

---

# 8. Diagnostics Overview

Show high-level health:

```text
Network: Connected
Messages: Healthy
Calls: Ready
Background Delivery: Available
Storage: Healthy
Search: Ready
Plugins: 1 warning
Security: Healthy
```

---

# 9. Health Model

```rust
pub enum DiagnosticHealth {
    Healthy,
    Degraded,
    Attention,
    Failed,
    Unknown,
}
```

---

# 10. Health Must Be Actionable

Bad:

```text
Network: 72%
```

Good:

```text
Network: Relay only
Direct connection unavailable
```

---

# 11. Diagnostic Issue

```rust
pub struct DiagnosticIssueView {
    pub id: DiagnosticIssueId,
    pub area: DiagnosticArea,
    pub severity: DiagnosticSeverity,
    pub title: String,
    pub summary: String,
    pub actions: Vec<DiagnosticActionView>,
}
```

---

# 12. Diagnostic Severity

```rust
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
    Critical,
}
```

---

# 13. Network Diagnostics

Core questions:

```text
Is there Internet?
Is LAN available?
Is nearby discovery available?
Is relay reachable?
Is direct P2P possible?
Is DTN/mesh available?
Which route is currently used?
```

---

# 14. Network Health Snapshot

```rust
pub struct NetworkDiagnosticView {
    pub internet: ConnectivityState,
    pub lan: ConnectivityState,
    pub relay: ConnectivityState,
    pub proximity: ConnectivityState,
    pub mesh: ConnectivityState,
    pub preferred_path: Option<NetworkPathView>,
}
```

---

# 15. Connectivity State

```rust
pub enum ConnectivityState {
    Available,
    Degraded,
    Unavailable,
    Disabled,
    Unknown,
}
```

---

# 16. Network Path Types

```rust
pub enum NetworkPathKind {
    DirectInternet,
    Relay,
    Lan,
    WifiDirect,
    Bluetooth,
    Mesh,
    Dtn,
}
```

---

# 17. Normal Path UX

Show:

```text
Direct
Relay
Local network
Nearby
Offline relay
```

---

# 18. Developer Path UX

May show:

```text
local endpoint
remote endpoint class
relay region
candidate path
RTT
loss
last path switch
```

without revealing private IPs unless explicitly enabled and safe.

---

# 19. Path Visualization

Recommended conceptual diagram:

```text
You
 │
 ├── Direct ─────── Peer
 │
 └── Relay ─────── Relay ─────── Peer
```

or:

```text
You
 ↓
LAN
 ↓
Peer
```

---

# 20. Multi-Path Visualization

If multiple paths active:

```text
Primary: Direct
Fallback: Relay
Nearby: Available
```

---

# 21. Do Not Overanimate Paths

Network path updates can be frequent.

Use stable status cards.

---

# 22. Path Switch Event

Normal UI:

```text
Connection switched to relay
```

only if user is troubleshooting.

Ordinary product UX hides it.

---

# 23. Direct Connection Failure

Possible user-facing explanation:

```text
Direct connection unavailable
Messages can still use relay.
```

---

# 24. Relay Failure

```text
Relay unavailable
Trying direct/local paths
```

---

# 25. No Internet + LAN

Correct status:

```text
Internet unavailable
Local network connection available
```

---

# 26. No Internet + Nearby Mesh

```text
Internet unavailable
Nearby offline relay available
```

---

# 27. No Route

```text
No route currently available
Messages remain queued
```

---

# 28. Peer Reachability Diagnostics

For a selected conversation/contact:

```text
Reachable
Last route
Relay available
Nearby available
Unknown
```

---

# 29. Never Equate Reachability with Presence

Reachability diagnostics are transport state.

Presence is Part 14.

---

# 30. Path History

Developer mode can show recent transitions:

```text
19:32 Direct
19:34 Relay
19:36 LAN
```

bounded history only.

---

# 31. Network Test

Safe user action:

```text
Run Connection Test
```

---

# 32. Connection Test Phases

```text
Checking local network
Checking relay
Checking direct connectivity
Checking nearby capability
```

---

# 33. Test Result

Show recommendations:

```text
Relay works
Direct connections blocked by network
Messaging should still work
```

---

# 34. Do Not Tell Users to Disable Firewall Blindly

If network policy blocks direct traffic:

```text
direct connections may be unavailable
```

Use relay fallback.

---

# 35. Advanced NAT Diagnostics

Developer mode only:

```text
NAT type
candidate gathering
hole-punch attempt
```

if backend exposes safely.

---

# 36. Raw IP Address Privacy

Hidden by default.

---

# 37. Messages & Sync Diagnostics

Questions:

```text
Is the outbox healthy?
Are messages queued?
Is sync caught up?
Are duplicates being suppressed?
Is the local database writable?
```

---

# 38. Message Diagnostic Snapshot

```rust
pub struct MessageDiagnosticView {
    pub outbox_queued: u32,
    pub outbox_failed: u32,
    pub sync_state: SyncHealth,
    pub last_successful_sync: Option<Timestamp>,
    pub storage_writable: bool,
}
```

---

# 39. Sync Health

```rust
pub enum SyncHealth {
    Healthy,
    CatchingUp,
    WaitingForRoute,
    Degraded,
    Failed,
}
```

---

# 40. Queued Messages

Show:

```text
3 messages waiting for connection
```

---

# 41. Failed Messages

Show:

```text
2 messages need attention
```

with:

```text
Open Outbox
Retry
```

---

# 42. Outbox Details

Developer mode may show:

```text
MessageId
retry count
next retry
last error class
route attempts
```

No content preview by default.

---

# 43. Sync Catch-Up

Show:

```text
Syncing recent changes…
```

---

# 44. Last Successful Sync

Useful for support.

---

# 45. Local Database Health

User-facing:

```text
Local data store healthy
```

or:

```text
Local data store needs repair
```

---

# 46. Database Diagnostics

Developer mode:

```text
schema version
migration version
WAL/checkpoint state
database size
integrity result
```

depending actual DB.

---

# 47. Safe Database Check

Action:

```text
Check Local Data
```

must be read-only unless user chooses repair.

---

# 48. Repair Action

Only if backend has safe transactional repair.

---

# 49. Never Offer "Delete Database" as First Fix

Hard rule.

---

# 50. Calls & Media Diagnostics

Questions:

```text
Why is audio bad?
Is microphone available?
Which codec is active?
Is video hardware accelerated?
Is packet loss high?
Is relay being used?
```

---

# 51. Call Diagnostic Snapshot

```rust
pub struct CallDiagnosticView {
    pub call_id: CallId,
    pub route: NetworkPathKind,
    pub rtt_ms: Option<u32>,
    pub jitter_ms: Option<u32>,
    pub packet_loss_pct: Option<f32>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub video_resolution: Option<VideoResolutionView>,
    pub video_fps: Option<f32>,
    pub hw_video_decode: Option<bool>,
    pub hw_video_encode: Option<bool>,
}
```

---

# 52. Normal Call Diagnostics

Show semantic:

```text
Connection: Good
Audio: Good
Video: Reduced quality
Route: Relay
```

---

# 53. Developer Call Diagnostics

Show numeric metrics.

---

# 54. Audio Device Test

Desktop:

```text
Test microphone
Test speaker
```

---

# 55. Android Audio Test

Can test:

```text
microphone permission
audio route availability
```

without awkward loopback if unnecessary.

---

# 56. Camera Test

Preview local camera.

---

# 57. Camera Test Privacy

No media leaves device.

---

# 58. Codec Test

Developer mode:

```text
supported encode/decode codecs
hardware/software paths
```

---

# 59. Call Reconnect History

Developer mode can show:

```text
reconnect count
last reconnect duration
path transitions
```

---

# 60. Files & Transfer Diagnostics

Questions:

```text
Why is file waiting?
Is source available?
Is storage full?
Is verification failing?
```

---

# 61. Transfer Diagnostic View

```rust
pub struct TransferDiagnosticView {
    pub transfer_id: TransferId,
    pub state: TransferState,
    pub route: Option<NetworkPathKind>,
    pub completed_bytes: u64,
    pub total_bytes: u64,
    pub retry_count: u32,
    pub source_available: Option<bool>,
    pub verification_state: Option<BackupVerificationState>,
}
```

---

# 62. User Transfer Diagnostic

Examples:

```text
Waiting for sender
Waiting for Wi-Fi
Paused by battery saver
Not enough storage
Integrity verification failed
```

---

# 63. Developer Transfer Detail

May show:

```text
chunk counts
throughput
resume checkpoint
path
retry backoff
```

---

# 64. Storage Diagnostics

Questions:

```text
How much storage is used?
What is reclaimable?
Is there corruption?
Are temporary files stuck?
```

---

# 65. Storage Health

```rust
pub struct StorageDiagnosticView {
    pub total_used: u64,
    pub free_space: Option<u64>,
    pub reclaimable_cache: u64,
    pub partial_transfer_bytes: u64,
    pub search_index_bytes: u64,
    pub orphaned_blob_count: u64,
    pub health: DiagnosticHealth,
}
```

---

# 66. User Storage Actions

Safe:

```text
Manage Storage
Clear Cache
Clear Partial Transfers
Rebuild Search Index
```

---

# 67. Orphan Cleanup

If backend detects reclaimable unreferenced blobs:

```text
Clean Up
```

after validation.

---

# 68. Do Not Expose CAS Internals

Normal users should not see:

```text
blob hash path
refcount table
chunk directory
```

---

# 69. Search Diagnostics

From Part 11:

```text
Ready
Building
Stale
Corrupt
Disabled
```

---

# 70. Search Diagnostic View

```rust
pub struct SearchDiagnosticView {
    pub state: SearchIndexState,
    pub indexed_messages: Option<u64>,
    pub indexed_files: Option<u64>,
    pub index_size: u64,
    pub last_update: Option<Timestamp>,
}
```

---

# 71. Search Repair

Action:

```text
Rebuild Search Index
```

---

# 72. Background Delivery Diagnostics

Important on Android.

Questions:

```text
Are notifications allowed?
Is push/wake registered?
Is battery restriction blocking work?
Is foreground-call service allowed?
When was last background sync?
```

---

# 73. Background Diagnostic Snapshot

```rust
pub struct BackgroundDiagnosticView {
    pub notification_permission: PermissionDiagnosticState,
    pub background_restricted: bool,
    pub push_registration: ServiceDiagnosticState,
    pub last_background_sync: Option<Timestamp>,
    pub call_background_ready: bool,
}
```

---

# 74. Android Diagnostics Actions

Potential:

```text
Open Notification Settings
Open Battery Settings
Open App Permissions
Test Notification
Run Background Test
```

---

# 75. Background Test

Example:

```text
schedule a safe local diagnostic wake
```

if platform allows.

Do not simulate by sending private network data unnecessarily.

---

# 76. Notification Test

Send:

```text
Test notification
```

clearly labeled as test.

---

# 77. Incoming Call Test

Optional local test mode.

Must not contact another person.

---

# 78. Desktop Background Diagnostics

Show:

```text
daemon running
tray enabled
autostart enabled
UI connected to daemon
```

---

# 79. Daemon State

```rust
pub enum DaemonHealth {
    Running,
    Starting,
    Stopped,
    Failed,
    Unsupported,
}
```

---

# 80. Daemon Restart

Action:

```text
Restart Background Service
```

if safe.

---

# 81. Plugin Diagnostics

Part 19.

Show:

```text
enabled
crashed
slow
quarantined
resource pressure
```

---

# 82. Plugin Detail Diagnostics

Developer:

```text
CPU time
memory
event queue depth
last error
capability violations
```

---

# 83. Core Isolation

Plugin diagnostic failure must not break diagnostic screen.

---

# 84. Security Diagnostics

Show:

```text
Security Center health
unresolved events
device trust status
recovery readiness
```

---

# 85. Security Diagnostics Must Not Dump Keys

Hard rule.

---

# 86. Identity Detail

Developer mode may show:

```text
short AccountId
short DeviceId
key generation/version
```

but never private key material.

---

# 87. Security Event Trace

Useful:

```text
DeviceLinked
DeviceRevoked
IdentityChanged
```

with IDs/time.

---

# 88. System Diagnostics

Potential:

```text
app version
build ID
OS version
architecture
runtime mode
database schema version
protocol versions
available codecs
available transports
```

---

# 89. Android System Diagnostics

Potential:

```text
Android version
ABI
battery restriction
notification permission
camera/mic permission
Bluetooth capability
NFC capability
```

---

# 90. Desktop System Diagnostics

Potential:

```text
OS
Wayland/X11/Windows/macOS
audio backend
GPU renderer
daemon mode
```

---

# 91. Hardware Information Privacy

Do not include unnecessary serial numbers or globally identifying hardware IDs.

---

# 92. Runtime Mode

Show:

```text
Standalone
Daemon
Client/Server
```

if architecture supports multiple modes.

---

# 93. Protocol Compatibility

Developer mode:

```text
message protocol vN
call protocol vN
plugin API vN
backup format vN
```

---

# 94. Diagnostics Search

Developer screen can search:

```text
issue
component
ID
error code
```

---

# 95. Structured Logs

Logs should be structured and level-based.

---

# 96. Log Levels

```rust
pub enum DiagnosticLogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}
```

---

# 97. Default Production Logging

Keep:

```text
Error
Warn
bounded Info
```

with redaction.

---

# 98. Debug Logging

Temporary developer option.

---

# 99. Trace Logging

Very temporary, developer-only.

---

# 100. Sensitive Logging Boundary

Never log:

```text
message plaintext
attachment plaintext
private keys
recovery keys
full auth tokens
SAS
QR device-link payloads
precise emergency location
```

---

# 101. Identifier Redaction

Default support logs may use:

```text
short hashed IDs
```

---

# 102. Log Viewer — Desktop

Developer mode may provide:

```text
filter by component
level
time
search
pause
copy selected
```

---

# 103. Log Viewer — Android

Keep simpler:

```text
recent issues
export diagnostics
```

Full live trace viewer is optional.

---

# 104. Live Event Inspector

Developer desktop feature.

Can inspect typed events:

```text
CallUiEvent
TransferUiEvent
PresenceUiEvent
PluginUiEvent
```

with sensitive fields redacted.

---

# 105. Do Not Expose Internal Event Bus to Plugins

Developer inspector is host-only.

---

# 106. Performance Diagnostics

Potential:

```text
CPU
memory
thread/task counts
event queue sizes
render latency
frame time
database latency
search latency
network throughput
```

---

# 107. User Performance UX

Normal:

```text
App performance is normal
```

or:

```text
Background indexing is using resources
```

---

# 108. Developer Performance Overlay

Desktop:

```text
FPS
render time
memory
event rate
```

---

# 109. Android Performance Diagnostics

May show:

```text
recomposition hotspots? developer-only
battery use class
background work
memory pressure
```

Do not rely on unstable platform internals.

---

# 110. UI Render Health

```rust
pub struct UiPerformanceView {
    pub frame_time_p95_ms: Option<f32>,
    pub dropped_frame_rate: Option<f32>,
    pub event_backlog: u32,
}
```

---

# 111. UI Event Backlog

Important for diagnosing:

```text
progress spam
presence updates
plugin event floods
```

---

# 112. Actor Queue Diagnostics

Rust can expose bounded queue occupancy.

Developer-only.

---

# 113. Backpressure Diagnostics

Show:

```text
Healthy
Approaching limit
Throttling
```

---

# 114. Resource Pressure

```rust
pub enum ResourcePressure {
    Normal,
    Memory,
    Storage,
    Cpu,
    Thermal,
    Battery,
}
```

---

# 115. Android Thermal State

Use semantic classification.

---

# 116. Resource Pressure UX

Normal:

```text
Video quality reduced to protect performance
```

Developer:

```text
Thermal pressure: Severe
```

---

# 117. Diagnostic Actions

Actions should be classified.

---

# 118. Safe Actions

Examples:

```text
Retry
Reconnect
Rebuild Search
Clear Cache
Restart Plugin
Restart Daemon
Run Connection Test
```

---

# 119. Destructive Actions

Examples:

```text
Reset Local Database
Clear All Local Data
Reset Security State
```

should not live in normal diagnostics.

---

# 120. Dangerous Repair Actions

Developer-only and separately confirmed.

---

# 121. Diagnostic Action Model

```rust
pub enum DiagnosticActionView {
    RetryComponent(DiagnosticComponent),
    RunConnectionTest,
    RebuildSearch,
    ClearCache,
    RestartDaemon,
    RestartPlugin(PluginId),
    OpenSystemSettings(SystemSettingsTarget),
    ExportSupportBundle,
}
```

---

# 122. Diagnostics Must Not Guess Repair

Rust decides which actions are safe for issue.

---

# 123. Issue to Action Mapping

Example:

```text
Notification permission denied
→ Open Notification Settings
```

not:

```text
Reset App
```

---

# 124. Support Bundle

One of the most important support features.

---

# 125. Support Bundle Contents

Potential:

```text
app/build version
OS/runtime metadata
diagnostic health snapshots
redacted logs
recent error classes
protocol versions
database schema version
network path summaries
plugin health
```

---

# 126. Excluded by Default

Never include:

```text
message content
file content
private keys
recovery secrets
full contact list
precise location
full IP history
```

---

# 127. Support Bundle Preview

Before export:

```text
View included data
```

---

# 128. User Consent

Explicit:

```text
Create Support Bundle
```

---

# 129. Redaction Level

Potential:

```text
Standard
Extra Private
Developer
```

Recommendation:

```text
Standard
```

only for normal users.

---

# 130. Developer Bundle

May include additional technical identifiers after clear warning.

---

# 131. Bundle Format

Could be:

```text
ZIP/TAR-like diagnostic archive
```

with structured JSON/RON/log files internally.

UI need not care.

---

# 132. Bundle Export — Android

Use SAF/share sheet.

---

# 133. Bundle Export — Desktop

Native Save dialog.

---

# 134. No Automatic Upload

Hard rule unless user explicitly chooses a support provider integration.

---

# 135. Support Upload

If later added:

```text
preview
consent
destination
```

before upload.

---

# 136. Support Bundle Encryption

Optional if support workflow requires.

---

# 137. Bundle Lifetime

Temporary staging cleaned after export/upload.

---

# 138. Diagnostic Snapshot

```rust
pub struct DiagnosticOverviewView {
    pub overall: DiagnosticHealth,
    pub areas: Vec<DiagnosticAreaSummary>,
    pub issues: Vec<DiagnosticIssueView>,
    pub generated_at: Timestamp,
}
```

---

# 139. Diagnostic Areas

```rust
pub enum DiagnosticArea {
    Network,
    Messaging,
    Calls,
    Transfers,
    Background,
    Storage,
    Search,
    Plugins,
    Security,
    System,
}
```

---

# 140. Diagnostics Presentation API

```rust
pub trait DiagnosticsPresentation {
    async fn overview(
        &self,
    ) -> Result<DiagnosticOverviewView, UiError>;

    async fn area(
        &self,
        area: DiagnosticArea,
    ) -> Result<DiagnosticAreaDetailView, UiError>;

    async fn run_action(
        &self,
        action: DiagnosticActionView,
    ) -> Result<DiagnosticActionResult, UiError>;
}
```

---

# 141. Network Diagnostics API

```rust
pub trait NetworkDiagnosticsPresentation {
    async fn snapshot(
        &self,
    ) -> Result<NetworkDiagnosticView, UiError>;

    async fn path_for_peer(
        &self,
        peer: AccountId,
    ) -> Result<PeerPathDiagnosticView, UiError>;

    async fn run_connection_test(
        &self,
    ) -> Result<ConnectionTestView, UiError>;
}
```

---

# 142. Call Diagnostics API

```rust
pub trait CallDiagnosticsPresentation {
    async fn current(
        &self,
        call: CallId,
    ) -> Result<CallDiagnosticView, UiError>;
}
```

---

# 143. Transfer Diagnostics API

```rust
pub trait TransferDiagnosticsPresentation {
    async fn transfer(
        &self,
        id: TransferId,
    ) -> Result<TransferDiagnosticView, UiError>;
}
```

---

# 144. Support Bundle API

```rust
pub trait SupportBundlePresentation {
    async fn preview(
        &self,
        options: SupportBundleOptions,
    ) -> Result<SupportBundlePreviewView, UiError>;

    async fn create(
        &self,
        options: SupportBundleOptions,
    ) -> Result<SupportBundleHandle, UiError>;
}
```

---

# 145. Log Presentation API

```rust
pub trait DiagnosticLogPresentation {
    async fn query(
        &self,
        filter: DiagnosticLogFilter,
    ) -> Result<DiagnosticLogPage, UiError>;

    async fn set_temporary_level(
        &self,
        level: DiagnosticLogLevel,
        duration: Duration,
    ) -> Result<(), UiError>;
}
```

---

# 146. Temporary Debug Level

Debug/trace should expire automatically.

---

# 147. Developer Mode

Unlocks:

```text
IDs
protocol versions
network metrics
logs
event inspector
performance overlay
plugin internals
manual test tools
```

---

# 148. Developer Mode Access

Settings → Advanced → Developer Mode.

---

# 149. Developer Mode Warning

```text
Developer diagnostics may expose technical metadata.
Do not share screenshots or logs without reviewing them.
```

---

# 150. Developer Mode Is Not Security Bypass

It must not reveal:

```text
private keys
recovery secrets
plaintext encrypted blobs
```

---

# 151. Copy Technical ID

Developer mode can copy:

```text
MessageId
ConversationId
AccountId
DeviceId
TransferId
CallId
```

with warning where sensitive.

---

# 152. Short IDs by Default

Display:

```text
a8f2…91c0
```

Full copy only on action.

---

# 153. Protocol Inspector

Developer desktop feature:

```text
active ALPN
protocol version
capability negotiation
```

---

# 154. Packet Inspector

Do not expose raw encrypted packets in normal developer UI by default.

If ever supported:

```text
strictly local
advanced
bounded
```

---

# 155. Event Timeline

Developer mode can correlate:

```text
message queued
route discovered
send attempt
delivery ACK
read cursor
```

without message body.

---

# 156. Correlation ID

Use:

```text
CommandId / MessageId / TransferId / CallId
```

---

# 157. Diagnostics for Local-First State

Show:

```text
local authoritative snapshot
pending sync changes
remote/source-of-truth state if applicable
```

where relevant.

---

# 158. Conflict Diagnostics

Developer mode may show:

```text
conflict count
resolution class
```

not raw private record content.

---

# 159. Backup Diagnostics

Part 16:

```text
last backup
verification
last restore
format compatibility
```

---

# 160. Backup Repair

Only safe verify/retry actions.

---

# 161. Emergency Diagnostics

Part 17:

```text
relay readiness
background permission
location permission
last route
```

No precise location log by default.

---

# 162. Nearby Diagnostics

Part 12:

```text
Bluetooth available
Wi-Fi nearby available
NFC available
camera scanner available
```

---

# 163. Permission Diagnostics

Android:

```text
Notifications
Camera
Microphone
Nearby
Location
```

Show current state.

---

# 164. Permission Action

```text
Open Settings
```

where permission can no longer be requested inline.

---

# 165. Desktop Permission Diagnostics

Where relevant:

```text
Camera
Microphone
Screen capture
Notifications
```

---

# 166. System Clock

If severe clock skew detected:

```text
Device time appears incorrect
```

can matter for signatures/expiry.

---

# 167. Clock Diagnostic

Do not claim authoritative external time if unavailable.

---

# 168. Disk Time/Filesystem Errors

Show semantic:

```text
Storage unavailable
```

---

# 169. Network DNS

If relay hostname lookup fails:

```text
Network name resolution failed
```

developer detail only if helpful.

---

# 170. TLS/Certificate Diagnostics

If external integration fails:

```text
Secure connection could not be established
```

No bypass button for invalid certificate in normal UX.

---

# 171. Plugin Network Diagnostics

Plugin network errors remain plugin-scoped.

---

# 172. Core Relay Diagnostics

Core transport separate from plugin integrations.

---

# 173. Diagnostics Refresh

Use:

```text
live updates
+
manual refresh
```

depending area.

---

# 174. Event Rate

Do not update full diagnostics tree on every packet.

---

# 175. Snapshot Frequency

Normal overview:

```text
1–5 seconds
```

while visible.

Developer live metrics can be faster but bounded.

---

# 176. Background Diagnostics Screen

When not visible:

```text
stop expensive polling
```

---

# 177. Event-Driven Preferred

Rust emits health changes.

UI samples metrics only for graphs/live views.

---

# 178. Graphs

Use sparingly.

Potential:

```text
RTT over time
transfer throughput
memory usage
```

developer-only.

---

# 179. No Sparkline Overload

Normal users need semantic status.

---

# 180. Android Diagnostics Layout

Recommended:

```text
Overview cards
Issues
Network
Background
Storage
System
Export Diagnostics
```

Developer sections nested deeper.

---

# 181. Desktop Diagnostics Layout

Recommended:

```text
Left navigation
Overview
Network
Messages
Calls
Transfers
Storage
Search
Plugins
Security
Logs
Developer
```

Wide detail panels/tables.

---

# 182. Tablet/Foldable

List/detail.

---

# 183. Accessibility — Diagnostic Health

Screen reader:

```text
Network, degraded. Relay available. Direct connection unavailable.
```

---

# 184. Diagnostic Issue Accessibility

Title + cause + action.

---

# 185. Charts Accessibility

Provide textual summary.

---

# 186. Large Font

Tables should degrade to stacked rows.

---

# 187. RTL

Diagnostic prose mirrors.

Technical IDs/log lines preserve canonical direction.

---

# 188. Reduced Motion

No animated network topology required.

---

# 189. Color Independence

Healthy/degraded/error always include:

```text
text
icon
```

---

# 190. Keyboard Desktop

Support:

```text
Ctrl/Cmd+F
arrow navigation
Enter
copy selected ID
pause/resume logs
```

---

# 191. Android TalkBack

All test/repair actions clearly labeled.

---

# 192. Diagnostic Action Confirmation

Safe read-only tests need no confirmation.

---

# 193. Stateful Repair

Examples:

```text
restart daemon
clear cache
rebuild index
```

can use light confirmation.

---

# 194. Data-Destructive Repair

Not part of normal diagnostics.

---

# 195. Testing Matrix

Required:

```text
healthy system
relay-only network
LAN-only
no route
queued messages
failed outbox
call packet loss
transfer storage full
search corrupt
Android notification denied
daemon stopped
plugin crash
security warning
```

---

# 196. Android Tests

Verify:

```text
permission diagnostics
battery restriction
background delivery
notification test
process death
TalkBack
large font
SAF support bundle export
```

---

# 197. Desktop Tests

Verify:

```text
daemon diagnostics
network path view
log viewer
keyboard
support bundle
developer mode
multi-window
```

---

# 198. Privacy Tests

Ensure support bundle excludes:

```text
message content
file content
keys
recovery secrets
precise emergency location
```

---

# 199. Redaction Tests

Known sensitive fields are replaced/omitted.

---

# 200. Developer Mode Tests

Enabling developer mode exposes technical metadata but never secrets.

---

# 201. Connection Test

Verify no destructive side effects.

---

# 202. Repair Tests

Rebuild search/clear cache/restart daemon do not damage authoritative data.

---

# 203. Scale Tests

Diagnostics remain responsive with:

```text
many transfers
many plugins
large logs
large message outbox
```

---

# 204. Log Paging

Cursor-based.

---

# 205. Live Log Backpressure

If UI cannot keep up:

```text
drop/coalesce debug entries
```

never block core runtime.

---

# 206. Diagnostic Event Priority

Diagnostics are lower priority than:

```text
calls
messages
security
emergency
```

---

# 207. Telemetry vs Diagnostics

Diagnostics are primarily local.

Telemetry, if any, is a separate opt-in/product policy.

---

# 208. No Automatic Support Upload

Hard rule.

---

# 209. Exported Support Bundle

User controls where it goes.

---

# 210. Diagnostic Persistence

Health state may be current/recent.

Do not retain detailed traces forever.

---

# 211. Retention

Example:

```text
recent errors: 7 days
debug logs: 1 day
trace logs: temporary
```

depending policy.

---

# 212. Crash Diagnostics

Crash record can include:

```text
component
build
stack trace if available
redacted context
```

---

# 213. Plugin Crash Attribution

If safe/confident:

```text
Plugin X crashed
```

---

# 214. Core Crash Attribution

Do not blame network/plugin without evidence.

---

# 215. User-Facing Root Cause

Prefer:

```text
Background delivery is restricted by Android
```

over vague:

```text
Something went wrong
```

---

# 216. Confidence

If cause uncertain:

```text
Possible cause
```

rather than certainty.

---

# 217. Diagnostic Recommendation Model

```rust
pub struct DiagnosticRecommendationView {
    pub title: String,
    pub reason: String,
    pub action: Option<DiagnosticActionView>,
    pub confidence: DiagnosticConfidence,
}
```

---

# 218. Diagnostic Confidence

```rust
pub enum DiagnosticConfidence {
    Confirmed,
    Likely,
    Possible,
}
```

---

# 219. Example

```text
Likely cause:
Android battery restrictions may be delaying background delivery.
```

---

# 220. Do Not Encourage Risky System Changes

Avoid:

```text
disable firewall
disable OS security
turn off battery protection globally
```

unless narrowly justified.

---

# 221. Suggested Fix Order

Prefer:

```text
app-local safe action
platform setting
advanced diagnostics
```

---

# 222. Initial Production Scope

Ship:

```text
diagnostics overview
network/direct/relay/LAN status
connection test
message/outbox health
call quality diagnostics
transfer diagnostics
Android background/notification permission diagnostics
desktop daemon health
storage/search health
plugin health
security summary
support bundle export
developer mode
structured logs
safe repair actions
```

Defer:

```text
raw packet capture UI
full protocol debugger
remote live-support tunnel
automatic support upload
deep OS kernel diagnostics
```

unless explicitly needed.

---

# 223. Definition of Done

UI/UX Part 20 is complete when:

- user diagnostics and developer diagnostics are separate
- overview provides actionable semantic health rather than meaningless scores
- direct, relay, LAN, nearby, mesh, and DTN paths are represented without confusing normal users
- network path visualization does not expose raw IPs by default
- message/outbox/sync health is visible without message content
- call/media diagnostics provide semantic quality normally and numeric metrics in developer mode
- transfer/storage/search/plugin/security/background-delivery health are integrated
- Android notification/battery/background restrictions can be diagnosed and linked to system settings
- desktop daemon/tray/runtime health is explicit
- support bundles are previewable, redacted, local-first, and never uploaded automatically
- debug/trace logging is temporary and cannot capture secrets
- developer mode exposes IDs/protocol versions/metrics but not cryptographic private material
- safe repair actions are Rust-provided and clearly separated from destructive operations
- diagnostics cannot starve calls/messages/security/emergency traffic
- accessibility, RTL, large font, chart summaries, keyboard/TalkBack, and color independence are defined
- Rust diagnostics, network, call, transfer, support-bundle, and log presentation APIs are specified
- privacy, redaction, backpressure, scale, repair, platform-permission, and developer-mode tests are included

---

# 224. Final Architecture

```text
                    RUST OBSERVABILITY CORE
                             │
       ┌─────────────────────┼─────────────────────┐
       │                     │                     │
    Health                Metrics                Logs
       │                     │                     │
 Network/Sync         Call/Transfer/CPU      Structured/Redacted
 Storage/Search       Queue/Latency          Bounded Retention
       │                     │                     │
       └─────────────────────┼─────────────────────┘
                             │
                 Diagnostics Presentation
                    ┌────────┴────────┐
                    │                 │
                 Dioxus            Compose
                    │                 │
           Desktop Diagnostics   Android Diagnostics
```

Support path:

```text
Local Diagnostic State
      │
      ▼
Redaction
      │
      ▼
Preview
      │
      ▼
User Explicit Export
```

Never:

```text
diagnostics
→ automatic upload of private logs/content
```

---

# 225. Final Principle

Diagnostics should make failures understandable without weakening the privacy or security model.

The correct approach is:

```text
semantic health
+
actionable root cause
+
safe repair
+
developer detail when requested
+
strict redaction
```

not:

```text
dump every internal log and network detail onto the user
```

This gives Dioxus desktop and Android Compose a production-grade troubleshooting surface while keeping the Rust core authoritative for health, metrics, path state, repair actions, and privacy boundaries.
