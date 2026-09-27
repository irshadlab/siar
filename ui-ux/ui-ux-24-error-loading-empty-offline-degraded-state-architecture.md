# UI/UX Part 24 — Error, Loading, Empty, Offline & Degraded-State UX Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 24  
**Desktop UI:** Dioxus  
**Android UI:** Kotlin + Jetpack Compose  
**Core runtime:** Rust  
**Primary purpose:** define the complete error/loading/empty/offline/degraded-state UX architecture across messaging, calls, files, search, contacts, groups, pairing, notifications, security, backup, emergency, settings, plugins, diagnostics, and all future product surfaces.

---

# 1. Purpose

Failure states are part of the normal product, especially in a local-first P2P system.

Users will regularly encounter:

```text
no Internet
peer unavailable
relay unavailable
local-only connectivity
background delivery delayed
storage pressure
permission denied
partial sync
stale data
retrying transfer
search index rebuilding
plugin crashed
backup destination missing
camera unavailable
message queued
call reconnecting
```

These should not all become:

```text
Something went wrong
```

The governing principle is:

> **The UX must explain what is known, what still works, what is waiting, and what the user can do next.**

---

# 2. Architectural Position

```text
Rust Domain / Runtime State
        │
        ├── loading phase
        ├── data readiness
        ├── connectivity
        ├── degradation
        ├── failure class
        ├── retryability
        ├── recovery action
        └── data-safety state
        │
        ▼
Failure-State Presentation Service
        │
   ┌────┴─────┐
   │          │
Dioxus     Compose
Desktop    Android
```

---

# 3. Core State Families

Recommended:

```rust
pub enum UiDataState<T> {
    Loading(LoadingStateView),
    Ready(T),
    Empty(EmptyStateView),
    Degraded {
        data: T,
        degradation: DegradedStateView,
    },
    Failed(ErrorStateView),
}
```

---

# 4. Do Not Treat Degraded as Failed

Example:

```text
No Internet
LAN messaging still works
```

is:

```text
Degraded
```

not:

```text
Failed
```

---

# 5. Loading Taxonomy

Loading should describe phase when useful.

```rust
pub enum LoadingPhase {
    Starting,
    ReadingLocalData,
    Connecting,
    Syncing,
    Verifying,
    Preparing,
    Rebuilding,
    WaitingForPermission,
}
```

---

# 6. Initial Loading

Use only when no usable data exists yet.

---

# 7. Local-First Loading

Preferred startup:

```text
show local snapshot immediately
then sync in background
```

instead of blocking on network.

---

# 8. Skeleton Loading

Use only when structure is known.

Examples:

```text
conversation list
contact list
file grid
```

---

# 9. Spinner Loading

Use for small indeterminate operations.

---

# 10. Progress Loading

Use when meaningful total exists.

Examples:

```text
backup
restore
transfer
index rebuild
```

---

# 11. Loading Copy

Prefer:

```text
Loading messages…
Verifying backup…
Rebuilding search…
```

over generic:

```text
Loading…
```

---

# 12. Avoid Fake Progress

If total unknown:

```text
indeterminate
```

---

# 13. Loading Timeout

Long loading should transition to explanatory state.

Example:

```text
Still trying to connect…
```

with options.

---

# 14. Background Loading

Do not block navigation for background:

```text
sync
index
thumbnail generation
```

unless required.

---

# 15. Refresh State

When existing data visible:

```text
keep content
show small refresh indicator
```

---

# 16. Empty State Taxonomy

Empty is not failure.

Examples:

```text
No conversations yet
No contacts
No files
No search results
No blocked users
No security events
No plugins installed
```

---

# 17. Empty State Structure

Recommended:

```text
Title
Explanation
Primary action if useful
Secondary guidance if useful
```

---

# 18. Good Empty State

```text
No contacts yet
Add someone by QR, nearby, or invite code.
[Add Contact]
```

---

