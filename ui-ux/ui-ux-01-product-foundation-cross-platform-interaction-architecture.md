# UI/UX Part 01 — Product UX Foundation & Cross-Platform Interaction Architecture

## Reusable P2P Communication Platform

**Status:** UI/UX architecture specification  
**UI Series:** Part 01  
**Desktop UI:** Dioxus  
**Android UI:** Pure Kotlin + Jetpack Compose  
**Core runtime:** Rust  

---

# 1. Core Decision

The product will use:

```text
Desktop:
    Rust
    Dioxus

Android:
    Kotlin
    Jetpack Compose

Shared:
    Rust domain/core/runtime
    Rust networking
    Rust storage
    Rust security
    Rust sync
    Rust calls/media policy
    Rust search
    Rust backup
```

The UI frameworks are intentionally different.

The shared truth remains:

```text
Rust domain model
+
Rust application services
+
Rust runtime state
```

The two UIs are presentation clients of the same core.

---

# 2. Why This Split Is Strong

A single cross-platform UI framework can reduce UI code duplication, but it can also force each platform into the lowest common denominator.

Android has highly platform-specific concerns:

```text
permissions
IME
keyboard
predictive back
notifications
foreground services
share sheet
camera
Bluetooth
NFC
Picture-in-Picture
audio routing
activity/process lifecycle
accessibility
adaptive layouts
```

Jetpack Compose handles these naturally.

Desktop has different concerns:

```text
multi-window
keyboard-first navigation
mouse/trackpad
system tray
drag-and-drop
window sizing
desktop notifications
file pickers
global shortcuts
large-screen density
```

Dioxus keeps the desktop application Rust-first.

---

# 3. Prime Rule

> **UI owns presentation state. Rust owns product truth.**

UI may own:

```text
current tab
selected row
open dialog
scroll position
temporary form input
animation state
hover state
local expansion
```

Rust owns:

```text
messages
conversation state
delivery/read state
identity
security
contact state
file transfers
call state
presence truth
search results
backup state
notification policy
plugin state
device state
```

---

# 4. Avoid Two Independent Products

Do not build:

```text
Compose ViewModel
    ↓
Android-only business logic
    ↓
Android-only persistence
```

while desktop has a separate implementation.

Instead:

```text
Compose
    ↓
Android UI Adapter
    ↓
Rust Core

Dioxus
    ↓
Desktop UI Adapter
    ↓
Rust Core
```

---

# 5. Shared Presentation Contract

Both UIs consume stable screen-ready concepts:

```text
ConversationSummary
MessageView
ContactSummary
CallSnapshot
PresenceSummary
TransferProgress
SearchHit
SecurityEvent
BackupStatus
```

Never expose raw database rows.

---

# 6. Shared Command Model

Examples:

```rust
pub enum AppCommand {
    SendMessage(...),
    EditMessage(...),
    DeleteMessage(...),
    MarkRead(...),
    StartCall(...),
    AcceptCall(...),
    RejectCall(...),
    StartFileTransfer(...),
    CancelTransfer(...),
    Search(...),
    BlockPeer(...),
    VerifyPeer(...),
}
```

Both frontends ultimately invoke the same commands.

---

# 7. Shared Event Model

Examples:

```rust
pub enum AppEvent {
    ConversationUpdated(...),
    MessageInserted(...),
    MessageChanged(...),
    PresenceChanged(...),
    CallChanged(...),
    TransferChanged(...),
    SecurityAlert(...),
    BackupChanged(...),
}
```

---

# 8. Snapshot + Event Pattern

Recommended screen lifecycle:

```text
open screen
    ↓
request current snapshot
    ↓
render immediately
    ↓
subscribe to incremental events
    ↓
update projection
```

The UI should not reconstruct full application truth from event streams alone.

---

# 9. Shared UI Layer in Rust

Suggested crates:

```text
crates/
├── comm-ui-model/
├── comm-ui-service/
├── comm-ui-events/
├── comm-ui-actions/
├── comm-ui-format/
└── comm-ui-testkit/
```

Applications:

```text
apps/
├── desktop-dioxus/
└── android-compose/
```

---

# 10. `comm-ui-model`

Contains UI-safe DTOs.

Example:

```rust
pub struct ConversationSummary {
    pub id: ConversationId,
    pub title: String,
    pub subtitle: Option<String>,
    pub unread_count: u32,
    pub muted: bool,
    pub pinned: bool,
    pub presence: Option<PresenceSummary>,
    pub last_activity: Timestamp,
}
```

Do not expose:

```text
database connections
crypto secrets
Iroh endpoint internals
actor handles
platform pointers
```

---

# 11. Desktop Boundary

Dioxus can call Rust presentation/application services directly.

```text
Dioxus component
    ↓
Presenter / Hook
    ↓
Rust Presentation Service
    ↓
Rust Application/Core
```

If using daemon mode:

```text
Dioxus UI
    ↓
typed local IPC
    ↓
Rust daemon
```

The UI semantics stay the same.

---

# 12. Android Boundary

Recommended Android path:

```text
Jetpack Compose
    ↓
ViewModel
    ↓
RustBridge / Repository
    ↓
JNI
    ↓
Rust Presentation Service
    ↓
Rust Application/Core
```

JNI should expose coarse-grained semantic operations.

Good:

```text
getConversationList()
sendMessage()
observeConversation()
startCall()
```

Bad:

```text
queryRawSql()
getIrohStream()
decryptRawPacket()
```

---

# 13. Kotlin ViewModel Role

Android ViewModel owns:

```text
screen lifecycle
StateFlow
navigation-facing state
temporary UI state
permission prompts
platform intents
```

It should not own:

```text
message delivery rules
security state
call state machine
sync truth
file-transfer truth
```

---

# 14. Compose State

Recommended:

```text
Rust event stream
    ↓
JNI event adapter
    ↓
Kotlin coroutine
    ↓
StateFlow<ScreenUiState>
    ↓
Compose
```

Avoid unrestricted callbacks from arbitrary Rust worker threads directly into Compose.

---

# 15. Desktop State

Dioxus consumes the same semantic snapshots/events with Rust-native mechanisms such as signals/resources/channels.

---

# 16. Shared Semantics, Independent Layouts

These semantics must be identical:

```text
what Delivered means
what Read means
what Blocked means
when a message is editable
call state
security warning severity
transfer state
```

But layout should differ by platform.

Do not force pixel parity.

---

# 17. Platform-Native UX

Android should feel Android-native.

Desktop should feel desktop-native.

Share:

```text
product semantics
terminology
information hierarchy
design language
```

Do not force identical:

```text
navigation
density
gestures
window structure
```

---

# 18. Shared Design Language

Define conceptual tokens for:

```text
spacing
corner radius
typography hierarchy
semantic colors
status terminology
icon meanings
motion principles
```

Implement separately.

Desktop Dioxus:

```text
CSS/theme variables
```

Android Compose:

```text
MaterialTheme + custom tokens
```

---

# 19. Primary Product Areas

Recommended top-level areas:

```text
Chats
Calls
Contacts
Files
Search
Devices
Settings
```

Optional:

```text
Plugins
Emergency
Diagnostics
```

---

# 20. Desktop Navigation

Preferred architecture:

```text
Left rail/sidebar
    Chats
    Calls
    Contacts
    Files

Secondary pane
    list

Main pane
    active content

Optional details pane
    info/search/media/security
```

Desktop should exploit:

```text
keyboard
mouse
hover
split panes
context menus
resizable layouts
multi-selection
```

---

# 21. Android Navigation

Phone:

```text
Chats
Calls
Contacts
```

as primary destinations.

Secondary features:

```text
Files
Devices
Settings
Search
```

through app bar/navigation hierarchy.

Tablet/foldable:

```text
navigation rail
+
list pane
+
detail pane
```

when width allows.

---

# 22. Conversation Screen Semantics

Shared structure:

```text
header
message timeline
presence/typing
composer
attachments
call actions
```

Desktop may use wide split panes.

Android phone should use a focused single-pane conversation.

---

# 23. Message Timeline Ownership

Rust owns:

```text
paged messages
stable MessageId
message revisions
delivery/read state
```

UI owns:

```text
viewport
scroll anchor
selection
animations
```

Key rows by `MessageId`, never list index.

---

# 24. Paging

Do not load complete history.

Use:

```text
load recent window
scroll upward
load older page
preserve anchor
```

If user is reading old history and new messages arrive:

```text
show N new messages
```

Do not yank the viewport.

---

# 25. Read Detection