# 19. Bad Empty State

```text
Nothing here
```

---

# 20. Search Empty State

Differentiate:

```text
No results
```

from:

```text
Search index still building
```

---

# 21. Inbox Empty State

Could be:

```text
No conversations yet
```

not:

```text
No messages found
```

---

# 22. Filtered Empty State

Example:

```text
No unread conversations
```

with:

```text
Clear filter
```

---

# 23. Offline State

Offline is a connectivity condition, not global product failure.

---

# 24. Offline Modes

Potential:

```rust
pub enum OfflineModeView {
    InternetUnavailable,
    LocalNetworkOnly,
    NearbyOnly,
    DtnOnly,
    FullyDisconnected,
}
```

---

# 25. Internet Unavailable

If relay/LAN/nearby remains:

```text
No Internet
Local communication may still work
```

---

# 26. Local Network Only

Show:

```text
Connected locally
Internet services unavailable
```

---

# 27. Nearby Only

Show:

```text
Nearby messaging available
```

---

# 28. DTN Only

Show:

```text
Messages will be stored and forwarded when a route appears
```

only if true.

---

# 29. Fully Disconnected

Show:

```text
No connection right now
Messages will remain queued
```

---

# 30. Offline Banner

Should be compact and non-blocking.

---

# 31. Offline Banner Content

Example:

```text
No Internet — local messaging is still available
```

---

# 32. Do Not Spam Offline Errors

One persistent state is better than per-action error popups.

---

# 33. Queued Message UX

User sends offline:

```text
message appears immediately
Queued
```

---

# 34. Offline Success Semantics

The action can succeed locally even if network delivery waits.

---

# 35. Degraded State

Examples:

```text
relay only
background restricted
low storage
search stale
plugin disabled
camera unavailable
call audio-only fallback
```

---

# 36. Degradation Model

```rust
pub struct DegradedStateView {
    pub area: DegradedArea,
    pub reason: DegradedReason,
    pub impact: String,
    pub actions: Vec<RecoveryActionView>,
}
```

---

# 37. Degraded Area

```rust
pub enum DegradedArea {
    Network,
    Messaging,
    Calls,
    Files,
    Search,
    Background,
    Storage,
    Security,
    Plugins,
    Backup,
}
```

---

# 38. Degraded Reason

Examples:

```text
RelayOnly
StorageLow
IndexStale
PermissionDenied
BackgroundRestricted
PluginUnavailable
```

---

# 39. Impact Must Be Clear

Example:

```text
Messages still work, but file transfers may be slower.
```

---

# 40. Error Taxonomy

Errors should be typed.

```rust
pub enum UiErrorKind {
    Validation,
    Permission,
    Connectivity,
    Timeout,
    NotFound,
    Conflict,
    Storage,
    Security,
    Compatibility,
    RateLimited,
    Cancelled,
    Internal,
}
```

---

# 41. Recoverability

```rust
pub enum ErrorRecoverability {
    Retryable,
    ActionRequired,
    Terminal,
}
```

---

# 42. Data Safety

Critical dimension.

```rust
pub enum DataSafetyState {
    Safe,
    Pending,
    AtRisk,
    Unknown,
}
```

---

# 43. Error View

```rust
pub struct ErrorStateView {
    pub kind: UiErrorKind,
    pub title: String,
    pub explanation: String,
    pub recoverability: ErrorRecoverability,
    pub data_safety: DataSafetyState,
    pub actions: Vec<RecoveryActionView>,
}
```

---

# 44. Error Copy Should Answer

```text
What failed?
Is my data safe?
What can I do?
```

---

# 45. Example — Message Send Failed

```text
Message couldn't be sent
Your draft is safe.
[Retry]
```

---

# 46. Example — Backup Failed

```text
Backup failed
Your previous verified backup is unchanged.
[Retry]
```

---

# 47. Example — Restore Failed

```text
Restore could not be completed
Your current data was not changed.
```