Part 30 owns semantics.

UI reports what is actually read.

Desktop factors:

```text
window focused
conversation active
message visible
```

Android factors:

```text
activity resumed
conversation visible
LazyColumn viewport
device/app state
```

---

# 26. Composer

UI may hold draft text locally.

When user sends:

```text
UI command
→ Rust creates durable pending message
→ Rust returns MessageView
→ UI renders pending state
```

Do not create a fake UI-only message that later needs difficult reconciliation.

---

# 27. Message States

Visual states:

```text
Queued
Sending
Sent
Delivered
Read
Failed
```

Failure actions:

```text
Retry
Delete
Details
```

---

# 28. Message Actions

Depending permissions/type:

```text
Reply
Copy
Edit
Delete
Forward
Save attachment
Info
```

Android:

```text
long press
selection mode
bottom sheet
```

Desktop:

```text
right click
hover actions
keyboard shortcuts
multi-select
```

---

# 29. Calls

Part 29 remains authoritative.

UI renders:

```text
Incoming
Outgoing ringing
Connecting
Active
Reconnecting
Held
Ended
```

Android Compose integrates:

```text
full-screen call UI
PiP
notification actions
audio route chooser
```

Desktop Dioxus may provide:

```text
main call screen
floating/minimized call panel
separate call window
```

---

# 30. Raw Media Must Bypass Ordinary UI State

Android:

```text
Surface/native renderer
```

Desktop:

```text
native video/GPU renderer handle
```

UI receives:

```text
RendererId
participant
mute state
quality state
```

not raw video frames or PCM.

---

# 31. File Transfer UX

Common states:

```text
Preparing
Waiting for peer
Transferring
Paused
Verifying
Complete
Failed
```

Show:

```text
progress
bytes
speed
ETA when stable
```

Do not let UI own transfer state.

---

# 32. Large Files Across Android JNI

Use:

```text
file descriptor
stream handle
content URI adapter
```

Never transfer giant file contents through `ByteArray`.

---

# 33. Search

Part 32 supplies search engine.

Desktop:

```text
keyboard-first global search
multi-section results
wide detail view
```

Android:

```text
search app bar/destination
filter chips
single-column results
```

Search hit navigation uses stable logical IDs such as `MessageId`.

---

# 34. Contacts and Identity

UI should distinguish:

```text
saved contact
verified identity
nearby discovered peer
unknown request
blocked peer
```

A saved display name does not equal cryptographic verification.

---

# 35. Device Management

Devices screen should show:

```text
This device
Other authorized devices
Last active
Verification
Revoke
```

High-risk actions such as revoke require clear confirmation.

---

# 36. Pairing / Nearby UX

Separate:

```text
Add Contact
Link My Device
Nearby Devices
Scan QR
Tap NFC
```

Own-device linking must not look identical to adding a contact.

---

# 37. Pairing Flow

```text
Discover / Scan
    ↓
Authenticate
    ↓
Confirm SAS
    ↓
Name device/contact
    ↓
Complete
```

Errors:

```text
Expired code
Already used
Identity mismatch
Unsupported version
```

---

# 38. Security UX

Do not overload the normal chat UI with crypto details.

Show security state when actionable:

```text
Verified
Identity changed
New device added
Device revoked
Unverified authority
```

Provide a dedicated Security Center.

---

# 39. Security Center

Recommended sections:

```text
My Devices
Trusted Contacts
Identity / Fingerprint
Security Events
Blocked Peers
Recovery
```

---

# 40. Emergency UX

Part 17 deserves dedicated visual treatment.

Potential surfaces:

```text
SOS
Verified Alerts
Emergency Contacts
Offline Mesh Status
```

Prevent accidental SOS with an intentional interaction:

```text
press and hold
confirmation
short countdown
```

depending product use case.

---

# 41. Settings Information Architecture

Organize by user intent:

```text
Account
Privacy & Security
Notifications
Calls & Media
Storage & Data
Devices
Appearance
Plugins
Advanced
About
```

Do not expose backend module names such as:

```text
DTN Scheduler
ALPN
Ratchet Store
Resource Limiter
```

to ordinary users.

---

# 42. Progressive Disclosure

Normal users see simple defaults.

Advanced users can open:

```text
Network Diagnostics
Protocol/Codec Information
Developer Mode
Plugin Developer Tools
```