if transaction rollback succeeded.

---

# 48. Internal Error

Normal users do not need stack traces.

Show:

```text
Something failed unexpectedly
Your data is safe
Retry
Export Diagnostics
```

where appropriate.

---

# 49. Inline vs Banner vs Blocking

Use different surfaces.

---

# 50. Inline Error

Use for local component/input issue.

Examples:

```text
invalid invite code
attachment too large
wrong recovery key
```

---

# 51. Banner

Use for screen-level persistent degradation.

Examples:

```text
offline
background restricted
search index stale
```

---

# 52. Snackbar/Toast

Use for short transient feedback.

Examples:

```text
Copied
Archived
Retry started
```

---

# 53. Dialog

Use when user decision required.

Examples:

```text
storage full during restore
device revoke confirmation
```

---

# 54. Full-Screen Blocking Error

Reserved for:

```text
account revoked
critical storage corruption
unsupported app state
security lockout
```

---

# 55. Error Surface Escalation

Start with least disruptive surface compatible with severity.

---

# 56. Retry Architecture

Retry must be idempotent where possible.

---

# 57. Retry Action

```rust
pub enum RecoveryActionView {
    Retry,
    Refresh,
    Reconnect,
    OpenSettings,
    FreeStorage,
    RebuildSearch,
    ChooseAnotherDestination,
    Reauthenticate,
    UpdateApp,
    ExportDiagnostics,
    ContactSupport,
}
```

---

# 58. Retry Button

Do not create duplicate messages/transfers/jobs.

---

# 59. Automatic Retry

Background retry can continue silently for:

```text
message delivery
sync
relay connection
transfer resume
```

---

# 60. Manual Retry

Expose when user action adds value.

---

# 61. Backoff

Do not show technical backoff values normally.

---

# 62. Retry Countdown

If rate-limited:

```text
Try again in 30 seconds
```

if reliable.

---

# 63. Rate Limit

Do not repeatedly enable a button known to fail.

---

# 64. Cancelled Is Not Error

User cancellation:

```text
Cancelled
```

should not show failure styling unless partial risk exists.

---

# 65. Timeout

Explain what timed out.

Example:

```text
Could not reach this device
Try again
```

---

# 66. Network Timeout With Fallback

If relay fallback exists:

```text
Direct connection failed
Using relay instead
```

degraded, not failed.

---

# 67. Partial Data

Local-first systems may have incomplete history.

---

# 68. Partial Data State

Example:

```text
Recent messages available
Older history is still syncing
```

---

# 69. Partial Search

Example:

```text
Search results may be incomplete while history syncs
```

---

# 70. Partial File Availability

Example:

```text
File metadata available
Content not downloaded
```

---

# 71. Stale Data

Some state can be usable but old.

---

# 72. Stale Presence

Use:

```text
Last seen recently
```

not falsely current.

---

# 73. Stale Search Index

Show:

```text
Some recent content may be missing from search
```

---

# 74. Stale Device List

High-risk security state should refresh before destructive action.

---

# 75. Stale Settings

Rust revalidates before commit.

---

# 76. Stale Restore Plan

Show:

```text
This restore plan is no longer current
Review again
```

---

# 77. Permission Failure

Must explain:

```text
what permission enables
what still works
```

---

# 78. Camera Denied

```text
Camera access is off
You can enter the code manually.
```

---

# 79. Microphone Denied

```text
Microphone access is required for voice notes
Text messaging still works.
```

---

# 80. Notification Denied

```text
You may miss message and call alerts in the background.
```

---

# 81. Nearby Denied

```text
Nearby discovery is unavailable
QR/manual code still work.
```

---

# 82. Location Denied in SOS

```text
SOS will be sent without location.
```

---

# 83. Permission Recovery

Offer:

```text
Allow
Open Settings
Use Alternative
```

---

# 84. Storage Pressure

Important degraded state.

---

# 85. Storage Levels

```rust
pub enum StoragePressureState {
    Normal,
    Low,
    Critical,
    Full,
}
```

---

# 86. Low Storage

```text
Storage is running low
Large downloads may be paused.
```

---

# 87. Critical Storage

```text
Storage is critically low
Backups/transfers may fail.
```

---

# 88. Storage Full

Block operations that cannot safely continue.

---

# 89. Storage Recovery

Offer:

```text
Manage Storage
Clear Cache
Clear Partial Transfers
Choose Another Destination
```

---

# 90. Do Not Auto-Delete Authoritative Data

Hard rule.

---

# 91. Search Rebuild State

```text
Search is rebuilding
Messaging remains available.
```

---

# 92. Search Failure

If index corrupt:

```text
Search unavailable
Rebuild Index
```

---

# 93. Plugin Failure

Plugin-specific.

---

# 94. Plugin Crashed

```text
Translator Plugin stopped unexpectedly
Core messaging is unaffected.
```

---

# 95. Plugin Quarantined

```text
Plugin disabled for security reasons
```

---

# 96. Plugin Extension Missing

Core UI remains usable.

---

# 97. Background Delivery Degraded

Android example:

```text
Background delivery may be delayed
Battery restrictions are enabled.
```

---

# 98. Foreground Still Works

Explain:

```text
Messages will sync when the app is open.
```

if true.

---

# 99. Daemon Stopped Desktop

```text
Background service is stopped
Notifications and background delivery are unavailable.
```

---

# 100. Restart Daemon

Safe recovery action.

---

# 101. Call Degradation

Calls often degrade gradually.

---

# 102. Call States

Examples:

```text
Video quality reduced
Audio only
Reconnecting
Relay connection
Camera unavailable
```

---

# 103. Call Reconnecting

Show:

```text
Reconnecting…
```

while preserving same CallId.

---

# 104. Call Failure

After retry window:

```text
Call ended because connection was lost
```

---

# 105. Camera Failure During Call

```text
Camera unavailable
Audio call continues.
```

---

# 106. Microphone Failure

Critical for call.

Show actionable:

```text
Microphone unavailable
Check permission/device
```

---

# 107. File Transfer Degradation

Examples:

```text
Waiting for sender
Paused on mobile data
Waiting for Wi-Fi
Storage full
Verification failed
```

---

# 108. Waiting Is Not Failure

Hard rule.

---

# 109. Transfer Verification Failure

Strong:

```text
File could not be verified
It was not opened or marked complete.
```

---

# 110. Backup Degradation

Examples:

```text
destination unavailable
previous backup still safe
backup stale
verification pending
```

---

# 111. Restore Blocking Failure

If compatibility/security fails:

```text
restore blocked
```

not degraded.

---

# 112. Emergency Degraded State

Examples:

```text
No Internet — nearby relay available
No route right now — SOS stored
Location unavailable
```

---

# 113. Emergency Error Honesty

Never show:

```text
SOS failed
```

if it is safely stored and still eligible for relay.

---

# 114. Security Failure

Security errors should fail closed.

---

# 115. Verification Mismatch

```text
Codes do not match
Do not continue.
```

---

# 116. Identity Changed

Persistent warning until reviewed.

---

# 117. Device Revoked

Blocking state:

```text
This device no longer has access.
```

---

# 118. Recovery Invalid

Critical.

---

# 119. Error Severity

```rust
pub enum UiErrorSeverity {
    Info,
    Warning,
    Error,
    Critical,
}
```

---

# 120. Severity Does Not Determine Surface Alone

A critical background security issue may need banner/notification/full screen depending context.

---

# 121. State Persistence

Errors must survive restart if underlying issue persists.

---

# 122. Transient Errors

Do not persist forever.

Example:

```text
temporary relay timeout
```

---

# 123. Durable Errors

Examples:

```text
security warning
backup failed
device revoked
storage corruption
```