---

# 43. Error Architecture

Classify UI-facing failures:

```rust
pub enum UiFailureClass {
    Transient,
    ActionRequired,
    Permission,
    Security,
    Storage,
    Unsupported,
    Fatal,
}
```

---

# 44. Error Presentation

Transient:

```text
small banner/snackbar
retry automatically where safe
```

Action required:

```text
clear banner + action
```

Security:

```text
strong warning
no silent retry
```

Fatal:

```text
recovery screen
diagnostics
backup restore path
```

---

# 45. Offline UX

Offline is a normal operating state.

Avoid treating it as application failure.

Examples:

```text
Queued
Will send when a connection is available
```

Use subtle status unless user needs action.

---

# 46. Local-First UX

Existing local conversations, files, contacts, and search should appear without waiting for Internet.

Prefer:

```text
slightly stale local data
```

over:

```text
blank spinner waiting for network
```

---

# 47. Loading States

Use progressive/skeleton states for lists.

Avoid fullscreen spinners for ordinary local reads.

---

# 48. Empty States

Each major destination needs an intentional empty state:

```text
No conversations yet
No files yet
No linked devices
No search results
```

Each should offer one useful next action.

---

# 49. Accessibility

Accessibility is architectural, not polish.

Android Compose:

```text
TalkBack
semantics
focus order
content descriptions
minimum touch targets
font scaling
reduced animation
```

Desktop Dioxus:

```text
keyboard navigation
screen reader semantics
focus indicators
high contrast
UI scale
```

---

# 50. Desktop Keyboard-First Interaction

Important shortcuts:

```text
Ctrl/Cmd+K → Global Search
Ctrl/Cmd+N → New Conversation
Ctrl/Cmd+F → Search Conversation
Ctrl/Cmd+, → Settings
Esc → Close dialog/panel
```

Every critical action must remain available without hover-only UI.

---

# 51. Localization

Visible strings belong to platform/UI localization resources.

Rust should return:

```text
stable error/status codes
structured parameters
```

not fully formatted English strings.

Rust:

```text
PeerUnavailable { peer: ... }
```

UI:

```text
localized human text
```

---

# 52. Time and Number Formatting

UI layer formats:

```text
12/24-hour time
dates
relative time
file sizes
durations
pluralization
```

Rust supplies canonical values.

---

# 53. Android Process Death

Compose/ViewModel state can disappear.

Rust durable truth reloads.

`SavedStateHandle` should restore presentation concerns such as:

```text
current destination
selected conversation
draft ID
scroll anchor
```

not stale:

```text
typing
presence
fake call state
```

---

# 54. Android Platform Adapter Responsibilities

Kotlin owns platform mechanics for:

```text
Activity
permissions
notifications
foreground services
share sheet
file picker
Camera permission
Bluetooth permission
NFC
BiometricPrompt
PiP
predictive back
```

Rust owns the semantic decision.

---

# 55. Platform Request Pattern

Example:

```text
User taps Video Call
    ↓
Rust validates call state
    ↓
Rust requests CameraPermission
    ↓
Kotlin launches Android permission flow
    ↓
result returned to Rust
    ↓
Rust proceeds or degrades
```

---

# 56. File Picker Pattern

```text
User taps Attach
    ↓
Compose opens Android system picker
    ↓
URI/FD returned
    ↓
Rust file subsystem receives safe handle
    ↓
normal file-transfer pipeline
```

---

# 57. Desktop Platform Adapter Responsibilities

Examples:

```text
file dialogs
drag/drop
clipboard
system notifications
tray
window management
open URL
```

---

# 58. Command/Event Flow

Recommended:

```text
UI
→ semantic command
→ Rust application service
→ state transition
→ event/snapshot update
→ UI
```

Avoid two-way mutable shared state.

---

# 59. High-Frequency State

Examples:

```text
call quality
audio level
transfer progress
typing
```

should be coalesced/throttled before reaching UI.

Do not emit hundreds of updates per second into Compose or Dioxus.

---

# 60. Virtualization

Android:

```text
LazyColumn
LazyGrid
```

Desktop:

```text
virtualized long lists
```

Required for:

```text
large message histories
contacts
files
search results
```

---

# 61. Media Thumbnails

Use appropriately sized cached thumbnails.

Do not decode original multi-megabyte media just to draw a tiny row preview.