may persist.

---

# 124. Error Identity

```rust
pub struct UiIssueId(pub Uuid);
```

Useful for deduplication.

---

# 125. Duplicate Errors

Do not show repeated identical banners/snackbars.

---

# 126. Error Coalescing

Example:

```text
10 failed message retries
```

can become:

```text
3 messages need attention
```

---

# 127. Burst Failures

Plugin/network storms should not create notification storm.

---

# 128. Recovery Progress

After user acts:

```text
Reconnecting…
Rebuilding…
Retrying…
```

---

# 129. Recovery Success

Use concise confirmation:

```text
Connected
Search rebuilt
Plugin restarted
```

---

# 130. Recovery Failure

Keep prior issue visible with updated reason.

---

# 131. Loading + Error Transition

A component should not oscillate:

```text
loading → error → loading → error
```

every retry tick.

Use stable retry state.

---

# 132. Error Hysteresis

For flaky connectivity, avoid banner flicker.

---

# 133. Offline Hysteresis

Short network blip may not immediately show full offline banner.

---

# 134. Foreground vs Background

Foreground can show more detail.

Background may rely on Part 13 notifications.

---

# 135. Notification Failure

If notification permission unavailable:

```text
in-app warning
```

not repeated system popups.

---

# 136. Startup Error Architecture

Startup phases:

```text
Loading local state
Starting runtime
Opening database
Restoring session
```

---

# 137. Startup Local Data Failure

If DB temporarily locked:

```text
Retrying local data…
```

---

# 138. Startup Corruption

Blocking screen with safe options:

```text
Run Repair
Restore Backup
Export Diagnostics
```

if supported.

---

# 139. Startup Network Failure

Should not block local-first app startup.

---

# 140. Startup Plugin Failure

Start without plugin if possible.

---

# 141. Safe Mode

From Part 19.

---

# 142. Crash Recovery

After crash:

```text
restore durable state
preserve drafts
resume jobs where safe
```

---

# 143. Crash Message

If needed:

```text
The app restarted unexpectedly
Your messages are safe.
```

only if known.

---

# 144. Unknown Data Safety

If uncertain:

```text
We could not confirm whether the last operation completed.
Review status before retrying.
```

---

# 145. Optimistic UI Failure

Example message send:

```text
pending row remains
failed state
retry
```

---

# 146. Rollback

For reversible settings/action failure:

```text
restore previous state
```

---

# 147. High-Risk Action Failure

Do not optimistically commit.

---

# 148. Error Presentation API

```rust
pub trait FailureStatePresentation {
    async fn issue(
        &self,
        id: UiIssueId,
    ) -> Result<ErrorStateView, UiError>;

    async fn recover(
        &self,
        id: UiIssueId,
        action: RecoveryActionView,
    ) -> Result<RecoveryResultView, UiError>;
}
```

---

# 149. Global Degradation API

```rust
pub trait DegradationPresentation {
    async fn current(
        &self,
    ) -> Result<Vec<DegradedStateView>, UiError>;
}
```

---

# 150. Loading View

```rust
pub struct LoadingStateView {
    pub phase: LoadingPhase,
    pub progress: Option<f32>,
    pub cancellable: bool,
}
```

---

# 151. Empty View

```rust
pub struct EmptyStateView {
    pub kind: EmptyStateKind,
    pub primary_action: Option<EmptyStateAction>,
}
```

---

# 152. Empty State Kind

```rust
pub enum EmptyStateKind {
    NoConversations,
    NoContacts,
    NoFiles,
    NoSearchResults,
    NoSecurityEvents,
    NoPlugins,
    NoBackups,
    NoBlockedContacts,
}
```

---

# 153. Offline Snapshot

```rust
pub struct OfflineStateView {
    pub mode: OfflineModeView,
    pub queued_messages: u32,
    pub local_features_available: Vec<OfflineCapability>,
}
```

---

# 154. Offline Capability

Examples:

```text
LANMessaging
NearbyMessaging
ReadLocalHistory
CreateDrafts
SearchLocal
ViewFiles
```

---

# 155. UI Event Model

```rust
pub enum FailureUiEvent {
    IssueAdded(ErrorStateView),
    IssueUpdated(ErrorStateView),
    IssueResolved(UiIssueId),
    DegradationChanged(Vec<DegradedStateView>),
    OfflineChanged(OfflineStateView),
}
```

---

# 156. Android ViewModel

Owns:

```text
banner visibility
snackbar queue
permission-navigation effects
retry button presentation
```

Rust owns:

```text
failure semantics
retryability
data safety
recovery action validity
```

---

# 157. Dioxus Presenter

Owns:

```text
inline errors
desktop banners
dialogs
focus after recovery
```

---

# 158. Do Not Infer Retryability in UI

Hard rule.

---

# 159. Do Not Infer Data Safety in UI

Hard rule.

---

# 160. Error Localization

Rust can expose typed reason.

UI localizes natural explanation.

---

# 161. Error Codes

Developer diagnostics may expose stable:

```text
E_NET_RELAY_UNAVAILABLE
```

Normal UI does not.

---

# 162. Error Correlation ID

Support may use:

```text
Issue ID
```

without private content.

---

# 163. Accessibility — Loading

Screen reader announces:

```text
Loading conversations
```

once.

---

# 164. Loading Progress Accessibility

Do not announce every percent.

---

# 165. Accessibility — Empty

Example:

```text
No contacts yet. Add a contact.
```

---

# 166. Accessibility — Error

Example:

```text
Backup failed. Previous backup is safe. Retry button.
```

---

# 167. Accessibility — Offline

```text
No Internet. Nearby messaging is available.
```

---

# 168. Focus on Error

Do not steal focus for ordinary inline/banner errors.

---

# 169. Blocking Error

Focus moves to heading/actions.

---

# 170. Snackbar Accessibility

Must remain long enough and expose action.

---

# 171. Color Independence

All failure states use:

```text
text
icon
semantic label
```

not red/yellow alone.

---

# 172. Large Text

Error explanations wrap.

---

# 173. RTL

Localized error text mirrors.

Technical codes preserve direction.

---

# 174. Reduced Motion

Loading/retry animations can become static.

---

# 175. Cognitive Load

Show one primary recovery action.

Secondary actions beneath.

---

# 176. Error Message Style

Avoid:

```text
Unknown error
Invalid state
Operation failed
```

when more useful semantic reason exists.

---

# 177. Avoid Blame

Do not say:

```text
You failed to…
```

Prefer:

```text
Camera access is off
```

---

# 178. Offline Wording

Avoid:

```text
You're offline
```

if local/mesh routes exist.

---

# 179. Partial Success Wording

Example:

```text
Message saved locally and will send when a route is available.
```

---

# 180. Safety Wording

Example:

```text
Your draft is safe.
```

only when Rust confirms.

---

# 181. Diagnostics Link

Persistent/repeated failures may offer:

```text
View Diagnostics
```

---

# 182. Support Bundle Link

For unexplained failures:

```text
Export Diagnostics
```

---

# 183. No Automatic Error Upload

Hard rule.

---

# 184. Error Telemetry

Do not log private payloads.

---

# 185. Safe Metrics

Possible:

```text
error class
retry count
recovery success
loading latency
```

without content/identity.

---

# 186. Crash Reports

Redacted.

---

# 187. Testing Matrix

Required:

```text
initial loading
background refresh
empty
filtered empty
offline Internet-only
LAN-only
nearby-only
fully disconnected
degraded relay-only
permission denied
storage low/full
search stale/corrupt
plugin crash
background restricted
```

---

# 188. Messaging Tests

```text
queued
sending
failed
retry
duplicate retry
offline send
```

---