---

# 62. Multi-Window Desktop

Potential windows:

```text
Main
Call
Media Viewer
Settings
Diagnostics
```

Window state is presentation state.

Persist:

```text
size
position
last section
```

locally.

---

# 63. Responsive Android

Use adaptive Compose layouts for:

```text
phone
tablet
foldable
desktop-like Android window
```

Do not hardcode phone-only dimensions.

---

# 64. UI Security Boundary

UI must never receive:

```text
private keys
ratchet secrets
raw recovery material unless explicitly displaying recovery screen
unredacted crypto internals
```

Sensitive screen content should be minimized in logs/crash reports.

---

# 65. Clipboard and Recovery UX

Copying:

```text
recovery key
fingerprint
```

is explicit.

Android may optionally block screenshots on recovery screens if product policy chooses.

---

# 66. Authentication Gate

Sensitive actions/screens may request:

```text
biometric
device credential
```

through platform adapter.

---

# 67. UI Testing Strategy

Test:

```text
semantics
navigation
state mapping
accessibility
error flows
process death
permissions
offline behavior
```

not just screenshots.

---

# 68. Android Tests

Use:

```text
ViewModel tests
Compose UI tests
JNI adapter tests
process-death tests
permission tests
adaptive-layout tests
```

---

# 69. Desktop Tests

Use:

```text
component logic tests
keyboard navigation tests
window-layout tests
local IPC adapter tests
```

---

# 70. Shared Contract Tests

Given a Rust presentation snapshot, desktop and Android should expose equivalent product semantics.

They do not need identical pixels.

---

# 71. Performance Quality Gates

Measure:

```text
app shell startup
conversation open
message scroll
search latency
contact-list scroll
call-screen updates
```

Existing local conversation should open without waiting for network.

---

# 72. UI Series Roadmap

Recommended next parts:

```text
UI/UX 01 — Product UX Foundation & Cross-Platform Interaction Architecture
UI/UX 02 — Desktop Dioxus App Shell, Navigation & Window Architecture
UI/UX 03 — Android Compose App Shell, Navigation & Lifecycle Architecture
UI/UX 04 — Conversation List / Inbox UX
UI/UX 05 — Conversation / Message Timeline UX
UI/UX 06 — Message Composer, Attachments, Voice Notes & Drafts UX
UI/UX 07 — Calls & Realtime Media UX
UI/UX 08 — Contacts, Requests, Verification & Identity UX
UI/UX 09 — Groups, Membership & Roles UX
UI/UX 10 — Files, Media Gallery & Transfer UX
UI/UX 11 — Search & Local Knowledge Retrieval UX
UI/UX 12 — Nearby, QR/NFC Pairing & Device Linking UX
UI/UX 13 — Notifications, Background & Incoming Call UX
UI/UX 14 — Presence, Typing, Receipts & Status UX
UI/UX 15 — Security Center, Devices, Keys & Recovery UX
UI/UX 16 — Backup, Restore, Export & Migration UX
UI/UX 17 — Emergency / SOS / Offline Mesh UX
UI/UX 18 — Settings, Privacy, Notifications & Data Controls UX
UI/UX 19 — Plugin / Module Ecosystem UX
UI/UX 20 — Diagnostics, Network Paths & Advanced Developer UX
UI/UX 21 — Accessibility & Inclusive Interaction Architecture
UI/UX 22 — Design System, Tokens, Typography, Icons & Motion
UI/UX 23 — Responsive / Adaptive Desktop, Tablet, Foldable & Phone Layouts
UI/UX 24 — Error, Loading, Empty, Offline & Degraded-State UX
UI/UX 25 — Onboarding, First Run & Permission Education UX
UI/UX 26 — Performance, Virtualization & Large-Data UI Architecture
UI/UX 27 — UI Testing, Screenshot/Interaction Tests & Release Quality Gates
```

---

# 73. Why UI/UX Should Be Designed Now

The backend is already deep enough that continuing backend-only design risks:

```text
awkward presentation APIs
duplicated UI projections
missing user flows
overengineered invisible features
late discovery of lifecycle problems
```

UI/UX design now provides feedback into:

```text
API shapes
events
pagination
error models
permissions
notifications
calls
search
security
```

---

# 74. UI-Driven Backend Validation

Every screen should answer:

```text
What snapshot does it need?
What commands can the user issue?
What events update it?
What happens offline?
What happens after process death?
What is sensitive?
What is platform-specific?
```

If the backend cannot answer cleanly, the backend contract needs refinement.

---

# 75. Shared Presentation Service

The best place to maximize reuse is not widget code.

It is:

```text
screen-ready DTOs
validation
commands
events
business semantics
```

Example:

```rust
pub trait ConversationPresentation {
    async fn list_conversations(
        &self,
        page: ConversationPageRequest,
    ) -> Result<ConversationPage, UiError>;

    async fn open_conversation(
        &self,
        id: ConversationId,
    ) -> Result<ConversationScreenSnapshot, UiError>;

    async fn send_message(
        &self,
        command: SendMessageCommand,
    ) -> Result<MessageView, UiError>;
}
```

---

# 76. Android Event Bridge

Recommended:

```text
Rust bounded event channel
    ↓
JNI bridge
    ↓
Coroutine
    ↓
StateFlow
    ↓
Compose
```

The bridge should be lifecycle-aware and bounded.

---

# 77. Desktop Event Bridge

Dioxus can consume the same Rust event channel/presentation layer directly or through local daemon IPC.

---

# 78. Shared Validation

Rust remains authority for:

```text
message length
attachment count
file size
call eligibility
security policy
peer permissions
```

UI may duplicate lightweight prevalidation only for responsiveness.

---

# 79. Recomposition/Re-render Safety

Compose recomposition and Dioxus re-rendering must never:

```text
send a message twice
start a call twice
open duplicate subscriptions
repeat destructive commands
```

Side effects belong in explicit event handlers/effects.

---

# 80. Subscription Lifecycle

Screen-specific subscriptions should exist only while useful.

Examples:

```text
typing → current conversation
transfer progress → visible transfer
presence → visible contacts/conversation
```

Global streams:

```text
active call
security alerts
core health
```

can remain app-wide.

---

# 81. Definition of Done

UI/UX Part 01 is complete when:

- desktop uses Dioxus and Android uses pure Kotlin + Jetpack Compose
- both UIs consume one shared Rust application/domain core
- no business logic is independently reimplemented in Android
- UI owns presentation state while Rust owns product truth
- shared UI-safe DTOs exist
- commands/events/snapshots are typed
- Android JNI is semantic and coarse-grained
- Kotlin ViewModels convert Rust events into Compose-friendly `StateFlow`
- Dioxus consumes the same presentation service directly or via local IPC
- platform navigation/layouts are free to differ
- Android remains Android-native
- desktop remains keyboard/mouse/window optimized
- message, call, file, search, presence, security, and backup semantics remain identical
- platform permissions/OS services are isolated behind adapters
- raw media and large file bytes never flow through ordinary UI state
- accessibility, localization, font scaling, reduced motion, and keyboard navigation are first-class
- loading/offline/error/degraded states are part of every screen contract
- backend features are validated against real screen requirements before further deep backend expansion

---

# 82. Final Architecture

```text
                           RUST CORE
                               │
                    Presentation Service
                    Commands / Events
                     ┌─────────┴─────────┐
                     │                   │
                DESKTOP               ANDROID
                     │                   │
                  Dioxus              JNI Bridge
                     │                   │
             Rust Presenter            ViewModel
                                         │
                                      StateFlow
                                         │
                                  Jetpack Compose
```

Platform integrations:

```text
Desktop:
    windows
    tray
    file dialogs
    clipboard
    notifications
    drag/drop

Android:
    Activity
    permissions
    notifications
    foreground services
    camera/audio
    NFC/Bluetooth
    file picker
    share sheet
    PiP
```

Both operate on the same domain identities:

```text
ConversationId
MessageId
CallId
TransferId
DeviceId
SearchHit
SecurityEvent
```

---

# 83. Final Principle

The objective is not maximum UI-code sharing.

The objective is maximum:

```text
business-logic sharing
semantic consistency
platform quality
maintainability
```

The ideal split is:

```text
Rust:
    truth
    networking
    storage
    security
    sync
    calls
    search
    backup
    screen-ready presentation models

Dioxus:
    desktop interaction and presentation

Kotlin + Jetpack Compose:
    Android interaction and presentation
```

That gives both platforms native-quality UX without creating two separate implementations of the communication system.