# 189. Call Tests

```text
reconnecting
relay-only
camera failure
microphone failure
network loss
```

---

# 190. File Tests

```text
waiting for sender
Wi-Fi-only pause
storage full
verification failure
```

---

# 191. Backup Tests

```text
destination unavailable
storage full
verify fail
retry
previous backup safe
```

---

# 192. Security Tests

```text
identity mismatch
device revoked
recovery invalid
```

---

# 193. Android Tests

```text
permission denial
battery restriction
process death
background delivery degraded
TalkBack
large text
```

---

# 194. Desktop Tests

```text
daemon stopped
network loss
window restore
keyboard
safe mode
```

---

# 195. Accessibility Tests

All recovery actions usable via:

```text
TalkBack
keyboard
large text
RTL
```

---

# 196. Error Dedup Tests

Repeated same issue shows one persistent state.

---

# 197. Hysteresis Tests

Short network flaps do not create banner flicker.

---

# 198. Data Safety Tests

Every destructive/failed operation reports correct:

```text
Safe
Pending
AtRisk
Unknown
```

---

# 199. Loading Performance

Local snapshot should render quickly.

---

# 200. Error Performance

Failure rendering must not block core recovery work.

---

# 201. Event Flood

Hundreds of retry updates should be coalesced.

---

# 202. Initial Production Scope

Ship:

```text
typed loading states
meaningful empty states
offline/local-only banner
degraded-state model
typed error classes
retryability
data-safety semantics
permission failures
storage pressure
sync/queue states
call degradation
transfer waiting/failure
backup/search/plugin degradation
startup/crash recovery states
accessibility
```

Defer:

```text
complex predictive remediation
AI-generated error explanations
automatic destructive repair
```

---

# 203. Definition of Done

UI/UX Part 24 is complete when:

- loading, empty, offline, degraded, failed, and cancelled states are distinct
- local-first startup renders usable local data before network sync where possible
- empty states explain what is empty and provide appropriate next action
- no-Internet state does not imply total failure when LAN/nearby/DTN still works
- degraded states clearly explain impact and what continues to work
- errors are typed by category, recoverability, severity, and data-safety state
- retry actions are Rust-authoritative and idempotent
- user cancellation is not styled as failure
- permission failures always provide alternatives where available
- storage pressure never causes silent deletion of authoritative data
- queued/waiting states are not mislabeled as failure
- security failures fail closed
- startup network failure does not block local-first operation
- plugin failure cannot take down core UX
- duplicate errors and network flaps are coalesced/hysteresis-controlled
- accessibility, large text, RTL, reduced motion, and screen-reader semantics are defined
- Rust failure/degradation/offline presentation contracts are specified
- data-safety wording is shown only when confirmed by Rust
- offline/degraded/permission/storage/crash/retry/deduplication tests are included

---

# 204. Final Architecture

```text
                     RUST PRODUCT STATE
                              │
      ┌───────────────────────┼───────────────────────┐
      │                       │                       │
   Loading                 Degraded                Failed
      │                       │                       │
 Phase/Progress        Impact/What Works      Cause/Recovery
      │                       │                       │
      └───────────────────────┼───────────────────────┘
                              │
                 Failure-State Presentation
                    ┌─────────┴─────────┐
                    │                   │
                 Dioxus              Compose
                 Desktop             Android
```

The key UX sequence is:

```text
What happened?
→ What still works?
→ Is my data safe?
→ What should I do next?
```

---

# 205. Final Principle

Failure states should reduce uncertainty rather than add to it.

The correct model is:

```text
typed state
+
clear impact
+
honest data-safety status
+
one useful recovery path
+
local-first continuity
```

not:

```text
spinner
→ generic error
→ retry
→ generic error
```

This gives Dioxus desktop and Android Compose resilient behavior that remains understandable even when connectivity, permissions, storage, background execution, plugins, or other subsystems are partially unavailable.
